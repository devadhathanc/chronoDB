# ChronoDB progress tracker

Last updated: 2026-10-15

| Area | Status | Delivered scope |
|---|---|---|
| Workspace and CLI | Done | Rust crate, one-shot commands, interactive REPL with transaction and branch command guidance, result table formatting |
| Lexer and AST parser | Done | Tokenizer with single-line (`--`) and multi-line (`/* */`) comment stripping, escaped string literals, negative numbers, public `LexerToken` taxonomy; recursive-descent parser supporting `CREATE TABLE`, `CREATE INDEX`, `INSERT`, `UPDATE`, `DELETE`, `SELECT`, `BEGIN`, `COMMIT`, `ROLLBACK`, `CHECKPOINT`, `VACUUM`/`GC`, `EXPLAIN`, `CREATE BRANCH`, `CHECKOUT`, `DIFF`, `MERGE`, `SHOW HISTORY`, `BISECT`, and `WHAT IF`; computed expressions and column aliasing (`AS alias`) in `SELECT` |
| Catalog/schema | Done | Tables, typed columns (`INTEGER`, `FLOAT`, `STRING`, `BOOLEAN`, `TIMESTAMP`), single primary-key validation, nullable constraint checks |
| MVCC version chains | Done | Immutable row version chains with commit timestamps, soft deletes (tombstones), visibility filtering, and `VACUUM`/`GC` pruning |
| Time travel | Done | `SELECT ... AS OF <commit>` point-in-time historical version resolution respecting transaction snapshot boundaries |
| Branch manager | Done | `CREATE BRANCH`, `CHECKOUT`, isolated branch snapshots, row-level `DIFF`, and audit logging (`SHOW HISTORY`) |
| 3-Way Merge & DAG resolution | Done | Lowest Common Ancestor (LCA) search across branch DAGs; 3-way merge comparing ancestor base, source, and target with row-level and schema conflict detection |
| Temporal Debugging (`BISECT` & `WHAT IF`) | Done | `BISECT <table> WHERE <cond>` binary-searches MVCC commit history in $O(\log C)$ probes to isolate the first commit violating an invariant; `WHAT IF (<mutation>) <query>` performs zero-side-effect counterfactual simulation |
| WAL/recovery & Checkpoints | Done | Append-only write-ahead log replay, `CHECKPOINT` support with dirty-page flushing and WAL compaction/truncation, persistent index restoration |
| Query executor (Volcano) | Done | Volcano-style pull operators: `Values` (Scan), `Filter`, `Sort` (typed multi-column ASC/DESC), `Offset`, `Limit`, and `Project` with computed expression evaluation |
| Page manager / buffer pool | Done | 4 KiB slotted page storage (64-byte header, slot directory, bottom-up tuple allocation, defragmentation, FNV-1a checksums) and LRU buffer pool with cache hit/miss statistics |
| B+ tree indexes | Done | 64-bit integer primary-key B+ tree with split/merge, point query, range scan, and checksummed directory/leaf page serialization |
| Transaction control | Done | Snapshot-isolation transaction manager supporting `BEGIN`, `COMMIT`, and `ROLLBACK`, concurrent session isolation (`begin_tx`/`execute_in_tx`), read-your-own-writes isolation, and write-write serialization conflict detection |
| Query planner & optimizer | Done | Logical and physical plan structures with predicate pushdown and primary-key index selection for equality and range filters (`EXPLAIN`) |
