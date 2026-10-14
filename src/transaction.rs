//! Snapshot-isolation transaction metadata and write-write conflict checking.
use std::collections::BTreeMap;

pub type TxnId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxnState {
    Active,
    Committed,
    Aborted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteOp {
    Put(Vec<u8>),
    Delete,
}

#[derive(Clone, Debug)]
pub struct Transaction {
    pub id: TxnId,
    pub snapshot: u64,
    pub state: TxnState,
    pub writes: BTreeMap<Vec<u8>, WriteOp>,
}

impl Transaction {
    pub fn new(id: TxnId, snapshot: u64) -> Self {
        Self {
            id,
            snapshot,
            state: TxnState::Active,
            writes: BTreeMap::new(),
        }
    }

    pub fn put(&mut self, key: impl Into<Vec<u8>>, value: impl Into<Vec<u8>>) {
        self.writes.insert(key.into(), WriteOp::Put(value.into()));
    }

    pub fn delete(&mut self, key: impl Into<Vec<u8>>) {
        self.writes.insert(key.into(), WriteOp::Delete);
    }

    pub fn pending_writes(&self) -> usize {
        self.writes.len()
    }

    pub fn is_active(&self) -> bool {
        self.state == TxnState::Active
    }
}

#[derive(Default)]
pub struct TransactionManager {
    next_id: TxnId,
    last_commit: u64,
    active: BTreeMap<TxnId, Transaction>,
    last_write: BTreeMap<Vec<u8>, u64>,
}

impl TransactionManager {
    pub fn begin(&mut self) -> Transaction {
        self.next_id += 1;
        let tx = Transaction::new(self.next_id, self.last_commit);
        self.active.insert(tx.id, tx.clone());
        tx
    }

    pub fn read_timestamp(&self, tx: &Transaction) -> Result<u64, String> {
        if !self.active.contains_key(&tx.id) || !tx.is_active() {
            return Err("transaction is not active".into());
        }
        Ok(tx.snapshot)
    }

    pub fn commit(&mut self, tx: &mut Transaction) -> Result<u64, String> {
        if !self.active.contains_key(&tx.id) || !tx.is_active() {
            return Err("transaction is not active".into());
        }

        // Conflict detection: write-write conflicts against commits newer than snapshot
        for key in tx.writes.keys() {
            if self.last_write.get(key).copied().unwrap_or(0) > tx.snapshot {
                tx.state = TxnState::Aborted;
                self.active.remove(&tx.id);
                return Err("serialization failure: concurrent write conflict".into());
            }
        }

        self.last_commit += 1;
        let ts = self.last_commit;
        for key in tx.writes.keys() {
            self.last_write.insert(key.clone(), ts);
        }
        tx.state = TxnState::Committed;
        self.active.remove(&tx.id);
        Ok(ts)
    }

    pub fn rollback(&mut self, tx: &mut Transaction) -> Result<(), String> {
        if !self.active.contains_key(&tx.id) {
            return Err("transaction is not active".into());
        }
        tx.state = TxnState::Aborted;
        self.active.remove(&tx.id);
        Ok(())
    }

    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    pub fn last_commit(&self) -> u64 {
        self.last_commit
    }

    pub fn set_last_commit(&mut self, commit: u64) {
        if commit > self.last_commit {
            self.last_commit = commit;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflicting_snapshots_abort() {
        let mut m = TransactionManager::default();
        let mut a = m.begin();
        let mut b = m.begin();
        a.put(b"key".to_vec(), b"a".to_vec());
        b.put(b"key".to_vec(), b"b".to_vec());
        assert_eq!(m.commit(&mut a).unwrap(), 1);
        assert!(m.commit(&mut b).unwrap_err().contains("conflict"));
        assert_eq!(b.state, TxnState::Aborted);
        assert_eq!(m.active_count(), 0);
    }

    #[test]
    fn disjoint_writes_commit() {
        let mut m = TransactionManager::default();
        let mut a = m.begin();
        let mut b = m.begin();
        a.put(b"a".to_vec(), b"x".to_vec());
        b.put(b"b".to_vec(), b"y".to_vec());
        assert!(m.commit(&mut a).is_ok());
        assert!(m.commit(&mut b).is_ok());
        assert_eq!(a.state, TxnState::Committed);
        assert_eq!(b.state, TxnState::Committed);
    }

    #[test]
    fn rollback_cancels_transaction() {
        let mut m = TransactionManager::default();
        let mut a = m.begin();
        a.put(b"a".to_vec(), b"val".to_vec());
        assert!(m.rollback(&mut a).is_ok());
        assert_eq!(a.state, TxnState::Aborted);
        assert_eq!(m.active_count(), 0);
    }
}
