# ChronoDB Terminal Demo Tape (run with: vhs demo.tape)
Output assets/demo.gif
Set FontSize 16
Set Width 1000
Set Height 620
Set Theme "Catppuccin Mocha"
Set Padding 24
Set TypingSpeed 35ms

Type "cargo run --quiet"
Enter
Sleep 800ms

Type "CREATE TABLE accounts (id INT PRIMARY KEY NOT NULL, owner STRING NOT NULL, balance FLOAT);"
Enter
Sleep 500ms

Type "INSERT INTO accounts VALUES (1, 'Ada', 1000.0), (2, 'Grace', 2500.0);"
Enter
Sleep 500ms

Type "UPDATE accounts SET balance = 800.0 WHERE id = 1;"
Enter
Sleep 500ms

Type "SELECT * FROM accounts AS OF 2;"
Enter
Sleep 1.2s

Type "CREATE BRANCH agent_sandbox FROM main;"
Enter
Sleep 400ms

Type "CHECKOUT agent_sandbox;"
Enter
Sleep 400ms

Type "UPDATE accounts SET balance = 5000.0 WHERE id = 2;"
Enter
Sleep 400ms

Type "DIFF main agent_sandbox;"
Enter
Sleep 1.2s

Type "CHECKOUT main;"
Enter
Sleep 400ms

Type "MERGE agent_sandbox INTO main;"
Enter
Sleep 800ms

Type "WHAT IF (UPDATE accounts SET balance = 0.0 WHERE id = 1) SELECT * FROM accounts WHERE balance = 0.0;"
Enter
Sleep 1.5s

Type "SELECT * FROM accounts;"
Enter
Sleep 1.2s

Type "BISECT accounts WHERE balance < 900.0;"
Enter
Sleep 3s

Type ".exit"
Enter
Sleep 500ms
