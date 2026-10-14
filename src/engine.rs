use crate::btree::BPlusTree;
use crate::executor::{
    compare_values, Filter as ExecFilter, Limit as ExecLimit, Offset as ExecOffset, Operator,
    Project as ExecProject, Row, Sort as ExecSort, Values as ExecValues,
};
use crate::parser::{
    is_valid_timestamp_str, parse_statements, ArithmeticOp, ColumnDef, ColumnType, Condition,
    Expression, Op, SelectColumn, Statement, Value,
};
use crate::transaction::{Transaction, TransactionManager, TxnId};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
struct Version {
    commit: u64,
    values: Option<Vec<Value>>,
}

#[derive(Clone, Debug)]
struct RowChain {
    versions: Vec<Version>,
}

#[derive(Clone, Debug)]
struct Table {
    columns: Vec<ColumnDef>,
    rows: BTreeMap<String, RowChain>,
    primary_index: Option<BPlusTree>,
    secondary_indexes: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
struct Branch {
    parent: Option<String>,
    base_commit: u64,
    tables: BTreeMap<String, Table>,
}

#[derive(Clone, Debug)]
struct History {
    commit: u64,
    branch: String,
    table: String,
    action: String,
}

#[derive(Clone, Debug)]
struct ConcurrentTxContext {
    tx: Transaction,
    branch_name: String,
    working_branch: Branch,
    staged_changes: Vec<(String, String, Option<Vec<Value>>)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub message: String,
}

impl QueryResult {
    pub fn message(message: impl Into<String>) -> Self {
        Self {
            columns: vec![],
            rows: vec![],
            message: message.into(),
        }
    }
}

impl std::fmt::Display for QueryResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.columns.is_empty() {
            writeln!(f, "{}", self.columns.join(" | "))?;
            writeln!(
                f,
                "{}",
                self.columns
                    .iter()
                    .map(|c| "-".repeat(c.len().max(3)))
                    .collect::<Vec<_>>()
                    .join("-+-")
            )?;
            for row in &self.rows {
                writeln!(f, "{}", row.join(" | "))?;
            }
        }
        write!(f, "{}", self.message)
    }
}

pub struct Database {
    branches: BTreeMap<String, Branch>,
    current: String,
    clock: u64,
    history: Vec<History>,
    wal_path: Option<PathBuf>,
    txn_mgr: TransactionManager,
    active_tx: Option<Transaction>,
    active_tx_branch: Option<String>,
    active_tx_snapshot_branch: Option<Branch>,
    concurrent_txs: BTreeMap<TxnId, ConcurrentTxContext>,
}

impl Database {
    pub fn memory() -> Self {
        let mut branches = BTreeMap::new();
        branches.insert(
            "main".into(),
            Branch {
                parent: None,
                base_commit: 0,
                tables: BTreeMap::new(),
            },
        );
        Self {
            branches,
            current: "main".into(),
            clock: 0,
            history: vec![],
            wal_path: None,
            txn_mgr: TransactionManager::default(),
            active_tx: None,
            active_tx_branch: None,
            active_tx_snapshot_branch: None,
            concurrent_txs: BTreeMap::new(),
        }
    }

    pub fn open(dir: impl AsRef<Path>) -> Result<Self, String> {
        let root = dir.as_ref().join(".chronodb");
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let wal = root.join("wal.log");
        let mut db = Self::memory();
        if wal.exists() {
            let log = fs::read_to_string(&wal).map_err(|e| e.to_string())?;
            for line in log.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if db.execute_internal(trimmed, false).is_err() {
                    break;
                }
            }
        }
        db.wal_path = Some(wal);
        let index_path = root.join("index.db");
        if index_path.exists() && db.restore_indexes(&index_path).is_err() {
            db.rebuild_indexes()?;
        }
        Ok(db)
    }

    pub fn execute(&mut self, sql: &str) -> Result<QueryResult, String> {
        self.execute_internal(sql, true)
    }

    /// Starts an isolated concurrent transaction and returns its transaction ID.
    pub fn begin_tx(&mut self) -> Result<TxnId, String> {
        self.txn_mgr.set_last_commit(self.clock);
        let mut tx = self.txn_mgr.begin();
        tx.snapshot = self.clock;
        let id = tx.id;
        let working_branch = self.branch()?.clone();
        self.concurrent_txs.insert(
            id,
            ConcurrentTxContext {
                tx,
                branch_name: self.current.clone(),
                working_branch,
                staged_changes: Vec::new(),
            },
        );
        Ok(id)
    }

    /// Executes a SQL statement within the isolated context of `tx_id`.
    pub fn execute_in_tx(&mut self, tx_id: TxnId, sql: &str) -> Result<QueryResult, String> {
        let mut ctx = self
            .concurrent_txs
            .remove(&tx_id)
            .ok_or_else(|| format!("transaction {tx_id} is not active"))?;

        let saved_current = self.current.clone();
        let saved_branch = self.branches.get(&ctx.branch_name).cloned();
        let saved_active_tx = self.active_tx.take();
        let saved_clock = self.clock;

        self.current = ctx.branch_name.clone();
        self.branches
            .insert(ctx.branch_name.clone(), ctx.working_branch.clone());
        // Within this isolated transaction, set clock to snapshot so reads only see <= snapshot
        // and any own writes at snapshot + 1.
        self.clock = ctx.tx.snapshot;
        self.active_tx = Some(ctx.tx.clone());

        let stmts = parse_statements(sql);
        let res = match stmts {
            Ok(list) => {
                let mut last = QueryResult::message("ok");
                let mut err = None;
                for stmt in list {
                    match self.execute_statement_with_staging(stmt, &mut ctx.staged_changes) {
                        Ok(r) => last = r,
                        Err(e) => {
                            err = Some(e);
                            break;
                        }
                    }
                }
                if let Some(e) = err {
                    Err(e)
                } else {
                    Ok(last)
                }
            }
            Err(e) => Err(e),
        };

        ctx.tx = self.active_tx.take().unwrap();
        ctx.working_branch = self.branches.get(&ctx.branch_name).unwrap().clone();

        // Restore shared database state
        if let Some(b) = saved_branch {
            self.branches.insert(ctx.branch_name.clone(), b);
        }
        self.current = saved_current;
        self.active_tx = saved_active_tx;
        self.clock = saved_clock;
        self.concurrent_txs.insert(tx_id, ctx);

        res
    }

    /// Commits a concurrent transaction, validating write-write conflicts against any intervening commits.
    pub fn commit_tx(&mut self, tx_id: TxnId) -> Result<QueryResult, String> {
        let mut ctx = self
            .concurrent_txs
            .remove(&tx_id)
            .ok_or_else(|| format!("transaction {tx_id} is not active"))?;
        self.txn_mgr.set_last_commit(self.clock);
        let commit_ts = self.txn_mgr.commit(&mut ctx.tx)?;
        self.clock = commit_ts;

        // Apply staged changes to shared branch at `commit_ts`
        let branch = self
            .branches
            .get_mut(&ctx.branch_name)
            .ok_or_else(|| format!("unknown branch '{}'", ctx.branch_name))?;

        for (table_name, row_key, values_opt) in ctx.staged_changes {
            if let Some(table) = branch.tables.get_mut(&table_name) {
                let pk_idx = table
                    .columns
                    .iter()
                    .position(|c| c.primary_key)
                    .unwrap_or(0);
                if let Some(vals) = &values_opt {
                    if let Some(Value::Number(n)) = vals.get(pk_idx) {
                        if let Some(idx) = table.primary_index.as_mut() {
                            idx.insert(*n, 1);
                        }
                    }
                } else if let Some(n) = row_key.strip_prefix('i').and_then(|s| s.parse::<i64>().ok())
                {
                    if let Some(idx) = table.primary_index.as_mut() {
                        idx.remove(n);
                    }
                }
                table
                    .rows
                    .entry(row_key)
                    .or_insert_with(|| RowChain { versions: vec![] })
                    .versions
                    .push(Version {
                        commit: commit_ts,
                        values: values_opt,
                    });
            }
        }

        self.history.push(History {
            commit: commit_ts,
            branch: ctx.branch_name,
            table: "__tx__".into(),
            action: format!("COMMIT tx {tx_id}"),
        });

        Ok(QueryResult::message(format!(
            "committed transaction {tx_id} at commit {commit_ts}"
        )))
    }

    /// Rolls back a concurrent transaction and discards its staged writes.
    pub fn rollback_tx(&mut self, tx_id: TxnId) -> Result<QueryResult, String> {
        let mut ctx = self
            .concurrent_txs
            .remove(&tx_id)
            .ok_or_else(|| format!("transaction {tx_id} is not active"))?;
        self.txn_mgr.rollback(&mut ctx.tx)?;
        Ok(QueryResult::message(format!(
            "rolled back transaction {tx_id}"
        )))
    }

    /// Garbage-collects obsolete versions strictly older than the version active at `retain_from`.
    pub fn gc(&mut self, retain_from: u64) -> usize {
        let mut removed = 0usize;
        for branch in self.branches.values_mut() {
            for table in branch.tables.values_mut() {
                for chain in table.rows.values_mut() {
                    if chain.versions.len() <= 1 {
                        continue;
                    }
                    // Find the index of the latest version with commit <= retain_from
                    let active_at_horizon = chain
                        .versions
                        .iter()
                        .rposition(|v| v.commit <= retain_from);
                    if let Some(keep_idx) = active_at_horizon {
                        if keep_idx > 0 {
                            removed += keep_idx;
                            chain.versions.drain(0..keep_idx);
                        }
                    }
                }
            }
        }
        removed
    }

    /// Returns the full chronological version chain `(commit_ts, Option<Row>)` for a primary key.
    pub fn version_history(
        &self,
        table: &str,
        pk: &Value,
    ) -> Result<Vec<(u64, Option<Vec<Value>>)>, String> {
        let t = self.table(table)?;
        let key = Self::value_key(pk);
        let Some(chain) = t.rows.get(&key) else {
            return Ok(vec![]);
        };
        Ok(chain
            .versions
            .iter()
            .map(|v| (v.commit, v.values.clone()))
            .collect())
    }

    fn execute_internal(&mut self, sql: &str, persist: bool) -> Result<QueryResult, String> {
        let statements = parse_statements(sql)?;
        let mut last_result = QueryResult::message("ok");
        for statement in statements {
            let mutates = !matches!(
                statement,
                Statement::Select { .. }
                    | Statement::Diff { .. }
                    | Statement::ShowHistory { .. }
                    | Statement::Begin
                    | Statement::Rollback
                    | Statement::Explain(_)
                    | Statement::Bisect { .. }
                    | Statement::WhatIf { .. }
            );
            let in_explicit_tx = self.active_tx.is_some();
            last_result = self.execute_statement(statement)?;

            if persist && mutates && !in_explicit_tx {
                if let Some(path) = &self.wal_path {
                    let mut file = OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                        .map_err(|e| e.to_string())?;
                    writeln!(file, "{}", sql.trim()).map_err(|e| e.to_string())?;
                    file.sync_data().map_err(|e| e.to_string())?;
                    if let Some(root) = path.parent() {
                        self.persist_indexes(&root.join("index.db"))?;
                    }
                }
            }
        }
        Ok(last_result)
    }

    fn persist_indexes(&self, path: &Path) -> Result<(), String> {
        let mut disk = crate::storage::PageManager::create(path)?;
        let mut manifest = disk.allocate(crate::storage::PageType::Catalog)?;
        let mut entries = Vec::new();
        for (branch_name, branch) in &self.branches {
            for (table_name, table) in &branch.tables {
                if let Some(index) = &table.primary_index {
                    let root = index.save_pages(&mut disk)?;
                    entries.push((branch_name, table_name, root));
                }
            }
        }
        let payload = manifest.payload_mut();
        payload[0] = 0xDB;
        payload[1..3].copy_from_slice(&(entries.len() as u16).to_le_bytes());
        let mut pos = 3usize;
        for (branch, table, root) in entries {
            let branch = branch.as_bytes();
            let table = table.as_bytes();
            if branch.len() > u8::MAX as usize
                || table.len() > u8::MAX as usize
                || pos + 2 + branch.len() + table.len() + 4 > payload.len()
            {
                return Err("index catalog page is full".into());
            }
            payload[pos] = branch.len() as u8;
            pos += 1;
            payload[pos..pos + branch.len()].copy_from_slice(branch);
            pos += branch.len();
            payload[pos] = table.len() as u8;
            pos += 1;
            payload[pos..pos + table.len()].copy_from_slice(table);
            pos += table.len();
            payload[pos..pos + 4].copy_from_slice(&root.to_le_bytes());
            pos += 4;
        }
        disk.write(&manifest)?;
        disk.sync()
    }

    fn restore_indexes(&mut self, path: &Path) -> Result<(), String> {
        let mut disk = crate::storage::PageManager::open(path)?;
        if disk.page_count() == 0 {
            return Ok(());
        }
        let manifest = disk.read(0)?;
        let payload = manifest.payload();
        if payload[0] != 0xDB {
            return Ok(());
        }
        let count = u16::from_le_bytes(payload[1..3].try_into().unwrap()) as usize;
        let mut pos = 3usize;
        for _ in 0..count {
            let branch_len = *payload.get(pos).ok_or("truncated index manifest")? as usize;
            pos += 1;
            let branch = std::str::from_utf8(
                payload
                    .get(pos..pos + branch_len)
                    .ok_or("truncated index manifest")?,
            )
            .map_err(|e| e.to_string())?
            .to_owned();
            pos += branch_len;
            let table_len = *payload.get(pos).ok_or("truncated index manifest")? as usize;
            pos += 1;
            let table = std::str::from_utf8(
                payload
                    .get(pos..pos + table_len)
                    .ok_or("truncated index manifest")?,
            )
            .map_err(|e| e.to_string())?
            .to_owned();
            pos += table_len;
            let root = u32::from_le_bytes(
                payload
                    .get(pos..pos + 4)
                    .ok_or("truncated index manifest")?
                    .try_into()
                    .unwrap(),
            );
            pos += 4;
            let tree = BPlusTree::load_pages(&mut disk, root)?;
            if let Some(table) = self
                .branches
                .get_mut(&branch)
                .and_then(|b| b.tables.get_mut(&table))
            {
                table.primary_index = Some(tree);
            }
        }
        Ok(())
    }

    fn rebuild_indexes(&mut self) -> Result<(), String> {
        for branch in self.branches.values_mut() {
            for table in branch.tables.values_mut() {
                if table.primary_index.is_none() {
                    continue;
                }
                let mut index = BPlusTree::new(64)?;
                let pk = table
                    .columns
                    .iter()
                    .position(|c| c.primary_key)
                    .ok_or("table is missing its primary key")?;
                for chain in table.rows.values() {
                    if let Some(values) = Self::visible(chain, self.clock) {
                        if let Value::Number(key) = &values[pk] {
                            index.insert(*key, 1);
                        }
                    }
                }
                table.primary_index = Some(index);
            }
        }
        Ok(())
    }

    fn commit(&mut self, table: &str, action: &str) -> u64 {
        self.clock += 1;
        self.txn_mgr.set_last_commit(self.clock);
        self.history.push(History {
            commit: self.clock,
            branch: self.current.clone(),
            table: table.into(),
            action: action.into(),
        });
        self.clock
    }

    fn branch(&self) -> Result<&Branch, String> {
        self.branches
            .get(&self.current)
            .ok_or_else(|| "current branch missing".into())
    }

    fn branch_mut(&mut self) -> Result<&mut Branch, String> {
        self.branches
            .get_mut(&self.current)
            .ok_or_else(|| "current branch missing".into())
    }

    fn execute_statement(&mut self, s: Statement) -> Result<QueryResult, String> {
        let mut dummy = Vec::new();
        self.execute_statement_with_staging(s, &mut dummy)
    }

    fn execute_statement_with_staging(
        &mut self,
        s: Statement,
        staged: &mut Vec<(String, String, Option<Vec<Value>>)>,
    ) -> Result<QueryResult, String> {
        match s {
            Statement::Explain(inner) => {
                let plan_desc = match *inner {
                    Statement::Select {
                        ref table,
                        ref filter,
                        ..
                    } => {
                        let t = self.table(table)?;
                        let has_idx = t.primary_index.is_some();
                        if crate::planner::primary_index_key(filter.as_ref(), &t.columns, has_idx)
                            .is_some()
                        {
                            format!("IndexScan on {table}")
                        } else if crate::planner::primary_index_range(
                            filter.as_ref(),
                            &t.columns,
                            has_idx,
                        )
                        .is_some()
                        {
                            format!("IndexRangeScan on {table}")
                        } else {
                            format!("TableScan on {table}")
                        }
                    }
                    other => format!("Execute {other:?}"),
                };
                Ok(QueryResult {
                    columns: vec!["plan".into()],
                    rows: vec![vec![plan_desc.clone()]],
                    message: plan_desc,
                })
            }
            Statement::Begin => {
                if self.active_tx.is_some() {
                    return Err("nested transactions are not supported".into());
                }
                self.txn_mgr.set_last_commit(self.clock);
                let mut tx = self.txn_mgr.begin();
                tx.snapshot = self.clock;
                self.active_tx = Some(tx);
                self.active_tx_branch = Some(self.current.clone());
                self.active_tx_snapshot_branch = Some(self.branch()?.clone());
                Ok(QueryResult::message("transaction started"))
            }
            Statement::Commit => {
                let Some(mut tx) = self.active_tx.take() else {
                    return Err("no active transaction to commit".into());
                };
                let branch_name = self
                    .active_tx_branch
                    .take()
                    .unwrap_or_else(|| self.current.clone());
                let snapshot_backup = self.active_tx_snapshot_branch.take();
                match self.txn_mgr.commit(&mut tx) {
                    Ok(commit_ts) => {
                        self.clock = commit_ts;
                        self.history.push(History {
                            commit: commit_ts,
                            branch: branch_name,
                            table: "__tx__".into(),
                            action: format!("COMMIT tx {}", tx.id),
                        });
                        if let Some(path) = &self.wal_path {
                            let mut file = OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(path)
                                .map_err(|e| e.to_string())?;
                            writeln!(file, "COMMIT").map_err(|e| e.to_string())?;
                            file.sync_data().map_err(|e| e.to_string())?;
                            if let Some(root) = path.parent() {
                                self.persist_indexes(&root.join("index.db"))?;
                            }
                        }
                        Ok(QueryResult::message(format!(
                            "committed transaction at commit {commit_ts}"
                        )))
                    }
                    Err(e) => {
                        if let Some(backup) = snapshot_backup {
                            self.branches.insert(branch_name, backup);
                        }
                        Err(e)
                    }
                }
            }
            Statement::Rollback => {
                let Some(mut tx) = self.active_tx.take() else {
                    return Err("no active transaction to rollback".into());
                };
                let branch_name = self
                    .active_tx_branch
                    .take()
                    .unwrap_or_else(|| self.current.clone());
                if let Some(backup) = self.active_tx_snapshot_branch.take() {
                    self.branches.insert(branch_name, backup);
                }
                let _ = self.txn_mgr.rollback(&mut tx);
                Ok(QueryResult::message("rolled back transaction"))
            }
            Statement::Checkpoint => {
                let commit = self.clock;
                if let Some(path) = &self.wal_path {
                    let mut file = OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                        .map_err(|e| e.to_string())?;
                    writeln!(file, "CHECKPOINT").map_err(|e| e.to_string())?;
                    file.sync_data().map_err(|e| e.to_string())?;
                    if let Some(root) = path.parent() {
                        self.persist_indexes(&root.join("index.db"))?;
                    }
                }
                Ok(QueryResult::message(format!(
                    "checkpoint created at commit {commit}"
                )))
            }
            Statement::Vacuum { retain_from } => {
                let horizon = if retain_from == 0 {
                    self.clock
                } else {
                    retain_from
                };
                let removed = self.gc(horizon);
                Ok(QueryResult::message(format!(
                    "garbage collected {removed} obsolete version(s) before commit {horizon}"
                )))
            }
            Statement::CreateTable { name, columns } => {
                if columns.is_empty() {
                    return Err("table needs at least one column".into());
                }
                if columns.iter().filter(|c| c.primary_key).count() > 1 {
                    return Err("at most one PRIMARY KEY column is supported".into());
                }
                let mut columns = columns;
                if !columns.iter().any(|c| c.primary_key) {
                    columns[0].primary_key = true;
                }
                if self.branch()?.tables.contains_key(&name) {
                    return Err(format!("table '{name}' already exists"));
                }
                self.branch_mut()?.tables.insert(
                    name.clone(),
                    Table {
                        primary_index: if columns
                            .iter()
                            .find(|c| c.primary_key)
                            .is_some_and(|c| c.data_type == ColumnType::Integer)
                        {
                            Some(BPlusTree::new(64)?)
                        } else {
                            None
                        },
                        columns,
                        rows: BTreeMap::new(),
                        secondary_indexes: BTreeMap::new(),
                    },
                );
                let c = self.commit(&name, "CREATE TABLE");
                Ok(QueryResult::message(format!(
                    "created table {name} at commit {c}"
                )))
            }
            Statement::CreateIndex {
                name,
                table,
                column,
            } => {
                let t = self.table(&table)?;
                if !t.columns.iter().any(|c| c.name == column) {
                    return Err(format!("unknown column '{column}' in table '{table}'"));
                }
                self.branch_mut()?
                    .tables
                    .get_mut(&table)
                    .unwrap()
                    .secondary_indexes
                    .insert(name.clone(), column.clone());
                let c = self.commit(&table, &format!("CREATE INDEX {name}"));
                Ok(QueryResult::message(format!(
                    "created index {name} on {table}({column}) at commit {c}"
                )))
            }
            Statement::Insert {
                table,
                columns,
                values,
                additional_rows,
            } => {
                let branch_name = self.current.clone();
                let branch_backup = self.branch()?.clone();
                let clock_backup = self.clock;
                let history_len = self.history.len();
                let staged_len = staged.len();
                let result = (|| {
                    let mut result = self.insert(&table, columns.clone(), values, staged)?;
                    for row in additional_rows {
                        result = self.insert(&table, columns.clone(), row, staged)?;
                    }
                    Ok(result)
                })();
                if result.is_err() {
                    self.branches.insert(branch_name, branch_backup);
                    self.clock = clock_backup;
                    self.history.truncate(history_len);
                    staged.truncate(staged_len);
                }
                result
            }
            Statement::Update {
                table,
                assignments,
                filter,
            } => self.update(&table, assignments, filter, staged),
            Statement::Delete { table, filter } => self.delete(&table, filter, staged),
            Statement::Select {
                columns,
                table,
                as_of,
                filter,
                order_by,
                limit,
                offset,
            } => self.select(&table, columns, as_of, filter, order_by, limit, offset),
            Statement::CreateBranch { name, from } => {
                if self.branches.contains_key(&name) {
                    return Err(format!("branch '{name}' already exists"));
                }
                let parent = self
                    .branches
                    .get(&from)
                    .ok_or_else(|| format!("unknown branch '{from}'"))?
                    .clone();
                self.branches.insert(
                    name.clone(),
                    Branch {
                        parent: Some(from.clone()),
                        base_commit: self.clock,
                        tables: parent.tables,
                    },
                );
                let c = self.commit("__branch__", &format!("CREATE BRANCH {name} FROM {from}"));
                Ok(QueryResult::message(format!(
                    "created branch {name} from {from} at commit {c}"
                )))
            }
            Statement::Checkout { name } => {
                if !self.branches.contains_key(&name) {
                    return Err(format!("unknown branch '{name}'"));
                }
                self.current = name.clone();
                Ok(QueryResult::message(format!("checked out {name}")))
            }
            Statement::Diff { left, right } => self.diff(&left, &right),
            Statement::Merge { source, target } => self.merge(&source, &target),
            Statement::ShowHistory { table, filter } => self.show_history(&table, filter),
            Statement::Bisect { table, filter } => self.bisect(&table, &filter),
            Statement::WhatIf { mutation, query } => self.what_if(*mutation, *query),
        }
    }

    fn table(&self, name: &str) -> Result<&Table, String> {
        self.branch()?
            .tables
            .get(name)
            .ok_or_else(|| format!("unknown table '{name}'"))
    }

    fn visible(chain: &RowChain, as_of: u64) -> Option<&Vec<Value>> {
        chain
            .versions
            .iter()
            .rev()
            .find(|v| v.commit <= as_of)
            .and_then(|v| v.values.as_ref())
    }

    fn value_key(v: &Value) -> String {
        match v {
            Value::Null => "n".into(),
            Value::Bool(v) => format!("b{v}"),
            Value::Number(v) => format!("i{v}"),
            Value::Float(v) => format!("f{v}"),
            Value::Text(v) => format!("s{v}"),
        }
    }

    fn validate(table: &Table, values: &[Value]) -> Result<(), String> {
        for (i, col) in table.columns.iter().enumerate() {
            let v = &values[i];
            if col.not_null && matches!(v, Value::Null) {
                return Err(format!("{} cannot be NULL", col.name));
            }
            match (&col.data_type, v) {
                (_, Value::Null)
                | (ColumnType::Integer, Value::Number(_))
                | (ColumnType::Timestamp, Value::Number(_))
                | (ColumnType::Float, Value::Number(_))
                | (ColumnType::Float, Value::Float(_))
                | (ColumnType::String, Value::Text(_))
                | (ColumnType::Boolean, Value::Bool(_)) => (),
                (ColumnType::Timestamp, Value::Text(s)) if is_valid_timestamp_str(s) => (),
                _ => return Err(format!("invalid value for {}", col.name)),
            }
        }
        Ok(())
    }

    fn insert(
        &mut self,
        name: &str,
        cols: Vec<String>,
        vals: Vec<Value>,
        staged: &mut Vec<(String, String, Option<Vec<Value>>)>,
    ) -> Result<QueryResult, String> {
        let (mut values, pk, numeric_pk) = {
            let t = self.table(name)?;
            let cols = if cols.is_empty() {
                t.columns.iter().map(|c| c.name.clone()).collect::<Vec<_>>()
            } else {
                cols
            };
            if cols.len() != vals.len() {
                return Err("column/value count mismatch".into());
            }
            let mut out = vec![Value::Null; t.columns.len()];
            for (c, v) in cols.iter().zip(vals) {
                let i = t
                    .columns
                    .iter()
                    .position(|x| x.name == *c)
                    .ok_or_else(|| format!("unknown column '{c}'"))?;
                out[i] = v;
            }
            Self::validate(t, &out)?;
            let i = t.columns.iter().position(|x| x.primary_key).unwrap();
            let key = Self::value_key(&out[i]);
            let numeric = match out[i] {
                Value::Number(n) => Some(n),
                _ => None,
            };
            (out, key, numeric)
        };

        let current_visibility = if let Some(tx) = &self.active_tx {
            tx.snapshot + 1
        } else {
            self.clock
        };

        if self
            .table(name)?
            .rows
            .get(&pk)
            .and_then(|r| Self::visible(r, current_visibility))
            .is_some()
        {
            return Err("duplicate primary key".into());
        }

        let commit = if self.active_tx.is_some() {
            self.active_tx
                .as_mut()
                .unwrap()
                .put(format!("{name}:{pk}"), b"inserted".to_vec());
            staged.push((name.to_string(), pk.clone(), Some(values.clone())));
            self.clock + 1
        } else {
            self.commit(name, "INSERT")
        };

        self.branch_mut()?
            .tables
            .get_mut(name)
            .unwrap()
            .rows
            .entry(pk)
            .or_insert(RowChain { versions: vec![] })
            .versions
            .push(Version {
                commit,
                values: Some(std::mem::take(&mut values)),
            });

        if let Some(n) = numeric_pk {
            if let Some(index) = self
                .branch_mut()?
                .tables
                .get_mut(name)
                .unwrap()
                .primary_index
                .as_mut()
            {
                index.insert(n, 1);
            }
        }
        Ok(QueryResult::message(format!(
            "inserted 1 row at commit {commit}"
        )))
    }

    fn update(
        &mut self,
        name: &str,
        assignments: Vec<(String, Value)>,
        filter: Option<Condition>,
        staged: &mut Vec<(String, String, Option<Vec<Value>>)>,
    ) -> Result<QueryResult, String> {
        let snapshot = if let Some(tx) = &self.active_tx {
            tx.snapshot + 1
        } else {
            self.clock
        };
        let matches = self.matching(name, snapshot, filter.as_ref())?;
        if matches.is_empty() {
            return Ok(QueryResult::message("updated 0 rows"));
        }
        let t = self.table(name)?.clone();
        let mut edits = vec![];
        for key in matches {
            let mut data = Self::visible(t.rows.get(&key).unwrap(), snapshot)
                .unwrap()
                .clone();
            for (c, v) in &assignments {
                let i = t
                    .columns
                    .iter()
                    .position(|x| x.name == *c)
                    .ok_or_else(|| format!("unknown column '{c}'"))?;
                if t.columns[i].primary_key {
                    return Err("updating a primary key is not supported".into());
                }
                data[i] = v.clone();
            }
            Self::validate(&t, &data)?;
            edits.push((key, data));
        }

        let commit = if self.active_tx.is_some() {
            for (key, data) in &edits {
                self.active_tx
                    .as_mut()
                    .unwrap()
                    .put(format!("{name}:{key}"), b"updated".to_vec());
                staged.push((name.to_string(), key.clone(), Some(data.clone())));
            }
            self.clock + 1
        } else {
            self.commit(name, "UPDATE")
        };

        let live = self.branch_mut()?.tables.get_mut(name).unwrap();
        for (key, data) in edits {
            live.rows.get_mut(&key).unwrap().versions.push(Version {
                commit,
                values: Some(data),
            });
        }
        Ok(QueryResult::message(format!(
            "updated rows at commit {commit}"
        )))
    }

    fn delete(
        &mut self,
        name: &str,
        filter: Option<Condition>,
        staged: &mut Vec<(String, String, Option<Vec<Value>>)>,
    ) -> Result<QueryResult, String> {
        let snapshot = if let Some(tx) = &self.active_tx {
            tx.snapshot + 1
        } else {
            self.clock
        };
        let keys = self.matching(name, snapshot, filter.as_ref())?;
        if keys.is_empty() {
            return Ok(QueryResult::message("deleted 0 rows"));
        }

        let commit = if self.active_tx.is_some() {
            for key in &keys {
                self.active_tx
                    .as_mut()
                    .unwrap()
                    .delete(format!("{name}:{key}"));
                staged.push((name.to_string(), key.clone(), None));
            }
            self.clock + 1
        } else {
            self.commit(name, "DELETE")
        };

        let count = keys.len();
        let t = self.branch_mut()?.tables.get_mut(name).unwrap();
        for key in keys {
            if let Some(n) = key.strip_prefix('i').and_then(|s| s.parse::<i64>().ok()) {
                if let Some(index) = t.primary_index.as_mut() {
                    index.remove(n);
                }
            }
            t.rows.get_mut(&key).unwrap().versions.push(Version {
                commit,
                values: None,
            });
        }
        Ok(QueryResult::message(format!(
            "deleted {count} rows at commit {commit}"
        )))
    }

    fn matching(
        &self,
        name: &str,
        as_of: u64,
        filter: Option<&Condition>,
    ) -> Result<Vec<String>, String> {
        let t = self.table(name)?;
        if let Some(Condition::Compare(col, Op::Eq, Value::Number(key))) = filter {
            if t.columns
                .iter()
                .position(|c| c.name == *col)
                .is_some_and(|i| t.columns[i].primary_key)
            {
                if t.primary_index
                    .as_ref()
                    .is_some_and(|idx| idx.get(*key).is_some())
                {
                    let row_key = Self::value_key(&Value::Number(*key));
                    if let Some(chain) = t.rows.get(&row_key) {
                        if let Some(row) = Self::visible(chain, as_of) {
                            if filter.is_none_or(|f| Self::matches(t, row, f)) {
                                return Ok(vec![row_key]);
                            }
                        }
                    }
                }
                return Ok(vec![]);
            }
        }
        let mut out = vec![];
        for (key, chain) in &t.rows {
            if let Some(row) = Self::visible(chain, as_of) {
                if filter.map(|f| Self::matches(t, row, f)).unwrap_or(true) {
                    out.push(key.clone());
                }
            }
        }
        Ok(out)
    }

    fn matches(t: &Table, row: &[Value], c: &Condition) -> bool {
        match c {
            Condition::And(a, b) => Self::matches(t, row, a) && Self::matches(t, row, b),
            Condition::Or(a, b) => Self::matches(t, row, a) || Self::matches(t, row, b),
            Condition::Not(inner) => !Self::matches(t, row, inner),
            Condition::CompareExpr(left, op, right) => {
                let Some(a) = Self::eval_expression(t, row, left) else {
                    return false;
                };
                let Some(b) = Self::eval_expression(t, row, right) else {
                    return false;
                };
                if matches!(a, Value::Null) || matches!(b, Value::Null) {
                    return false;
                }
                compare_values(&a, &b).is_some_and(|ordering| match op {
                    Op::Eq => ordering == std::cmp::Ordering::Equal,
                    Op::Ne => ordering != std::cmp::Ordering::Equal,
                    Op::Lt => ordering == std::cmp::Ordering::Less,
                    Op::Gt => ordering == std::cmp::Ordering::Greater,
                    Op::Le => ordering != std::cmp::Ordering::Greater,
                    Op::Ge => ordering != std::cmp::Ordering::Less,
                    Op::Like => false,
                })
            }
            Condition::In(col, values) => t
                .columns
                .iter()
                .position(|x| x.name == *col)
                .and_then(|i| row.get(i))
                .is_some_and(|left| !matches!(left, Value::Null) && values.contains(left)),
            Condition::Between(col, low, high) => t
                .columns
                .iter()
                .position(|x| x.name == *col)
                .and_then(|i| row.get(i))
                .is_some_and(|v| {
                    !matches!(v, Value::Null)
                        && compare_values(v, low).is_some_and(|o| o != std::cmp::Ordering::Less)
                        && compare_values(v, high)
                            .is_some_and(|o| o != std::cmp::Ordering::Greater)
                }),
            Condition::IsNull(col, negated) => t
                .columns
                .iter()
                .position(|x| x.name == *col)
                .and_then(|i| row.get(i))
                .is_some_and(|v| matches!(v, Value::Null) != *negated),
            Condition::Compare(col, op, right) => {
                let Some(i) = t.columns.iter().position(|x| x.name == *col) else {
                    return false;
                };
                let left = &row[i];
                if matches!(left, Value::Null) || matches!(right, Value::Null) {
                    return false;
                }
                match op {
                    Op::Eq => compare_values(left, right) == Some(std::cmp::Ordering::Equal),
                    Op::Ne => compare_values(left, right)
                        .is_some_and(|o| o != std::cmp::Ordering::Equal),
                    Op::Like => {
                        matches!((left, right), (Value::Text(a), Value::Text(b)) if Self::like_matches(a, b))
                    }
                    _ => compare_values(left, right).is_some_and(|order| match op {
                        Op::Lt => order == std::cmp::Ordering::Less,
                        Op::Gt => order == std::cmp::Ordering::Greater,
                        Op::Le => order != std::cmp::Ordering::Greater,
                        Op::Ge => order != std::cmp::Ordering::Less,
                        _ => false,
                    }),
                }
            }
        }
    }

    fn like_matches(value: &str, pattern: &str) -> bool {
        let v: Vec<char> = value.chars().collect();
        let p: Vec<char> = pattern.chars().collect();
        let mut dp = vec![vec![false; p.len() + 1]; v.len() + 1];
        dp[0][0] = true;
        for j in 1..=p.len() {
            if p[j - 1] == '%' {
                dp[0][j] = dp[0][j - 1];
            }
        }
        for i in 1..=v.len() {
            for j in 1..=p.len() {
                dp[i][j] = match p[j - 1] {
                    '%' => dp[i][j - 1] || dp[i - 1][j],
                    '_' => dp[i - 1][j - 1],
                    c => c == v[i - 1] && dp[i - 1][j - 1],
                };
            }
        }
        dp[v.len()][p.len()]
    }

    fn eval_expression(t: &Table, row: &[Value], expr: &Expression) -> Option<Value> {
        match expr {
            Expression::Literal(v) => Some(v.clone()),
            Expression::Column(name) => t
                .columns
                .iter()
                .position(|c| c.name == *name)
                .and_then(|i| row.get(i))
                .cloned(),
            Expression::Binary(left, op, right) => {
                let a = Self::eval_expression(t, row, left)?;
                let b = Self::eval_expression(t, row, right)?;
                if let (Value::Number(x), Value::Number(y)) = (&a, &b) {
                    return match op {
                        ArithmeticOp::Add => x.checked_add(*y).map(Value::Number),
                        ArithmeticOp::Subtract => x.checked_sub(*y).map(Value::Number),
                        ArithmeticOp::Multiply => x.checked_mul(*y).map(Value::Number),
                        ArithmeticOp::Divide => {
                            if *y == 0 {
                                None
                            } else {
                                Some(Value::Float(*x as f64 / *y as f64))
                            }
                        }
                        ArithmeticOp::Modulo => x.checked_rem(*y).map(Value::Number),
                    };
                }
                let to_f64 = |v: &Value| match v {
                    Value::Number(n) => Some(*n as f64),
                    Value::Float(n) => Some(*n),
                    _ => None,
                };
                let x = to_f64(&a)?;
                let y = to_f64(&b)?;
                if matches!(op, ArithmeticOp::Divide | ArithmeticOp::Modulo) && y == 0.0 {
                    return None;
                }
                let n = match op {
                    ArithmeticOp::Add => x + y,
                    ArithmeticOp::Subtract => x - y,
                    ArithmeticOp::Multiply => x * y,
                    ArithmeticOp::Divide => x / y,
                    ArithmeticOp::Modulo => x % y,
                };
                Some(Value::Float(n))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn select(
        &self,
        name: &str,
        columns: Vec<SelectColumn>,
        as_of: Option<u64>,
        filter: Option<Condition>,
        order: Vec<(String, bool)>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<QueryResult, String> {
        let current_max = if let Some(tx) = &self.active_tx {
            tx.snapshot + 1
        } else {
            self.clock
        };
        let at = as_of.unwrap_or(current_max);
        if at > current_max {
            return Err(format!("commit {at} is in the future"));
        }
        let t = self.table(name)?;

        let is_count_star = columns.len() == 1 && matches!(columns[0], SelectColumn::CountStar);

        // Prepare projected column names and evaluators
        let (output_col_names, evaluators): (
            Vec<String>,
            Vec<Box<dyn Fn(&Row) -> Result<Value, String>>>,
        ) = if is_count_star {
            (vec!["COUNT(*)".into()], vec![])
        } else {
            let mut names = Vec::new();
            let mut evals: Vec<Box<dyn Fn(&Row) -> Result<Value, String>>> = Vec::new();

            for col in columns {
                match col {
                    SelectColumn::Wildcard => {
                        for (idx, c) in t.columns.iter().enumerate() {
                            names.push(c.name.clone());
                            evals.push(Box::new(move |r: &Row| {
                                r.get(idx)
                                    .cloned()
                                    .ok_or_else(|| "column index out of bounds".into())
                            }));
                        }
                    }
                    SelectColumn::CountStar => {
                        names.push("COUNT(*)".into());
                        evals.push(Box::new(|_: &Row| Ok(Value::Number(1))));
                    }
                    SelectColumn::Column(cname) => {
                        let idx = t
                            .columns
                            .iter()
                            .position(|c| c.name == cname)
                            .ok_or_else(|| format!("unknown column '{cname}'"))?;
                        names.push(cname);
                        evals.push(Box::new(move |r: &Row| {
                            r.get(idx)
                                .cloned()
                                .ok_or_else(|| "column index out of bounds".into())
                        }));
                    }
                    SelectColumn::AliasedExpr(expr, alias) => {
                        names.push(alias);
                        let schema = t.clone();
                        evals.push(Box::new(move |r: &Row| {
                            Ok(Self::eval_expression(&schema, r, &expr).unwrap_or(Value::Null))
                        }));
                    }
                    SelectColumn::Expr(expr) => {
                        names.push(expr.to_string());
                        let schema = t.clone();
                        evals.push(Box::new(move |r: &Row| {
                            Ok(Self::eval_expression(&schema, r, &expr).unwrap_or(Value::Null))
                        }));
                    }
                }
            }
            (names, evals)
        };

        let indexed = if at == self.clock {
            crate::planner::primary_index_key(
                filter.as_ref(),
                &t.columns,
                t.primary_index.is_some(),
            )
        } else {
            None
        };

        let raw = if indexed.is_some() {
            self.matching(name, at, filter.as_ref())?
                .into_iter()
                .filter_map(|k| t.rows.get(&k))
                .filter_map(|chain| Self::visible(chain, at))
                .cloned()
                .collect::<Vec<_>>()
        } else {
            t.rows
                .values()
                .filter_map(|c| Self::visible(c, at))
                .cloned()
                .collect::<Vec<_>>()
        };

        let mut plan: Box<dyn Operator> = Box::new(ExecValues::new(raw));
        if let Some(condition) = filter {
            let schema = t.clone();
            plan = Box::new(ExecFilter::new(plan, move |row| {
                Self::matches(&schema, row, &condition)
            }));
        }

        let mut sort_keys = Vec::new();
        for (column, desc) in order {
            let i = t
                .columns
                .iter()
                .position(|c| c.name == column)
                .ok_or_else(|| format!("unknown column '{column}'"))?;
            sort_keys.push((i, desc));
        }
        if !sort_keys.is_empty() {
            plan = Box::new(ExecSort::new_multi(plan, sort_keys));
        }

        if let Some(n) = offset {
            plan = Box::new(ExecOffset::new(plan, n));
        }
        if let Some(n) = limit {
            plan = Box::new(ExecLimit::new(plan, n));
        }

        if is_count_star {
            plan.open()?;
            let mut count = 0usize;
            while plan.next()?.is_some() {
                count += 1;
            }
            plan.close()?;
            return Ok(QueryResult {
                columns: output_col_names,
                rows: vec![vec![count.to_string()]],
                message: format!("1 row(s) at commit {at} on branch {}", self.current),
            });
        }

        plan = Box::new(ExecProject::with_evaluators(plan, evaluators));
        plan.open()?;
        let mut result_rows = Vec::new();
        while let Some(row) = plan.next()? {
            result_rows.push(row);
        }
        plan.close()?;
        let count = result_rows.len();
        Ok(QueryResult {
            columns: output_col_names,
            rows: result_rows
                .into_iter()
                .map(|r| r.iter().map(ToString::to_string).collect())
                .collect(),
            message: format!("{count} row(s) at commit {at} on branch {}", self.current),
        })
    }

    fn diff(&self, left: &str, right: &str) -> Result<QueryResult, String> {
        let a = self
            .branches
            .get(left)
            .ok_or_else(|| format!("unknown branch '{left}'"))?;
        let b = self
            .branches
            .get(right)
            .ok_or_else(|| format!("unknown branch '{right}'"))?;
        let mut rows = vec![];
        for name in a
            .tables
            .keys()
            .chain(b.tables.keys())
            .collect::<BTreeSet<_>>()
        {
            let at = self.clock;
            let ta = a.tables.get(name);
            let tb = b.tables.get(name);
            let keys = ta
                .into_iter()
                .flat_map(|t| t.rows.keys())
                .chain(tb.into_iter().flat_map(|t| t.rows.keys()))
                .collect::<BTreeSet<_>>();
            for key in keys {
                let av = ta
                    .and_then(|t| t.rows.get(key))
                    .and_then(|r| Self::visible(r, at));
                let bv = tb
                    .and_then(|t| t.rows.get(key))
                    .and_then(|r| Self::visible(r, at));
                if av != bv {
                    rows.push(vec![
                        (*name).clone(),
                        key.clone(),
                        av.map(|r| {
                            r.iter()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                                .join(",")
                        })
                        .unwrap_or_else(|| "<absent>".into()),
                        bv.map(|r| {
                            r.iter()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                                .join(",")
                        })
                        .unwrap_or_else(|| "<absent>".into()),
                    ]);
                }
            }
        }
        let count = rows.len();
        Ok(QueryResult {
            columns: vec!["table".into(), "key".into(), left.into(), right.into()],
            rows,
            message: format!("{count} changed row(s)"),
        })
    }

    fn find_common_ancestor(&self, source: &str, target: &str) -> Result<(String, u64), String> {
        let mut ancestors_src = Vec::new();
        let mut curr = Some(source.to_string());
        while let Some(name) = curr {
            ancestors_src.push(name.clone());
            curr = self.branches.get(&name).and_then(|b| b.parent.clone());
        }

        let mut ancestors_tgt = Vec::new();
        let mut curr = Some(target.to_string());
        while let Some(name) = curr {
            ancestors_tgt.push(name.clone());
            curr = self.branches.get(&name).and_then(|b| b.parent.clone());
        }

        for anc in &ancestors_src {
            if ancestors_tgt.contains(anc) {
                let base = if anc == target {
                    self.branches.get(source).map(|b| b.base_commit).unwrap_or(0)
                } else if anc == source {
                    self.branches.get(target).map(|b| b.base_commit).unwrap_or(0)
                } else {
                    let s_base = self.branches.get(source).map(|b| b.base_commit).unwrap_or(0);
                    let t_base = self.branches.get(target).map(|b| b.base_commit).unwrap_or(0);
                    s_base.min(t_base)
                };
                return Ok((anc.clone(), base));
            }
        }
        Err(format!(
            "no common ancestor between '{source}' and '{target}'"
        ))
    }

    fn merge(&mut self, source: &str, target: &str) -> Result<QueryResult, String> {
        if !self.branches.contains_key(source) {
            return Err(format!("unknown branch '{source}'"));
        }
        if !self.branches.contains_key(target) {
            return Err(format!("unknown branch '{target}'"));
        }
        let (ancestor_name, base_commit) = self.find_common_ancestor(source, target)?;
        let src = self.branches.get(source).unwrap().clone();
        let dst = self.branches.get(target).unwrap().clone();
        let anc = self
            .branches
            .get(&ancestor_name)
            .ok_or_else(|| format!("unknown ancestor '{ancestor_name}'"))?
            .clone();

        let mut changes = Vec::new();
        let table_names = src
            .tables
            .keys()
            .chain(dst.tables.keys())
            .cloned()
            .collect::<BTreeSet<_>>();

        for table_name in table_names {
            let source_table = src.tables.get(&table_name);
            let target_table = dst.tables.get(&table_name);
            let ancestor_table = anc.tables.get(&table_name);

            if let (Some(s), Some(t)) = (source_table, target_table) {
                if s.columns != t.columns {
                    return Err(format!(
                        "merge conflict: schema differs for table '{table_name}'"
                    ));
                }
            }

            let keys = source_table
                .into_iter()
                .flat_map(|t| t.rows.keys())
                .chain(target_table.into_iter().flat_map(|t| t.rows.keys()))
                .chain(ancestor_table.into_iter().flat_map(|t| t.rows.keys()))
                .cloned()
                .collect::<BTreeSet<_>>();

            for key in keys {
                let base_row = ancestor_table
                    .and_then(|t| t.rows.get(&key))
                    .and_then(|r| Self::visible(r, base_commit));
                let source_row = source_table
                    .and_then(|t| t.rows.get(&key))
                    .and_then(|r| Self::visible(r, self.clock));
                let target_row = target_table
                    .and_then(|t| t.rows.get(&key))
                    .and_then(|r| Self::visible(r, self.clock));

                let source_changed = source_row != base_row;
                let target_changed = target_row != base_row;

                if source_changed && target_changed {
                    if source_row != target_row {
                        return Err(format!(
                            "merge conflict: both branches changed row {key} in '{table_name}'"
                        ));
                    }
                    changes.push((table_name.clone(), key, source_row.cloned()));
                } else if source_changed && !target_changed {
                    changes.push((table_name.clone(), key, source_row.cloned()));
                }
            }
        }

        self.clock += 1;
        let commit = self.clock;
        let target_branch = self.branches.get_mut(target).unwrap();
        for (table, key, value) in &changes {
            if let Some(row) = value {
                let dest = target_branch
                    .tables
                    .entry(table.clone())
                    .or_insert_with(|| Table {
                        columns: src.tables[table].columns.clone(),
                        rows: BTreeMap::new(),
                        primary_index: src.tables[table]
                            .primary_index
                            .as_ref()
                            .and_then(|_| BPlusTree::new(64).ok()),
                        secondary_indexes: src.tables[table].secondary_indexes.clone(),
                    });
                dest.rows
                    .entry(key.clone())
                    .or_insert_with(|| RowChain { versions: vec![] })
                    .versions
                    .push(Version {
                        commit,
                        values: Some(row.clone()),
                    });
                if let Some(Value::Number(k)) = row.get(
                    dest.columns
                        .iter()
                        .position(|c| c.primary_key)
                        .unwrap_or(usize::MAX),
                ) {
                    if let Some(index) = dest.primary_index.as_mut() {
                        index.insert(*k, 1);
                    }
                }
            } else if let Some(row) = target_branch
                .tables
                .get_mut(table)
                .and_then(|t| t.rows.get_mut(key))
            {
                row.versions.push(Version {
                    commit,
                    values: None,
                });
                if let Some(n) = key.strip_prefix('i').and_then(|s| s.parse::<i64>().ok()) {
                    if let Some(table) = target_branch.tables.get_mut(table) {
                        if let Some(index) = table.primary_index.as_mut() {
                            index.remove(n);
                        }
                    }
                }
            }
        }
        self.history.push(History {
            commit,
            branch: target.to_owned(),
            table: "__branch__".into(),
            action: format!("MERGE {source} INTO {target}"),
        });
        Ok(QueryResult::message(format!(
            "merged {source} into {target} with {} row change(s) at commit {commit}",
            changes.len()
        )))
    }

    fn show_history(&self, table: &str, filter: Option<Condition>) -> Result<QueryResult, String> {
        let history_schema = Table {
            primary_index: None,
            secondary_indexes: BTreeMap::new(),
            columns: vec![
                ColumnDef {
                    name: "commit".into(),
                    data_type: ColumnType::Integer,
                    primary_key: false,
                    not_null: true,
                },
                ColumnDef {
                    name: "branch".into(),
                    data_type: ColumnType::String,
                    primary_key: false,
                    not_null: true,
                },
                ColumnDef {
                    name: "action".into(),
                    data_type: ColumnType::String,
                    primary_key: false,
                    not_null: true,
                },
                ColumnDef {
                    name: "table".into(),
                    data_type: ColumnType::String,
                    primary_key: false,
                    not_null: true,
                },
            ],
            rows: BTreeMap::new(),
        };
        let rows = self
            .history
            .iter()
            .filter(|h| h.table == table)
            .filter(|h| {
                let values = vec![
                    Value::Number(h.commit as i64),
                    Value::Text(h.branch.clone()),
                    Value::Text(h.action.clone()),
                    Value::Text(h.table.clone()),
                ];
                filter
                    .as_ref()
                    .map(|f| Self::matches(&history_schema, &values, f))
                    .unwrap_or(true)
            })
            .map(|h| vec![h.commit.to_string(), h.branch.clone(), h.action.clone()])
            .collect::<Vec<_>>();
        let count = rows.len();
        Ok(QueryResult {
            columns: vec!["commit".into(), "branch".into(), "action".into()],
            rows,
            message: format!("{count} history event(s)"),
        })
    }

    fn bisect(&self, table: &str, filter: &Condition) -> Result<QueryResult, String> {
        let _ = self.table(table)?;
        if self.clock == 0 {
            return Ok(QueryResult {
                columns: vec![
                    "commit".into(),
                    "branch".into(),
                    "action".into(),
                    "matching_rows".into(),
                    "probes".into(),
                ],
                rows: vec![],
                message: "bisected 0 commits: condition never matched".into(),
            });
        }
        let mut low = 1u64;
        let mut high = self.clock;
        let mut found: Option<(u64, usize)> = None;
        let mut probes = 0usize;
        while low <= high {
            probes += 1;
            let mid = low + (high - low) / 2;
            let matches = self.matching(table, mid, Some(filter))?;
            if !matches.is_empty() {
                found = Some((mid, matches.len()));
                if mid == 1 {
                    break;
                }
                high = mid - 1;
            } else {
                low = mid + 1;
            }
        }
        if let Some((commit, count)) = found {
            let (branch, action) = self
                .history
                .iter()
                .find(|h| h.commit == commit)
                .map(|h| (h.branch.clone(), h.action.clone()))
                .unwrap_or_else(|| (self.current.clone(), "UNKNOWN".into()));
            Ok(QueryResult {
                columns: vec![
                    "commit".into(),
                    "branch".into(),
                    "action".into(),
                    "matching_rows".into(),
                    "probes".into(),
                ],
                rows: vec![vec![
                    commit.to_string(),
                    branch,
                    action,
                    count.to_string(),
                    probes.to_string(),
                ]],
                message: format!(
                    "bisected {table} in {probes} probe(s): first matched at commit {commit}"
                ),
            })
        } else {
            Ok(QueryResult {
                columns: vec![
                    "commit".into(),
                    "branch".into(),
                    "action".into(),
                    "matching_rows".into(),
                    "probes".into(),
                ],
                rows: vec![],
                message: format!("bisected {table} in {probes} probe(s): condition never matched"),
            })
        }
    }

    fn what_if(&mut self, mutation: Statement, query: Statement) -> Result<QueryResult, String> {
        let branch_name = self.current.clone();
        let branch_backup = self.branch()?.clone();
        let clock_backup = self.clock;
        let history_len = self.history.len();

        let res = (|| {
            let _ = self.execute_statement(mutation)?;
            let mut q_res = self.execute_statement(query)?;
            q_res.message = format!(
                "{} (counterfactual WHAT IF — state rolled back)",
                q_res.message
            );
            Ok(q_res)
        })();

        self.branches.insert(branch_name, branch_backup);
        self.clock = clock_backup;
        self.history.truncate(history_len);
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn sql(db: &mut Database, s: &str) {
        db.execute(s).unwrap();
    }

    #[test]
    fn mvcc_and_branches() {
        let mut d = Database::memory();
        sql(
            &mut d,
            "CREATE TABLE t (id INTEGER PRIMARY KEY, name STRING)",
        );
        sql(&mut d, "INSERT INTO t (id, name) VALUES (1, 'old')");
        sql(&mut d, "UPDATE t SET name = 'new' WHERE id = 1");
        assert_eq!(
            d.execute("SELECT * FROM t AS OF 2").unwrap().rows[0][1],
            "old"
        );
        sql(&mut d, "CREATE BRANCH dev FROM main");
        sql(&mut d, "CHECKOUT dev");
        sql(&mut d, "UPDATE t SET name = 'dev' WHERE id = 1");
        assert_eq!(d.execute("SELECT * FROM t").unwrap().rows[0][1], "dev");
        sql(&mut d, "CHECKOUT main");
        assert_eq!(d.execute("SELECT * FROM t").unwrap().rows[0][1], "new");
    }

    #[test]
    fn transaction_commit_and_rollback() {
        let mut d = Database::memory();
        sql(
            &mut d,
            "CREATE TABLE t (id INTEGER PRIMARY KEY, val STRING)",
        );
        sql(&mut d, "BEGIN");
        sql(&mut d, "INSERT INTO t (id, val) VALUES (1, 'tx-val')");
        assert_eq!(
            d.execute("SELECT val FROM t WHERE id = 1")
                .unwrap()
                .rows
                .len(),
            1
        );
        sql(&mut d, "ROLLBACK");
        assert_eq!(
            d.execute("SELECT val FROM t WHERE id = 1")
                .unwrap()
                .rows
                .len(),
            0
        );

        sql(&mut d, "BEGIN");
        sql(
            &mut d,
            "INSERT INTO t (id, val) VALUES (2, 'committed-val')",
        );
        sql(&mut d, "COMMIT");
        assert_eq!(
            d.execute("SELECT val FROM t WHERE id = 2").unwrap().rows[0][0],
            "committed-val"
        );
    }

    #[test]
    fn concurrent_transactions_isolation_and_conflict() {
        let mut d = Database::memory();
        sql(
            &mut d,
            "CREATE TABLE t (id INTEGER PRIMARY KEY, val STRING)",
        );
        sql(&mut d, "INSERT INTO t VALUES (1, 'original')");

        // L4.02 & L4.04 & L4.05: Snapshot Isolation, Dirty Read & Non-Repeatable Read Prevention
        let tx1 = d.begin_tx().unwrap();
        let tx2 = d.begin_tx().unwrap();
        assert_eq!(
            d.execute_in_tx(tx1, "SELECT val FROM t WHERE id = 1")
                .unwrap()
                .rows[0][0],
            "original"
        );
        d.execute_in_tx(tx2, "UPDATE t SET val = 'modified' WHERE id = 1")
            .unwrap();
        // Before tx2 commits, tx1 still sees 'original' (Dirty Read Prevention)
        assert_eq!(
            d.execute_in_tx(tx1, "SELECT val FROM t WHERE id = 1")
                .unwrap()
                .rows[0][0],
            "original"
        );
        d.commit_tx(tx2).unwrap();
        // After tx2 commits, tx1 STILL sees 'original' (Non-Repeatable Read Prevention)
        assert_eq!(
            d.execute_in_tx(tx1, "SELECT val FROM t WHERE id = 1")
                .unwrap()
                .rows[0][0],
            "original"
        );
        d.commit_tx(tx1).unwrap();
        // Now new reads see 'modified'
        assert_eq!(
            d.execute("SELECT val FROM t WHERE id = 1").unwrap().rows[0][0],
            "modified"
        );

        // L4.03: Write-Write Conflict
        let tx_a = d.begin_tx().unwrap();
        let tx_b = d.begin_tx().unwrap();
        d.execute_in_tx(tx_a, "UPDATE t SET val = 'X' WHERE id = 1")
            .unwrap();
        d.execute_in_tx(tx_b, "UPDATE t SET val = 'Y' WHERE id = 1")
            .unwrap();
        assert!(d.commit_tx(tx_a).is_ok());
        assert!(d.commit_tx(tx_b).unwrap_err().contains("conflict"));
        assert_eq!(
            d.execute("SELECT val FROM t WHERE id = 1").unwrap().rows[0][0],
            "X"
        );
    }

    #[test]
    fn select_computed_expressions_and_aliases() {
        let mut d = Database::memory();
        sql(
            &mut d,
            "CREATE TABLE products (id INTEGER PRIMARY KEY, price FLOAT, qty INTEGER)",
        );
        sql(
            &mut d,
            "INSERT INTO products (id, price, qty) VALUES (1, 10.0, 5)",
        );
        let res = d
            .execute("SELECT id, price * qty AS total FROM products")
            .unwrap();
        assert_eq!(res.columns, vec!["id", "total"]);
        assert_eq!(res.rows[0], vec!["1", "50"]);
    }

    #[test]
    fn common_ancestor_branch_merge() {
        let mut d = Database::memory();
        sql(
            &mut d,
            "CREATE TABLE t (id INTEGER PRIMARY KEY, name STRING)",
        );
        sql(&mut d, "INSERT INTO t (id, name) VALUES (1, 'initial')");
        sql(&mut d, "CREATE BRANCH feat1 FROM main");
        sql(&mut d, "CREATE BRANCH feat2 FROM main");

        sql(&mut d, "CHECKOUT feat1");
        sql(&mut d, "INSERT INTO t (id, name) VALUES (2, 'from-feat1')");

        sql(&mut d, "CHECKOUT feat2");
        sql(&mut d, "INSERT INTO t (id, name) VALUES (3, 'from-feat2')");

        d.execute("MERGE feat1 INTO feat2").unwrap();
        let res = d.execute("SELECT id, name FROM t ORDER BY id").unwrap();
        assert_eq!(res.rows.len(), 3);
        assert_eq!(res.rows[1][1], "from-feat1");
        assert_eq!(res.rows[2][1], "from-feat2");
    }

    #[test]
    fn garbage_collection_and_version_chain() {
        let mut d = Database::memory();
        sql(
            &mut d,
            "CREATE TABLE t (id INTEGER PRIMARY KEY, val STRING)",
        );
        sql(&mut d, "INSERT INTO t VALUES (1, 'A')"); // commit 2
        sql(&mut d, "UPDATE t SET val = 'B' WHERE id = 1"); // commit 3
        sql(&mut d, "UPDATE t SET val = 'C' WHERE id = 1"); // commit 4
        assert_eq!(d.version_history("t", &Value::Number(1)).unwrap().len(), 3);
        // GC with horizon = 3 removes commit 2 ('A'), keeping commit 3 ('B') and commit 4 ('C')
        let removed = d.gc(3);
        assert_eq!(removed, 1);
        assert_eq!(d.version_history("t", &Value::Number(1)).unwrap().len(), 2);
        assert_eq!(
            d.execute("SELECT val FROM t").unwrap().rows[0][0],
            "C"
        );
    }

    #[test]
    fn wal_replays() {
        let root = std::env::temp_dir().join(format!(
            "chronodb-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        {
            let mut d = Database::open(&root).unwrap();
            sql(&mut d, "CREATE TABLE t (id INTEGER PRIMARY KEY)");
            sql(&mut d, "INSERT INTO t (id) VALUES (1)");
            assert!(root.join(".chronodb/index.db").exists());
        }
        let mut d = Database::open(&root).unwrap();
        assert_eq!(
            d.execute("SELECT * FROM t WHERE id = 1")
                .unwrap()
                .rows
                .len(),
            1
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn bisect_and_what_if_counterfactual() {
        let mut d = Database::memory();
        sql(
            &mut d,
            "CREATE TABLE accounts (id INT PRIMARY KEY, balance INT)",
        ); // commit 1
        sql(&mut d, "INSERT INTO accounts VALUES (1, 100)"); // commit 2
        sql(&mut d, "UPDATE accounts SET balance = 50 WHERE id = 1"); // commit 3
        sql(&mut d, "UPDATE accounts SET balance = -20 WHERE id = 1"); // commit 4 (invariant broken)
        sql(&mut d, "UPDATE accounts SET balance = -50 WHERE id = 1"); // commit 5

        let bisect_res = d.execute("BISECT accounts WHERE balance < 0").unwrap();
        assert_eq!(bisect_res.rows[0][0], "4");

        let what_if_res = d
            .execute(
                "WHAT IF (UPDATE accounts SET balance = 500 WHERE id = 1) SELECT balance FROM accounts WHERE id = 1",
            )
            .unwrap();
        assert_eq!(what_if_res.rows[0][0], "500");
        // Verify original state was not mutated
        assert_eq!(
            d.execute("SELECT balance FROM accounts WHERE id = 1")
                .unwrap()
                .rows[0][0],
            "-50"
        );
    }
}
