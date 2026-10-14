//! A small B+ tree for signed 64-bit keys and 64-bit row/page references.
//! Leaves contain values; internal nodes contain separator keys. Deletion
//! rebuilds the compact tree from the remaining sorted entries.
use crate::storage::{PageManager, PageType};

#[allow(clippy::vec_box)] // Each child is an independently split/owned tree node.
#[derive(Clone, Debug)]
enum Node {
    Leaf {
        keys: Vec<i64>,
        values: Vec<u64>,
    },
    Internal {
        keys: Vec<i64>,
        children: Vec<Box<Node>>,
    },
}
#[derive(Clone, Debug)]
pub struct BPlusTree {
    root: Box<Node>,
    order: usize,
    len: usize,
}
impl BPlusTree {
    pub fn new(order: usize) -> Result<Self, String> {
        if order < 3 {
            return Err("B+ tree order must be at least 3".into());
        }
        Ok(Self {
            root: Box::new(Node::Leaf {
                keys: vec![],
                values: vec![],
            }),
            order,
            len: 0,
        })
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn height(&self) -> usize {
        fn h(n: &Node) -> usize {
            match n {
                Node::Leaf { .. } => 1,
                Node::Internal { children, .. } => 1 + h(&children[0]),
            }
        }
        h(&self.root)
    }
    pub fn insert(&mut self, key: i64, value: u64) -> Option<u64> {
        let old = Self::get_node(&self.root, key);
        let split = Self::insert_node(&mut self.root, key, value, self.order);
        if let Some((sep, right)) = split {
            let prior = std::mem::replace(
                &mut self.root,
                Box::new(Node::Leaf {
                    keys: vec![],
                    values: vec![],
                }),
            );
            self.root = Box::new(Node::Internal {
                keys: vec![sep],
                children: vec![prior, right],
            });
        }
        if old.is_none() {
            self.len += 1
        }
        old
    }
    fn insert_node(
        node: &mut Box<Node>,
        key: i64,
        value: u64,
        order: usize,
    ) -> Option<(i64, Box<Node>)> {
        match node.as_mut() {
            Node::Leaf { keys, values } => {
                let i = keys.partition_point(|k| *k < key);
                if keys.get(i) == Some(&key) {
                    values[i] = value;
                    return None;
                }
                keys.insert(i, key);
                values.insert(i, value);
                if keys.len() <= order {
                    return None;
                }
                let right_keys = keys.split_off(keys.len() / 2);
                let right_values = values.split_off(values.len() - right_keys.len());
                let sep = right_keys[0];
                Some((
                    sep,
                    Box::new(Node::Leaf {
                        keys: right_keys,
                        values: right_values,
                    }),
                ))
            }
            Node::Internal { keys, children } => {
                let i = keys.partition_point(|k| key >= *k);
                if let Some((sep, right)) = Self::insert_node(&mut children[i], key, value, order) {
                    keys.insert(i, sep);
                    children.insert(i + 1, right)
                }
                if keys.len() <= order {
                    return None;
                }
                let mid = keys.len() / 2;
                let promote = keys[mid];
                let right_keys = keys.split_off(mid + 1);
                keys.pop();
                let right_children = children.split_off(mid + 1);
                Some((
                    promote,
                    Box::new(Node::Internal {
                        keys: right_keys,
                        children: right_children,
                    }),
                ))
            }
        }
    }
    fn get_node(node: &Node, key: i64) -> Option<u64> {
        match node {
            Node::Leaf { keys, values } => keys.binary_search(&key).ok().map(|i| values[i]),
            Node::Internal { keys, children } => {
                let i = keys.partition_point(|k| key >= *k);
                Self::get_node(&children[i], key)
            }
        }
    }
    pub fn get(&self, key: i64) -> Option<u64> {
        Self::get_node(&self.root, key)
    }
    fn collect(node: &Node, out: &mut Vec<(i64, u64)>) {
        match node {
            Node::Leaf { keys, values } => {
                out.extend(keys.iter().copied().zip(values.iter().copied()))
            }
            Node::Internal { children, .. } => {
                for c in children {
                    Self::collect(c, out)
                }
            }
        }
    }
    pub fn scan(&self, low: i64, high: i64) -> Vec<(i64, u64)> {
        let mut all = Vec::new();
        Self::collect(&self.root, &mut all);
        all.into_iter()
            .filter(|(k, _)| *k >= low && *k <= high)
            .collect()
    }
    pub fn entries(&self) -> Vec<(i64, u64)> {
        let mut all = Vec::new();
        Self::collect(&self.root, &mut all);
        all
    }
    pub fn remove(&mut self, key: i64) -> Option<u64> {
        let old = self.get(key)?;
        let all = self
            .entries()
            .into_iter()
            .filter(|(k, _)| *k != key)
            .collect::<Vec<_>>();
        self.root = Box::new(Node::Leaf {
            keys: vec![],
            values: vec![],
        });
        self.len = 0;
        for (k, v) in all {
            self.insert(k, v);
        }
        Some(old)
    }
    /// Persist leaf records as index pages and an index-directory page. Internal
    /// separators are rebuilt from the sorted leaf records when reopened.
    pub fn save_pages(&self, disk: &mut PageManager) -> Result<u32, String> {
        let entries = self.entries();
        let per_page = (disk_page_payload() - 3) / 16;
        let chunks = entries.chunks(per_page).collect::<Vec<_>>();
        let mut ids = Vec::new();
        for chunk in chunks {
            let mut p = disk.allocate(PageType::Index)?;
            p.payload_mut()[0] = 1;
            p.payload_mut()[1..3].copy_from_slice(&(chunk.len() as u16).to_le_bytes());
            for (i, (k, v)) in chunk.iter().enumerate() {
                let off = 3 + i * 16;
                p.payload_mut()[off..off + 8].copy_from_slice(&k.to_le_bytes());
                p.payload_mut()[off + 8..off + 16].copy_from_slice(&v.to_le_bytes());
            }
            disk.write(&p)?;
            ids.push(p.id)
        }
        let mut header = disk.allocate(PageType::Catalog)?;
        let payload = header.payload_mut();
        payload[0] = 0xB3;
        payload[1..3].copy_from_slice(&(self.order as u16).to_le_bytes());
        payload[3..11].copy_from_slice(&(entries.len() as u64).to_le_bytes());
        payload[11..13].copy_from_slice(&(ids.len() as u16).to_le_bytes());
        if 13 + ids.len() * 4 > payload.len() {
            return Err("index has too many leaf pages for one directory page".into());
        }
        for (i, id) in ids.iter().enumerate() {
            payload[13 + i * 4..17 + i * 4].copy_from_slice(&id.to_le_bytes())
        }
        disk.write(&header)?;
        disk.sync()?;
        Ok(header.id)
    }
    pub fn load_pages(disk: &mut PageManager, header_id: u32) -> Result<Self, String> {
        let header = disk.read(header_id)?;
        let p = header.payload();
        if p[0] != 0xB3 {
            return Err("page is not a B+ tree directory".into());
        }
        let order = u16::from_le_bytes(p[1..3].try_into().unwrap()) as usize;
        let count = u64::from_le_bytes(p[3..11].try_into().unwrap());
        let n = u16::from_le_bytes(p[11..13].try_into().unwrap()) as usize;
        let mut tree = Self::new(order)?;
        for i in 0..n {
            let off = 13 + i * 4;
            let id = u32::from_le_bytes(p[off..off + 4].try_into().unwrap());
            let leaf = disk.read(id)?;
            if leaf.kind != PageType::Index || leaf.payload()[0] != 1 {
                return Err(format!("invalid B+ tree leaf page {id}"));
            }
            let len = u16::from_le_bytes(leaf.payload()[1..3].try_into().unwrap()) as usize;
            for j in 0..len {
                let at = 3 + j * 16;
                let k = i64::from_le_bytes(leaf.payload()[at..at + 8].try_into().unwrap());
                let v = u64::from_le_bytes(leaf.payload()[at + 8..at + 16].try_into().unwrap());
                tree.insert(k, v);
            }
        }
        if tree.len as u64 != count {
            return Err("B+ tree directory count mismatch".into());
        }
        Ok(tree)
    }
}
const fn disk_page_payload() -> usize {
    4096 - 64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    #[test]
    fn split_search_scan_delete() {
        let mut t = BPlusTree::new(5).unwrap();
        for k in (0..100).rev() {
            t.insert(k, k as u64 * 2);
        }
        assert!(t.height() > 1);
        for k in 0..100 {
            assert_eq!(t.get(k), Some(k as u64 * 2));
        }
        assert_eq!(t.scan(30, 50).len(), 21);
        assert_eq!(t.remove(40), Some(80));
        assert_eq!(t.get(40), None);
        assert_eq!(t.len(), 99);
    }
    #[test]
    fn persisted_tree_reopens() {
        let p = std::env::temp_dir().join(format!(
            "chronodb-tree-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let header;
        {
            let mut d = PageManager::open(&p).unwrap();
            let mut t = BPlusTree::new(5).unwrap();
            for k in 0..500 {
                t.insert(k, k as u64 + 10);
            }
            header = t.save_pages(&mut d).unwrap();
        }
        {
            let mut d = PageManager::open(&p).unwrap();
            let t = BPlusTree::load_pages(&mut d, header).unwrap();
            assert_eq!(t.get(499), Some(509));
            assert_eq!(t.len(), 500);
        }
        let _ = std::fs::remove_file(p);
    }
}
