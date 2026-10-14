use chronodb::Database;
use std::io::{self, Write};

fn run(db: &mut Database, sql: &str) {
    match db.execute(sql) {
        Ok(result) => println!("{result}"),
        Err(error) => eprintln!("error: {error}"),
    }
}

fn main() {
    let mut db = match Database::open(".") {
        Ok(db) => db,
        Err(error) => {
            eprintln!("failed to open database: {error}");
            std::process::exit(1);
        }
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        run(&mut db, &args.join(" "));
        return;
    }
    println!("ChronoDB — type SQL, or .help / .exit");
    let stdin = io::stdin();
    let mut pending = String::new();
    loop {
        if pending.is_empty() {
            print!("chronodb> ");
        } else {
            print!("        ... ");
        }
        let _ = io::stdout().flush();
        let mut line = String::new();
        if stdin.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let trimmed = line.trim();
        if pending.is_empty() {
            match trimmed {
                ".exit" | ".quit" => break,
                ".help" => {
                    println!("SQL: CREATE TABLE, INSERT, UPDATE, DELETE, SELECT ... AS OF <commit>");
                    println!("Transactions: BEGIN, COMMIT, ROLLBACK, CHECKPOINT");
                    println!("Branching: CREATE BRANCH <name> FROM <source>, CHECKOUT <name>, DIFF <b1> <b2>, MERGE <src> INTO <tgt>, SHOW HISTORY <table>");
                    continue;
                }
                "" => continue,
                _ => {}
            }
        }
        pending.push_str(&line);
        if pending.trim_end().ends_with(';') {
            run(&mut db, &pending);
            pending.clear();
        }
    }
}
