//! Fixed-size page storage and a bounded LRU buffer pool.
//!
//! Pages use a stable little-endian header and an FNV-1a checksum.
//! Implements a slotted page architecture (header, slot directory, free space,
//! and bottom-up tuple data) as described in the ChronoDB architecture guide.
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

pub const PAGE_SIZE: usize = 4096;
pub const PAGE_HEADER_SIZE: usize = 64;
const MAGIC: &[u8; 4] = b"CHDB";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PageType {
    Header = 0,
    Catalog = 1,
    Index = 2,
    Data = 3,
    Wal = 4,
}

impl TryFrom<u8> for PageType {
    type Error = String;
    fn try_from(v: u8) -> Result<Self, Self::Error> {
        match v {
            0 => Ok(Self::Header),
            1 => Ok(Self::Catalog),
            2 => Ok(Self::Index),
            3 => Ok(Self::Data),
            4 => Ok(Self::Wal),
            _ => Err(format!("invalid page type {v}")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    pub id: u32,
    pub kind: PageType,
    pub lsn: u64,
    pub free_space_ptr: u16,
    pub num_slots: u16,
    pub bytes: Box<[u8; PAGE_SIZE]>,
}

impl Page {
    pub fn new(id: u32, kind: PageType) -> Self {
        Self {
            id,
            kind,
            lsn: 0,
            free_space_ptr: PAGE_SIZE as u16,
            num_slots: 0,
            bytes: Box::new([0; PAGE_SIZE]),
        }
    }

    pub fn payload(&self) -> &[u8] {
        &self.bytes[PAGE_HEADER_SIZE..]
    }

    pub fn payload_mut(&mut self) -> &mut [u8] {
        &mut self.bytes[PAGE_HEADER_SIZE..]
    }

    /// Insert a tuple into a slotted data page.
    /// Returns the slot index if space is available, or None if the page is full.
    pub fn insert_tuple(&mut self, data: &[u8]) -> Option<u16> {
        let slot_pos = PAGE_HEADER_SIZE + (self.num_slots as usize) * 4;
        let needed = 4 + data.len();
        let available = (self.free_space_ptr as usize).saturating_sub(slot_pos);
        if needed > available || data.len() > u16::MAX as usize {
            return None;
        }

        self.free_space_ptr -= data.len() as u16;
        let offset = self.free_space_ptr;
        self.bytes[offset as usize..offset as usize + data.len()].copy_from_slice(data);

        self.bytes[slot_pos..slot_pos + 2].copy_from_slice(&offset.to_le_bytes());
        self.bytes[slot_pos + 2..slot_pos + 4].copy_from_slice(&(data.len() as u16).to_le_bytes());
        let slot_idx = self.num_slots;
        self.num_slots += 1;
        Some(slot_idx)
    }

    /// Get tuple slice by slot index. Returns None if slot index is out of range or deleted.
    pub fn get_tuple(&self, slot: u16) -> Option<&[u8]> {
        if slot >= self.num_slots {
            return None;
        }
        let slot_pos = PAGE_HEADER_SIZE + (slot as usize) * 4;
        let offset =
            u16::from_le_bytes(self.bytes[slot_pos..slot_pos + 2].try_into().ok()?) as usize;
        let len =
            u16::from_le_bytes(self.bytes[slot_pos + 2..slot_pos + 4].try_into().ok()?) as usize;
        if len == 0 || offset + len > PAGE_SIZE {
            return None;
        }
        Some(&self.bytes[offset..offset + len])
    }

    /// Delete tuple by setting its slot length to 0 (tombstone).
    pub fn delete_tuple(&mut self, slot: u16) -> bool {
        if slot >= self.num_slots {
            return false;
        }
        let slot_pos = PAGE_HEADER_SIZE + (slot as usize) * 4;
        self.bytes[slot_pos + 2..slot_pos + 4].copy_from_slice(&0u16.to_le_bytes());
        true
    }

    /// Update tuple at a specific slot.
    pub fn update_tuple(&mut self, slot: u16, data: &[u8]) -> bool {
        if slot >= self.num_slots {
            return false;
        }
        let slot_pos = PAGE_HEADER_SIZE + (slot as usize) * 4;
        let cur_offset =
            u16::from_le_bytes(self.bytes[slot_pos..slot_pos + 2].try_into().unwrap()) as usize;
        let cur_len =
            u16::from_le_bytes(self.bytes[slot_pos + 2..slot_pos + 4].try_into().unwrap()) as usize;

        if cur_len == data.len() {
            self.bytes[cur_offset..cur_offset + data.len()].copy_from_slice(data);
            return true;
        }

        let slots_end = PAGE_HEADER_SIZE + (self.num_slots as usize) * 4;
        let available = (self.free_space_ptr as usize).saturating_sub(slots_end);
        if data.len() <= available {
            self.free_space_ptr -= data.len() as u16;
            let new_offset = self.free_space_ptr;
            self.bytes[new_offset as usize..new_offset as usize + data.len()]
                .copy_from_slice(data);
            self.bytes[slot_pos..slot_pos + 2].copy_from_slice(&new_offset.to_le_bytes());
            self.bytes[slot_pos + 2..slot_pos + 4]
                .copy_from_slice(&(data.len() as u16).to_le_bytes());
            return true;
        }

        self.defragment();
        let slots_end = PAGE_HEADER_SIZE + (self.num_slots as usize) * 4;
        let available = (self.free_space_ptr as usize).saturating_sub(slots_end);
        if data.len() <= available {
            self.free_space_ptr -= data.len() as u16;
            let new_offset = self.free_space_ptr;
            self.bytes[new_offset as usize..new_offset as usize + data.len()]
                .copy_from_slice(data);
            let slot_pos = PAGE_HEADER_SIZE + (slot as usize) * 4;
            self.bytes[slot_pos..slot_pos + 2].copy_from_slice(&new_offset.to_le_bytes());
            self.bytes[slot_pos + 2..slot_pos + 4]
                .copy_from_slice(&(data.len() as u16).to_le_bytes());
            return true;
        }
        false
    }

    /// Compacts active tuples towards the bottom of the page to reclaim unused space.
    pub fn defragment(&mut self) {
        let mut active = Vec::new();
        for s in 0..self.num_slots {
            let slot_pos = PAGE_HEADER_SIZE + (s as usize) * 4;
            let offset =
                u16::from_le_bytes(self.bytes[slot_pos..slot_pos + 2].try_into().unwrap()) as usize;
            let len = u16::from_le_bytes(self.bytes[slot_pos + 2..slot_pos + 4].try_into().unwrap())
                as usize;
            if len > 0 && offset + len <= PAGE_SIZE {
                let data = self.bytes[offset..offset + len].to_vec();
                active.push((s, data));
            }
        }

        let mut write_ptr = PAGE_SIZE;
        for (s, data) in active {
            write_ptr -= data.len();
            self.bytes[write_ptr..write_ptr + data.len()].copy_from_slice(&data);
            let slot_pos = PAGE_HEADER_SIZE + (s as usize) * 4;
            let offset = write_ptr as u16;
            let len = data.len() as u16;
            self.bytes[slot_pos..slot_pos + 2].copy_from_slice(&offset.to_le_bytes());
            self.bytes[slot_pos + 2..slot_pos + 4].copy_from_slice(&len.to_le_bytes());
        }
        self.free_space_ptr = write_ptr as u16;
    }

    pub fn encode(&self) -> [u8; PAGE_SIZE] {
        let mut out = *self.bytes;
        out[..4].copy_from_slice(MAGIC);
        out[4..8].copy_from_slice(&self.id.to_le_bytes());
        out[8] = self.kind as u8;
        out[9..11].copy_from_slice(&self.free_space_ptr.to_le_bytes());
        out[11..13].copy_from_slice(&self.num_slots.to_le_bytes());
        out[13..17].fill(0);
        out[17..25].copy_from_slice(&self.lsn.to_le_bytes());
        let checksum = checksum(&out);
        out[13..17].copy_from_slice(&checksum.to_le_bytes());
        out
    }

    pub fn decode(mut bytes: [u8; PAGE_SIZE], expected: u32) -> Result<Self, String> {
        if &bytes[..4] != MAGIC {
            return Err(format!("page {expected} has an invalid signature"));
        }
        let id = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        if id != expected {
            return Err(format!("page id mismatch: expected {expected}, found {id}"));
        }
        let stored = u32::from_le_bytes(bytes[13..17].try_into().unwrap());
        bytes[13..17].fill(0);
        let actual = checksum(&bytes);
        if stored != actual {
            return Err(format!("checksum mismatch on page {id}"));
        }
        bytes[13..17].copy_from_slice(&stored.to_le_bytes());
        let kind = PageType::try_from(bytes[8])?;
        let free_space_ptr = u16::from_le_bytes(bytes[9..11].try_into().unwrap());
        let num_slots = u16::from_le_bytes(bytes[11..13].try_into().unwrap());
        let lsn = u64::from_le_bytes(bytes[17..25].try_into().unwrap());
        Ok(Self {
            id,
            kind,
            lsn,
            free_space_ptr,
            num_slots,
            bytes: Box::new(bytes),
        })
    }
}

pub fn checksum(bytes: &[u8]) -> u32 {
    let mut h = 0x811c9dc5u32;
    for b in bytes {
        h ^= *b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h
}

pub struct PageManager {
    file: File,
    pages: u32,
}

impl PageManager {
    pub fn create(path: impl AsRef<Path>) -> Result<Self, String> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        Ok(Self { file, pages: 0 })
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        let len = file.metadata().map_err(|e| e.to_string())?.len();
        if len % PAGE_SIZE as u64 != 0 {
            return Err("database file length is not page-aligned".into());
        }
        let pages = (len / PAGE_SIZE as u64) as u32;
        Ok(Self { file, pages })
    }

    pub fn page_count(&self) -> u32 {
        self.pages
    }

    pub fn allocate(&mut self, kind: PageType) -> Result<Page, String> {
        let page = Page::new(self.pages, kind);
        self.pages = self.pages.checked_add(1).ok_or("page id space exhausted")?;
        self.write(&page)?;
        Ok(page)
    }

    pub fn read(&mut self, id: u32) -> Result<Page, String> {
        if id >= self.pages {
            return Err(format!("page {id} does not exist"));
        }
        let mut raw = [0; PAGE_SIZE];
        self.file
            .seek(SeekFrom::Start(id as u64 * PAGE_SIZE as u64))
            .and_then(|_| self.file.read_exact(&mut raw))
            .map_err(|e| e.to_string())?;
        Page::decode(raw, id)
    }

    pub fn write(&mut self, page: &Page) -> Result<(), String> {
        if page.id >= self.pages {
            return Err(format!("cannot write unallocated page {}", page.id));
        }
        let raw = page.encode();
        self.file
            .seek(SeekFrom::Start(page.id as u64 * PAGE_SIZE as u64))
            .and_then(|_| self.file.write_all(&raw))
            .map_err(|e| e.to_string())
    }

    pub fn sync(&mut self) -> Result<(), String> {
        self.file.sync_all().map_err(|e| e.to_string())
    }
}

#[derive(Clone)]
struct Frame {
    page: Page,
    pins: u32,
    dirty: bool,
    touched: u64,
}

pub struct BufferPool {
    disk: PageManager,
    capacity: usize,
    tick: u64,
    frames: HashMap<u32, Frame>,
    pub evictions: u64,
    pub total_requests: u64,
    pub cache_hits: u64,
}

impl BufferPool {
    pub fn new(disk: PageManager, capacity: usize) -> Result<Self, String> {
        if capacity == 0 {
            return Err("buffer pool capacity must be positive".into());
        }
        Ok(Self {
            disk,
            capacity,
            tick: 0,
            frames: HashMap::new(),
            evictions: 0,
            total_requests: 0,
            cache_hits: 0,
        })
    }

    fn tick(&mut self) -> u64 {
        self.tick = self.tick.wrapping_add(1);
        self.tick
    }

    fn make_room(&mut self) -> Result<(), String> {
        if self.frames.len() < self.capacity {
            return Ok(());
        }
        let victim = self
            .frames
            .iter()
            .filter(|(_, f)| f.pins == 0)
            .min_by_key(|(_, f)| f.touched)
            .map(|(id, _)| *id)
            .ok_or("all buffer pool pages are pinned")?;
        let frame = self.frames.remove(&victim).unwrap();
        if frame.dirty {
            self.disk.write(&frame.page)?;
        }
        self.evictions += 1;
        Ok(())
    }

    pub fn fetch(&mut self, id: u32) -> Result<&mut Page, String> {
        self.total_requests += 1;
        let now = self.tick();
        if self.frames.contains_key(&id) {
            self.cache_hits += 1;
        } else {
            self.make_room()?;
            let page = self.disk.read(id)?;
            self.frames.insert(
                id,
                Frame {
                    page,
                    pins: 0,
                    dirty: false,
                    touched: now,
                },
            );
        }
        let f = self.frames.get_mut(&id).unwrap();
        f.pins += 1;
        f.touched = now;
        Ok(&mut f.page)
    }

    pub fn unpin(&mut self, id: u32, dirty: bool) -> Result<(), String> {
        let f = self
            .frames
            .get_mut(&id)
            .ok_or_else(|| format!("page {id} is not buffered"))?;
        if f.pins == 0 {
            return Err(format!("page {id} is not pinned"));
        }
        f.pins -= 1;
        f.dirty |= dirty;
        Ok(())
    }

    pub fn new_page(&mut self, kind: PageType) -> Result<u32, String> {
        self.make_room()?;
        let page = self.disk.allocate(kind)?;
        let id = page.id;
        let now = self.tick();
        self.frames.insert(
            id,
            Frame {
                page,
                pins: 1,
                dirty: false,
                touched: now,
            },
        );
        Ok(id)
    }

    pub fn flush_all(&mut self) -> Result<(), String> {
        for f in self.frames.values_mut() {
            if f.dirty {
                self.disk.write(&f.page)?;
                f.dirty = false;
            }
        }
        self.disk.sync()
    }

    pub fn resident_pages(&self) -> usize {
        self.frames.len()
    }

    pub fn hit_rate(&self) -> f64 {
        if self.total_requests == 0 {
            0.0
        } else {
            self.cache_hits as f64 / self.total_requests as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "chronodb-pages-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn page_roundtrip_and_checksum() {
        let p = path();
        {
            let mut d = PageManager::open(&p).unwrap();
            let mut page = d.allocate(PageType::Data).unwrap();
            page.payload_mut()[0] = 7;
            d.write(&page).unwrap();
            assert_eq!(d.read(0).unwrap().payload()[0], 7);
        }
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn slotted_page_operations() {
        let mut page = Page::new(0, PageType::Data);
        let s0 = page.insert_tuple(b"first tuple").unwrap();
        let s1 = page.insert_tuple(b"second tuple").unwrap();
        assert_eq!(s0, 0);
        assert_eq!(s1, 1);
        assert_eq!(page.get_tuple(0), Some(&b"first tuple"[..]));
        assert_eq!(page.get_tuple(1), Some(&b"second tuple"[..]));

        assert!(page.update_tuple(0, b"first-updated"));
        assert_eq!(page.get_tuple(0), Some(&b"first-updated"[..]));

        assert!(page.delete_tuple(0));
        assert_eq!(page.get_tuple(0), None);
        assert_eq!(page.get_tuple(1), Some(&b"second tuple"[..]));

        page.defragment();
        assert_eq!(page.get_tuple(1), Some(&b"second tuple"[..]));
    }

    #[test]
    fn lru_evicts_unpinned_page() {
        let p = path();
        {
            let d = PageManager::open(&p).unwrap();
            let mut b = BufferPool::new(d, 1).unwrap();
            let id = b.new_page(PageType::Data).unwrap();
            b.unpin(id, true).unwrap();
            let _ = b.new_page(PageType::Index).unwrap();
            assert_eq!(b.evictions, 1);
        }
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn buffer_pool_statistics() {
        let p = path();
        {
            let d = PageManager::open(&p).unwrap();
            let mut b = BufferPool::new(d, 2).unwrap();
            let p0 = b.new_page(PageType::Data).unwrap();
            b.unpin(p0, false).unwrap();
            let _ = b.fetch(p0).unwrap();
            b.unpin(p0, false).unwrap();
            assert_eq!(b.cache_hits, 1);
            assert_eq!(b.total_requests, 1);
            assert_eq!(b.hit_rate(), 1.0);
        }
        let _ = std::fs::remove_file(p);
    }
}
