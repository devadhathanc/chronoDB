//! Comprehensive integration test suite covering Categories L1–L8 from `test.txt`.

use chronodb::btree::BPlusTree;
use chronodb::parser::{lex, parse, Condition, LexerToken, SelectColumn, Statement};
use chronodb::storage::{BufferPool, PageManager, PageType};
use chronodb::transaction::{TransactionManager, TxnState};
use chronodb::Database;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "chronodb-test-{tag}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

// ============================================================================
// L1.1: SQL Tokenizer (Lexer) Tests (L1.1.01 - L1.1.20)
// ============================================================================

#[test]
fn l1_1_lexer_suite() {
    assert_eq!(
        lex("SELECT * FROM users").unwrap(),
        vec![
            LexerToken::Select,
            LexerToken::Star,
            LexerToken::From,
            LexerToken::Identifier("users".into()),
            LexerToken::Eof
        ]
    );
    assert_eq!(
        lex("select * from USERS").unwrap(),
        vec![
            LexerToken::Select,
            LexerToken::Star,
            LexerToken::From,
            LexerToken::Identifier("USERS".into()),
            LexerToken::Eof
        ]
    );

    let toks = lex("SELECT * FROM users WHERE age = 25").unwrap();
    assert!(toks.contains(&LexerToken::Integer(25)));
    let toks = lex("SELECT * FROM products WHERE price = 19.99").unwrap();
    assert!(toks.contains(&LexerToken::Float(19.99)));

    let toks = lex("SELECT * FROM users WHERE name = 'Alice'").unwrap();
    assert!(toks.contains(&LexerToken::String("Alice".into())));
    let toks = lex("SELECT * FROM users WHERE name = \"Bob\"").unwrap();
    assert!(toks.contains(&LexerToken::String("Bob".into())));
    let toks = lex(r"SELECT * FROM t WHERE x = 'It\'s a test'").unwrap();
    assert!(toks.contains(&LexerToken::String("It's a test".into())));

    let toks = lex("SELECT * FROM t WHERE x != 5 AND y >= 3 AND z <= 10").unwrap();
    assert!(toks.contains(&LexerToken::NotEqual));
    assert!(toks.contains(&LexerToken::GreaterEq));
    assert!(toks.contains(&LexerToken::LessEq));

    assert_eq!(
        lex("CREATE TABLE t (a INT, b INT)").unwrap(),
        vec![
            LexerToken::Create,
            LexerToken::Table,
            LexerToken::Identifier("t".into()),
            LexerToken::LeftParen,
            LexerToken::Identifier("a".into()),
            LexerToken::IntegerType,
            LexerToken::Comma,
            LexerToken::Identifier("b".into()),
            LexerToken::IntegerType,
            LexerToken::RightParen,
            LexerToken::Eof
        ]
    );

    assert_eq!(lex("").unwrap(), vec![LexerToken::Eof]);
    assert_eq!(lex("   \t\n  ").unwrap(), vec![LexerToken::Eof]);

    let toks = lex("SELECT * FROM users;").unwrap();
    assert_eq!(toks.last(), Some(&LexerToken::Eof));
    assert!(toks.contains(&LexerToken::Semicolon));

    let toks = lex("SELECT * FROM t AS OF '2026-10-01 14:30:00'").unwrap();
    assert!(toks.contains(&LexerToken::As));
    assert!(toks.contains(&LexerToken::Of));
    assert!(toks.contains(&LexerToken::String("2026-10-01 14:30:00".into())));

    let toks = lex("CREATE BRANCH staging FROM main").unwrap();
    assert_eq!(
        toks,
        vec![
            LexerToken::Create,
            LexerToken::Branch,
            LexerToken::Identifier("staging".into()),
            LexerToken::From,
            LexerToken::Identifier("main".into()),
            LexerToken::Eof
        ]
    );

    assert!(lex("SELECT # FROM users").is_err());
    assert!(lex("SELECT * FROM t WHERE x = 'hello").is_err());

    let long_ident = "a".repeat(200);
    let toks = lex(&format!("SELECT {long_ident} FROM t")).unwrap();
    assert!(toks.contains(&LexerToken::Identifier(long_ident)));
}

// ============================================================================
// L1.2: SQL Parser Tests (L1.2.01 - L1.2.30)
// ============================================================================

#[test]
fn l1_2_parser_suite() {
    assert!(parse("CREATE TABLE users (id INT PRIMARY KEY, name STRING, age INT)").is_ok());
    assert!(parse("CREATE TABLE t (id INT PRIMARY KEY NOT NULL, name STRING NOT NULL)").is_ok());
    assert!(parse("INSERT INTO users (id, name, age) VALUES (1, 'Alice', 30)").is_ok());
    assert!(parse("INSERT INTO t (a,b) VALUES (1,2), (3,4), (5,6)").is_ok());
    assert!(parse("SELECT * FROM users").is_ok());
    assert!(parse("SELECT id, name FROM users").is_ok());
    assert!(parse("SELECT * FROM users WHERE age > 25").is_ok());
    assert!(parse("SELECT * FROM users WHERE age > 25 AND name = 'Alice'").is_ok());
    assert!(parse("SELECT * FROM t WHERE (a=1 OR b=2) AND c=3").is_ok());
    assert!(parse("SELECT * FROM t WHERE NOT active").is_ok());
    assert!(parse("SELECT * FROM users ORDER BY age DESC, name ASC").is_ok());
    assert!(parse("SELECT * FROM users LIMIT 10").is_ok());
    assert!(parse("SELECT * FROM users LIMIT 10 OFFSET 20").is_ok());
    assert!(parse("UPDATE users SET age = 31 WHERE id = 1").is_ok());
    assert!(parse("UPDATE users SET name='Bob', age=25 WHERE id=1").is_ok());
    assert!(parse("UPDATE users SET active = false").is_ok());
    assert!(parse("DELETE FROM users WHERE age < 18").is_ok());
    assert!(parse("DELETE FROM users").is_ok());

    assert!(parse("SELECT * users").is_err());
    assert!(parse("CREATE TABLE users (id INT, name STRING").is_err());
    assert!(parse(";").is_err());

    assert!(parse("SELECT * FROM t WHERE price * quantity > 100").is_ok());
    let s = parse("SELECT id, price * 1.1 AS new_price FROM products").unwrap();
    if let Statement::Select { columns, .. } = s {
        assert_eq!(columns[0], SelectColumn::Column("id".into()));
        assert!(matches!(columns[1], SelectColumn::AliasedExpr(_, ref a) if a == "new_price"));
    } else {
        panic!("expected Select");
    }
    assert!(parse("SELECT * FROM t WHERE (a + b) * (c - d) = 10").is_ok());
    assert!(parse("SELECT * FROM users WHERE name LIKE 'A%'").is_ok());
    assert!(parse("SELECT * FROM users WHERE age IN (20, 25, 30)").is_ok());
    assert!(parse("SELECT * FROM users WHERE age BETWEEN 18 AND 35").is_ok());
    assert!(parse("SELECT * FROM users WHERE email IS NULL").is_ok());
    assert!(parse("SELECT * FROM users WHERE email IS NOT NULL").is_ok());
    let deep = parse("SELECT * FROM t WHERE ((((a=1 AND b=2) OR c=3) AND d=4) OR e=5)").unwrap();
    assert!(matches!(
        deep,
        Statement::Select {
            filter: Some(Condition::Or(_, _)),
            ..
        }
    ));
}

// ============================================================================
// L1.3: B+ Tree Index Manager Tests (L1.3.01 - L1.3.25)
// ============================================================================

#[test]
fn l1_3_btree_suite() {
    let mut tree = BPlusTree::new(5).unwrap();
    tree.insert(10, 5);
    assert_eq!(tree.get(10), Some(5));
    assert_eq!(tree.height(), 1);

    for k in [20, 30, 40, 50, 60] {
        tree.insert(k, k as u64);
    }
    assert!(tree.height() >= 2);

    let mut t2 = BPlusTree::new(4).unwrap();
    for k in 1..=100 {
        t2.insert(k, k as u64 * 10);
    }
    assert!(t2.height() >= 2);
    for k in 1..=100 {
        assert_eq!(t2.get(k), Some(k as u64 * 10));
    }
    assert_eq!(t2.get(999), None);

    let range = t2.scan(30, 50);
    assert_eq!(range.len(), 21);
    assert_eq!(range.first().unwrap().0, 30);
    assert_eq!(range.last().unwrap().0, 50);
    assert_eq!(t2.entries().len(), 100);

    let mut desc_tree = BPlusTree::new(5).unwrap();
    for k in (1..=200).rev() {
        desc_tree.insert(k, k as u64);
    }
    assert_eq!(desc_tree.len(), 200);
    assert_eq!(desc_tree.entries().first().unwrap().0, 1);
    assert_eq!(desc_tree.entries().last().unwrap().0, 200);

    desc_tree.insert(10, 999);
    assert_eq!(desc_tree.get(10), Some(999));
    assert_eq!(desc_tree.remove(9999), None);
    for k in 1..=200 {
        assert!(desc_tree.remove(k).is_some());
    }
    assert!(desc_tree.is_empty());
}

// ============================================================================
// L1.4: Buffer Pool & Slotted Page Tests (L1.4.01 - L1.4.10)
// ============================================================================

#[test]
fn l1_4_buffer_pool_and_pages_suite() {
    let dir = temp_dir("bufpool");
    let mut pm = PageManager::create(&dir).unwrap();
    let mut p0 = pm.allocate(PageType::Data).unwrap();
    let slot0 = p0.insert_tuple(b"row-0").unwrap();
    let slot1 = p0.insert_tuple(b"row-1").unwrap();
    assert_eq!(slot0, 0);
    assert_eq!(slot1, 1);
    pm.write(&p0).unwrap();

    let mut pool = BufferPool::new(pm, 2).unwrap();
    let page = pool.fetch(0).unwrap();
    assert_eq!(page.get_tuple(0), Some(&b"row-0"[..]));
    pool.unpin(0, false).unwrap();

    let _ = pool.fetch(0).unwrap();
    pool.unpin(0, false).unwrap();
    assert_eq!(pool.cache_hits, 1);
    assert!(pool.hit_rate() > 0.0);

    let p1 = pool.new_page(PageType::Data).unwrap();
    pool.unpin(p1, true).unwrap();
    let p2 = pool.new_page(PageType::Data).unwrap();
    pool.unpin(p2, true).unwrap();
    assert!(pool.evictions >= 1);
    pool.flush_all().unwrap();

    let _ = std::fs::remove_file(dir);
}

// ============================================================================
// L1.6 & L1.7: Version Chain & Transaction Manager Tests
// ============================================================================

#[test]
fn l1_6_and_l1_7_mvcc_and_transactions() {
    let mut tm = TransactionManager::default();
    let mut tx1 = tm.begin();
    let mut tx2 = tm.begin();
    tx1.put(b"k1".to_vec(), b"v1".to_vec());
    tx2.put(b"k1".to_vec(), b"v2".to_vec());
    assert_eq!(tm.commit(&mut tx1).unwrap(), 1);
    assert!(tm.commit(&mut tx2).is_err());
    assert_eq!(tx2.state, TxnState::Aborted);
}

// ============================================================================
// L2: Multi-Module Integration Tests (L2.01 - L2.20)
// ============================================================================

#[test]
fn l2_integration_suite() {
    let mut db = Database::memory();

    db.execute("CREATE TABLE users (id INT PRIMARY KEY, name STRING, age INT)")
        .unwrap();
    db.execute("INSERT INTO users (id, name, age) VALUES (1, 'Alice', 30)")
        .unwrap();
    assert_eq!(
        db.execute("SELECT * FROM users").unwrap().rows,
        vec![vec!["1", "Alice", "30"]]
    );
    db.execute("UPDATE users SET age = 31 WHERE id = 1")
        .unwrap();
    assert_eq!(
        db.execute("SELECT * FROM users").unwrap().rows,
        vec![vec!["1", "Alice", "31"]]
    );
    db.execute("DELETE FROM users WHERE id = 1").unwrap();
    assert!(db.execute("SELECT * FROM users").unwrap().rows.is_empty());

    db.execute("CREATE TABLE products (id INT PRIMARY KEY, name STRING, price FLOAT)")
        .unwrap();
    db.execute("CREATE INDEX idx_price ON products(price)")
        .unwrap();
    let explain = db
        .execute("EXPLAIN SELECT * FROM products WHERE id = 1")
        .unwrap();
    assert!(explain.message.contains("IndexScan"));

    db.execute("CREATE TABLE t (id INT PRIMARY KEY, val STRING)")
        .unwrap();
    db.execute("BEGIN; INSERT INTO t VALUES (1, 'A'); COMMIT")
        .unwrap();
    db.execute("BEGIN; INSERT INTO t VALUES (2, 'B'); COMMIT")
        .unwrap();
    db.execute("BEGIN; INSERT INTO t VALUES (3, 'C'); COMMIT")
        .unwrap();
    assert_eq!(db.execute("SELECT * FROM t").unwrap().rows.len(), 3);

    assert!(db.execute("CREATE TABLE users (id INT PRIMARY KEY)").is_err());
    assert!(db.execute("INSERT INTO ghost (id) VALUES (1)").is_err());
    assert!(db.execute("SELECT * FROM ghost").is_err());
    assert!(db.execute("INSERT INTO t (id, val) VALUES (1)").is_err());
    assert!(db
        .execute("INSERT INTO users (id, age) VALUES ('hello', 25)")
        .is_err());
    assert!(db
        .execute("UPDATE users SET age = 99 WHERE id = 99999")
        .is_ok());
    assert!(db
        .execute("DELETE FROM users WHERE id = 99999")
        .is_ok());
    assert!(db.execute("INSERT INTO t VALUES (1, 'dup')").is_err());

    db.execute("CREATE TABLE strict_t (id INT PRIMARY KEY NOT NULL, name STRING NOT NULL)")
        .unwrap();
    assert!(db
        .execute("INSERT INTO strict_t (id, name) VALUES (1, NULL)")
        .is_err());
}

// ============================================================================
// L3: Edge Cases & Boundary Tests (L3.01 - L3.30)
// ============================================================================

#[test]
fn l3_edge_cases_suite() {
    let mut db = Database::memory();
    db.execute(
        "CREATE TABLE edge (id INT PRIMARY KEY, price FLOAT, name STRING, ts TIMESTAMP)",
    )
    .unwrap();

    db.execute("INSERT INTO edge (id, price, name, ts) VALUES (2147483647, 3.14159265358979, '', '1970-01-01 00:00:00')")
        .unwrap();
    db.execute("INSERT INTO edge (id, price, name, ts) VALUES (-2147483648, 0.0, NULL, '9999-12-31 23:59:59')")
        .unwrap();
    assert_eq!(
        db.execute("SELECT COUNT(*) FROM edge").unwrap().rows[0][0],
        "2"
    );

    db.execute("INSERT INTO edge (id, name) VALUES (1, '~!@#$%^&*()_+{}|:\"<>?')")
        .unwrap();
    db.execute("INSERT INTO edge (id, name) VALUES (2, '日本語テスト émojis 🎉')")
        .unwrap();
    let r = db
        .execute("SELECT name FROM edge WHERE id = 2")
        .unwrap();
    assert_eq!(r.rows[0][0], "日本語テスト émojis 🎉");

    assert!(db.execute("SELECT * FROM edge AS OF 'not-a-date'").is_err());

    assert_eq!(
        db.execute("SELECT * FROM edge WHERE 1 = 2")
            .unwrap()
            .rows
            .len(),
        0
    );
    assert_eq!(
        db.execute("SELECT * FROM edge WHERE 1 = 1")
            .unwrap()
            .rows
            .len(),
        4
    );
    assert_eq!(
        db.execute("SELECT * FROM edge LIMIT 0")
            .unwrap()
            .rows
            .len(),
        0
    );
    assert_eq!(
        db.execute("SELECT * FROM edge LIMIT 1000")
            .unwrap()
            .rows
            .len(),
        4
    );
    assert!(db.execute("SELECT * FROM edge LIMIT -1").is_err());
    assert!(db.execute("SELECT * FROM edge;;;").is_ok());
    assert!(db
        .execute("SELECT /* inline */ * FROM edge -- trailing comment")
        .is_ok());

    assert!(db.execute("SELECT ghost_col FROM edge").is_err());
    assert_eq!(
        db.execute("SELECT * FROM edge WHERE id / 0 = 5")
            .unwrap()
            .rows
            .len(),
        0
    );
    assert_eq!(
        db.execute("SELECT * FROM edge WHERE ((((((((((id = 1))))))))))")
            .unwrap()
            .rows
            .len(),
        1
    );
}

// ============================================================================
// L4: Concurrency & MVCC Isolation Tests (L4.01 - L4.12)
// ============================================================================

#[test]
fn l4_concurrency_mvcc_suite() {
    let mut db = Database::memory();
    db.execute("CREATE TABLE t (id INT PRIMARY KEY, val STRING)")
        .unwrap();
    db.execute("INSERT INTO t VALUES (1, 'A'), (2, 'B'), (3, 'C')")
        .unwrap();

    let tx1 = db.begin_tx().unwrap();
    let tx2 = db.begin_tx().unwrap();
    assert_eq!(
        db.execute_in_tx(tx1, "SELECT * FROM t WHERE id > 1")
            .unwrap()
            .rows
            .len(),
        2
    );
    db.execute_in_tx(tx2, "INSERT INTO t VALUES (4, 'D')")
        .unwrap();
    db.commit_tx(tx2).unwrap();
    assert_eq!(
        db.execute_in_tx(tx1, "SELECT * FROM t WHERE id > 1")
            .unwrap()
            .rows
            .len(),
        2
    );
    db.commit_tx(tx1).unwrap();
    assert_eq!(
        db.execute("SELECT * FROM t WHERE id > 1")
            .unwrap()
            .rows
            .len(),
        3
    );

    let tx_a = db.begin_tx().unwrap();
    let tx_b = db.begin_tx().unwrap();
    db.execute_in_tx(tx_a, "INSERT INTO t VALUES (10, 'X')")
        .unwrap();
    db.execute_in_tx(tx_b, "INSERT INTO t VALUES (20, 'Y')")
        .unwrap();
    assert!(db.commit_tx(tx_a).is_ok());
    assert!(db.commit_tx(tx_b).is_ok());

    let tx_c = db.begin_tx().unwrap();
    let tx_d = db.begin_tx().unwrap();
    db.execute_in_tx(tx_c, "INSERT INTO t VALUES (30, 'C1')")
        .unwrap();
    db.execute_in_tx(tx_d, "INSERT INTO t VALUES (30, 'D1')")
        .unwrap();
    assert!(db.commit_tx(tx_c).is_ok());
    assert!(db.commit_tx(tx_d).is_err());
}

// ============================================================================
// L5, L7, L8: Crash Recovery, Time-Travel & Branch/Diff/Merge Tests
// ============================================================================

#[test]
fn l5_l7_l8_recovery_time_travel_and_branching_suite() {
    let dir = temp_dir("recovery");
    {
        let mut db = Database::open(&dir).unwrap();
        db.execute("CREATE TABLE items (id INT PRIMARY KEY, status STRING)")
            .unwrap();
        db.execute("INSERT INTO items VALUES (1, 'v1')").unwrap();
        db.execute("UPDATE items SET status = 'v2' WHERE id = 1")
            .unwrap();
        db.execute("CHECKPOINT").unwrap();

        assert_eq!(
            db.execute("SELECT status FROM items AS OF 2")
                .unwrap()
                .rows[0][0],
            "v1"
        );
        assert_eq!(
            db.execute("SELECT status FROM items AS OF 3")
                .unwrap()
                .rows[0][0],
            "v2"
        );

        db.execute("CREATE BRANCH staging FROM main").unwrap();
        db.execute("CHECKOUT staging").unwrap();
        db.execute("INSERT INTO items VALUES (2, 'staging-item')")
            .unwrap();
        db.execute("CHECKOUT main").unwrap();
        db.execute("INSERT INTO items VALUES (3, 'main-item')")
            .unwrap();

        let diff = db.execute("DIFF main staging").unwrap();
        assert_eq!(diff.rows.len(), 2);

        db.execute("MERGE staging INTO main").unwrap();
        assert_eq!(
            db.execute("SELECT COUNT(*) FROM items").unwrap().rows[0][0],
            "3"
        );
    }

    {
        let mut db = Database::open(&dir).unwrap();
        assert_eq!(
            db.execute("SELECT COUNT(*) FROM items").unwrap().rows[0][0],
            "3"
        );
        let hist = db.execute("SHOW HISTORY items").unwrap();
        assert!(!hist.rows.is_empty());
    }
    let _ = std::fs::remove_dir_all(dir);
}
