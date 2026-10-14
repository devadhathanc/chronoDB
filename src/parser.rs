use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(i64),
    Float(f64),
    Text(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ColumnType {
    Integer,
    Float,
    String,
    Boolean,
    Timestamp,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnDef {
    pub name: String,
    pub data_type: ColumnType,
    pub primary_key: bool,
    pub not_null: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    Like,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ArithmeticOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expression {
    Column(String),
    Literal(Value),
    Binary(Box<Expression>, ArithmeticOp, Box<Expression>),
}

impl fmt::Display for Expression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Column(c) => write!(f, "{c}"),
            Self::Literal(v) => write!(f, "{v}"),
            Self::Binary(l, op, r) => {
                let op_str = match op {
                    ArithmeticOp::Add => "+",
                    ArithmeticOp::Subtract => "-",
                    ArithmeticOp::Multiply => "*",
                    ArithmeticOp::Divide => "/",
                    ArithmeticOp::Modulo => "%",
                };
                write!(f, "{l} {op_str} {r}")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    Compare(String, Op, Value),
    And(Box<Condition>, Box<Condition>),
    Or(Box<Condition>, Box<Condition>),
    Not(Box<Condition>),
    In(String, Vec<Value>),
    Between(String, Value, Value),
    IsNull(String, bool),
    CompareExpr(Expression, Op, Expression),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SelectColumn {
    Wildcard,
    CountStar,
    Column(String),
    AliasedExpr(Expression, String),
    Expr(Expression),
}

impl SelectColumn {
    pub fn name(&self) -> String {
        match self {
            Self::Wildcard => "*".into(),
            Self::CountStar => "COUNT(*)".into(),
            Self::Column(name) => name.clone(),
            Self::AliasedExpr(_, alias) => alias.clone(),
            Self::Expr(expr) => expr.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    CreateTable {
        name: String,
        columns: Vec<ColumnDef>,
    },
    CreateIndex {
        name: String,
        table: String,
        column: String,
    },
    Insert {
        table: String,
        columns: Vec<String>,
        values: Vec<Value>,
        additional_rows: Vec<Vec<Value>>,
    },
    Update {
        table: String,
        assignments: Vec<(String, Value)>,
        filter: Option<Condition>,
    },
    Delete {
        table: String,
        filter: Option<Condition>,
    },
    Select {
        columns: Vec<SelectColumn>,
        table: String,
        as_of: Option<u64>,
        filter: Option<Condition>,
        order_by: Vec<(String, bool)>,
        limit: Option<usize>,
        offset: Option<usize>,
    },
    CreateBranch {
        name: String,
        from: String,
    },
    Checkout {
        name: String,
    },
    Diff {
        left: String,
        right: String,
    },
    Merge {
        source: String,
        target: String,
    },
    ShowHistory {
        table: String,
        filter: Option<Condition>,
    },
    Begin,
    Commit,
    Rollback,
    Checkpoint,
    Vacuum {
        retain_from: u64,
    },
    Bisect {
        table: String,
        filter: Condition,
    },
    WhatIf {
        mutation: Box<Statement>,
        query: Box<Statement>,
    },
    Explain(Box<Statement>),
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Word(String),
    String(String),
    Number(i64),
    Float(f64),
    Plus,
    Minus,
    Slash,
    Percent,
    Star,
    Comma,
    LParen,
    RParen,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    Semi,
    Dot,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LexerToken {
    Select,
    From,
    Where,
    Insert,
    Into,
    Values,
    Update,
    Set,
    Delete,
    Create,
    Table,
    Branch,
    Checkout,
    Merge,
    Diff,
    As,
    Of,
    Show,
    History,
    Order,
    By,
    Group,
    Having,
    Join,
    On,
    Primary,
    Key,
    Not,
    Null,
    True,
    False,
    And,
    Or,
    In,
    Between,
    Like,
    Limit,
    Offset,
    Begin,
    Commit,
    Rollback,
    Checkpoint,
    IntegerType,
    FloatType,
    StringType,
    BooleanType,
    TimestampType,
    Identifier(String),
    Integer(i64),
    Float(f64),
    String(String),
    Equal,
    NotEqual,
    LessThan,
    GreaterThan,
    LessEq,
    GreaterEq,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Comma,
    Semicolon,
    Dot,
    LeftParen,
    RightParen,
    Eof,
}

pub fn lex(input: &str) -> Result<Vec<LexerToken>, String> {
    let mut result = tokenize(input)?
        .into_iter()
        .map(|token| match token {
            Token::Word(word) => match word.to_ascii_uppercase().as_str() {
                "SELECT" => LexerToken::Select,
                "FROM" => LexerToken::From,
                "WHERE" => LexerToken::Where,
                "INSERT" => LexerToken::Insert,
                "INTO" => LexerToken::Into,
                "VALUES" => LexerToken::Values,
                "UPDATE" => LexerToken::Update,
                "SET" => LexerToken::Set,
                "DELETE" => LexerToken::Delete,
                "CREATE" => LexerToken::Create,
                "TABLE" => LexerToken::Table,
                "BRANCH" => LexerToken::Branch,
                "CHECKOUT" => LexerToken::Checkout,
                "MERGE" => LexerToken::Merge,
                "DIFF" => LexerToken::Diff,
                "AS" => LexerToken::As,
                "OF" => LexerToken::Of,
                "SHOW" => LexerToken::Show,
                "HISTORY" => LexerToken::History,
                "ORDER" => LexerToken::Order,
                "BY" => LexerToken::By,
                "GROUP" => LexerToken::Group,
                "HAVING" => LexerToken::Having,
                "JOIN" => LexerToken::Join,
                "ON" => LexerToken::On,
                "PRIMARY" => LexerToken::Primary,
                "KEY" => LexerToken::Key,
                "NOT" => LexerToken::Not,
                "NULL" => LexerToken::Null,
                "TRUE" => LexerToken::True,
                "FALSE" => LexerToken::False,
                "AND" => LexerToken::And,
                "OR" => LexerToken::Or,
                "IN" => LexerToken::In,
                "BETWEEN" => LexerToken::Between,
                "LIKE" => LexerToken::Like,
                "LIMIT" => LexerToken::Limit,
                "OFFSET" => LexerToken::Offset,
                "BEGIN" => LexerToken::Begin,
                "COMMIT" => LexerToken::Commit,
                "ROLLBACK" => LexerToken::Rollback,
                "CHECKPOINT" => LexerToken::Checkpoint,
                "INT" | "INTEGER" => LexerToken::IntegerType,
                "FLOAT" => LexerToken::FloatType,
                "STRING" | "TEXT" => LexerToken::StringType,
                "BOOL" | "BOOLEAN" => LexerToken::BooleanType,
                "TIMESTAMP" => LexerToken::TimestampType,
                _ => LexerToken::Identifier(word),
            },
            Token::String(s) => LexerToken::String(s),
            Token::Number(n) => LexerToken::Integer(n),
            Token::Float(n) => LexerToken::Float(n),
            Token::Star => LexerToken::Star,
            Token::Plus => LexerToken::Plus,
            Token::Minus => LexerToken::Minus,
            Token::Slash => LexerToken::Slash,
            Token::Percent => LexerToken::Percent,
            Token::Comma => LexerToken::Comma,
            Token::Semi => LexerToken::Semicolon,
            Token::Dot => LexerToken::Dot,
            Token::LParen => LexerToken::LeftParen,
            Token::RParen => LexerToken::RightParen,
            Token::Eq => LexerToken::Equal,
            Token::Ne => LexerToken::NotEqual,
            Token::Lt => LexerToken::LessThan,
            Token::Gt => LexerToken::GreaterThan,
            Token::Le => LexerToken::LessEq,
            Token::Ge => LexerToken::GreaterEq,
        })
        .collect::<Vec<_>>();
    result.push(LexerToken::Eof);
    Ok(result)
}

fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let mut out = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            c if c.is_whitespace() => i += 1,
            '-' if chars.get(i + 1) == Some(&'-') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                if i < chars.len() {
                    i += 2;
                }
            }
            '(' => {
                out.push(Token::LParen);
                i += 1;
            }
            ')' => {
                out.push(Token::RParen);
                i += 1;
            }
            ',' => {
                out.push(Token::Comma);
                i += 1;
            }
            '*' => {
                out.push(Token::Star);
                i += 1;
            }
            '-' => {
                out.push(Token::Minus);
                i += 1;
            }
            '+' => {
                out.push(Token::Plus);
                i += 1;
            }
            '/' => {
                out.push(Token::Slash);
                i += 1;
            }
            '%' => {
                out.push(Token::Percent);
                i += 1;
            }
            ';' => {
                out.push(Token::Semi);
                i += 1;
            }
            '.' => {
                out.push(Token::Dot);
                i += 1;
            }
            '=' => {
                out.push(Token::Eq);
                i += 1;
            }
            '!' if chars.get(i + 1) == Some(&'=') => {
                out.push(Token::Ne);
                i += 2;
            }
            '<' if chars.get(i + 1) == Some(&'=') => {
                out.push(Token::Le);
                i += 2;
            }
            '>' if chars.get(i + 1) == Some(&'=') => {
                out.push(Token::Ge);
                i += 2;
            }
            '<' => {
                out.push(Token::Lt);
                i += 1;
            }
            '>' => {
                out.push(Token::Gt);
                i += 1;
            }
            '\'' | '"' => {
                let quote = chars[i];
                i += 1;
                let mut s = String::new();
                while i < chars.len() {
                    if chars[i] == quote {
                        if chars.get(i + 1) == Some(&quote) {
                            s.push(quote);
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 1;
                        match chars[i] {
                            'n' => s.push('\n'),
                            't' => s.push('\t'),
                            'r' => s.push('\r'),
                            '\\' => s.push('\\'),
                            '\'' => s.push('\''),
                            '"' => s.push('"'),
                            other => {
                                s.push('\\');
                                s.push(other);
                            }
                        }
                        i += 1;
                        continue;
                    }
                    s.push(chars[i]);
                    i += 1;
                }
                if i == chars.len() {
                    return Err("Unterminated string literal".into());
                }
                i += 1;
                out.push(Token::String(s));
            }
            c if c.is_ascii_digit() => {
                let start = i;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                if i < chars.len()
                    && chars[i] == '.'
                    && chars.get(i + 1).is_some_and(|ch| ch.is_ascii_digit())
                {
                    i += 1;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                let literal: String = chars[start..i].iter().collect();
                if literal.contains('.') {
                    out.push(Token::Float(literal.parse().map_err(|_| "invalid float")?));
                } else {
                    out.push(Token::Number(
                        literal.parse().map_err(|_| "invalid number")?,
                    ));
                }
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                out.push(Token::Word(chars[start..i].iter().collect()));
            }
            c => return Err(format!("Unexpected character '{c}' at position {i}")),
        }
    }
    Ok(out)
}

pub fn is_valid_timestamp_str(s: &str) -> bool {
    // Format: YYYY-MM-DD HH:MM:SS or YYYY-MM-DD
    let bytes = s.as_bytes();
    if bytes.len() != 19 && bytes.len() != 10 {
        return false;
    }
    let is_digits = |slice: &[u8]| slice.iter().all(|b| b.is_ascii_digit());
    if !is_digits(&bytes[0..4])
        || bytes[4] != b'-'
        || !is_digits(&bytes[5..7])
        || bytes[7] != b'-'
        || !is_digits(&bytes[8..10])
    {
        return false;
    }
    let month: u32 = s[5..7].parse().unwrap_or(0);
    let day: u32 = s[8..10].parse().unwrap_or(0);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return false;
    }
    if bytes.len() == 19 {
        if bytes[10] != b' ' && bytes[10] != b'T' {
            return false;
        }
        if !is_digits(&bytes[11..13])
            || bytes[13] != b':'
            || !is_digits(&bytes[14..16])
            || bytes[16] != b':'
            || !is_digits(&bytes[17..19])
        {
            return false;
        }
        let hour: u32 = s[11..13].parse().unwrap_or(99);
        let min: u32 = s[14..16].parse().unwrap_or(99);
        let sec: u32 = s[17..19].parse().unwrap_or(99);
        if hour > 23 || min > 59 || sec > 59 {
            return false;
        }
    }
    true
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn is_word(&self, expected: &str) -> bool {
        matches!(self.peek(), Some(Token::Word(w)) if w.eq_ignore_ascii_case(expected))
    }

    fn take(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        self.pos += usize::from(t.is_some());
        t
    }

    fn word(&mut self, expected: &str) -> Result<(), String> {
        match self.take() {
            Some(Token::Word(w)) if w.eq_ignore_ascii_case(expected) => Ok(()),
            got => Err(format!("expected {expected}, got {got:?}")),
        }
    }

    fn ident(&mut self) -> Result<String, String> {
        match self.take() {
            Some(Token::Word(w)) => Ok(w.to_ascii_lowercase()),
            got => Err(format!("expected identifier, got {got:?}")),
        }
    }

    fn comma(&mut self) -> bool {
        if self.peek() == Some(&Token::Comma) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn value(&mut self) -> Result<Value, String> {
        match self.peek() {
            Some(Token::Minus) => {
                self.pos += 1;
                match self.take() {
                    Some(Token::Number(n)) => Ok(Value::Number(-n)),
                    Some(Token::Float(f)) => Ok(Value::Float(-f)),
                    got => Err(format!("expected number after minus, got {got:?}")),
                }
            }
            _ => match self.take() {
                Some(Token::String(s)) => Ok(Value::Text(s)),
                Some(Token::Number(n)) => Ok(Value::Number(n)),
                Some(Token::Float(n)) => Ok(Value::Float(n)),
                Some(Token::Word(w)) if w.eq_ignore_ascii_case("TRUE") => Ok(Value::Bool(true)),
                Some(Token::Word(w)) if w.eq_ignore_ascii_case("FALSE") => Ok(Value::Bool(false)),
                Some(Token::Word(w)) if w.eq_ignore_ascii_case("NULL") => Ok(Value::Null),
                got => Err(format!("expected literal, got {got:?}")),
            },
        }
    }

    fn condition(&mut self) -> Result<Condition, String> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Condition, String> {
        let mut left = self.parse_and()?;
        while self.is_word("OR") {
            self.pos += 1;
            left = Condition::Or(Box::new(left), Box::new(self.parse_and()?));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Condition, String> {
        let mut left = self.parse_not()?;
        while self.is_word("AND") {
            self.pos += 1;
            left = Condition::And(Box::new(left), Box::new(self.parse_not()?));
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<Condition, String> {
        if self.is_word("NOT") {
            self.pos += 1;
            return Ok(Condition::Not(Box::new(self.parse_not()?)));
        }
        if self.peek() == Some(&Token::LParen) && self.starts_boolean_group() {
            self.pos += 1;
            let condition = self.parse_or()?;
            match self.take() {
                Some(Token::RParen) => return Ok(condition),
                got => return Err(format!("expected closing parenthesis, got {got:?}")),
            }
        }
        self.parse_predicate()
    }

    fn starts_boolean_group(&self) -> bool {
        let mut depth = 0usize;
        let mut end = None;
        for i in self.pos..self.tokens.len() {
            match self.tokens[i] {
                Token::LParen => depth += 1,
                Token::RParen => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        end = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else { return false };
        if matches!(
            self.tokens.get(end + 1),
            Some(
                Token::Eq
                    | Token::Ne
                    | Token::Lt
                    | Token::Gt
                    | Token::Le
                    | Token::Ge
                    | Token::Plus
                    | Token::Minus
                    | Token::Star
                    | Token::Slash
                    | Token::Percent
            )
        ) {
            return false;
        }
        self.tokens[self.pos + 1..end].iter().any(|t| {
            matches!(
                t,
                Token::Eq | Token::Ne | Token::Lt | Token::Gt | Token::Le | Token::Ge
            ) || matches!(t, Token::Word(w) if ["AND", "OR", "LIKE", "IN", "BETWEEN", "IS", "NOT"].iter().any(|k| w.eq_ignore_ascii_case(k)))
        })
    }

    fn parse_predicate(&mut self) -> Result<Condition, String> {
        let left = self.parse_expression()?;
        let simple_column = if let Expression::Column(c) = &left {
            Some(c.clone())
        } else {
            None
        };
        if let Some(col) = simple_column.as_ref() {
            if self.is_word("IS") {
                self.pos += 1;
                let negated = if self.is_word("NOT") {
                    self.pos += 1;
                    true
                } else {
                    false
                };
                self.word("NULL")?;
                return Ok(Condition::IsNull(col.clone(), negated));
            }
            if self.is_word("IN") {
                self.pos += 1;
                match self.take() {
                    Some(Token::LParen) => (),
                    got => return Err(format!("expected ( after IN, got {got:?}")),
                }
                let mut values = vec![self.value()?];
                while self.comma() {
                    values.push(self.value()?);
                }
                match self.take() {
                    Some(Token::RParen) => (),
                    got => return Err(format!("expected ) after IN list, got {got:?}")),
                }
                return Ok(Condition::In(col.clone(), values));
            }
            if self.is_word("BETWEEN") {
                self.pos += 1;
                let low = self.value()?;
                self.word("AND")?;
                let high = self.value()?;
                return Ok(Condition::Between(col.clone(), low, high));
            }
        }
        if matches!(self.peek(), None | Some(Token::RParen) | Some(Token::Semi))
            || self.is_word("AND")
            || self.is_word("OR")
        {
            if let Some(col) = simple_column {
                return Ok(Condition::Compare(col, Op::Eq, Value::Bool(true)));
            }
            return Err("expected comparison operator after expression".into());
        }
        let op = match self.take() {
            Some(Token::Eq) => Op::Eq,
            Some(Token::Ne) => Op::Ne,
            Some(Token::Lt) => Op::Lt,
            Some(Token::Gt) => Op::Gt,
            Some(Token::Le) => Op::Le,
            Some(Token::Ge) => Op::Ge,
            Some(Token::Word(w)) if w.eq_ignore_ascii_case("LIKE") => Op::Like,
            got => return Err(format!("expected comparison operator, got {got:?}")),
        };
        let right = self.parse_expression()?;
        if let (Expression::Column(col), Expression::Literal(value)) = (&left, &right) {
            Ok(Condition::Compare(col.clone(), op, value.clone()))
        } else {
            Ok(Condition::CompareExpr(left, op, right))
        }
    }

    pub fn parse_expression(&mut self) -> Result<Expression, String> {
        self.parse_add_sub()
    }

    fn parse_add_sub(&mut self) -> Result<Expression, String> {
        let mut left = self.parse_mul_div()?;
        loop {
            let op = match self.peek() {
                Some(Token::Plus) => ArithmeticOp::Add,
                Some(Token::Minus) => ArithmeticOp::Subtract,
                _ => break,
            };
            self.pos += 1;
            left = Expression::Binary(Box::new(left), op, Box::new(self.parse_mul_div()?));
        }
        Ok(left)
    }

    fn parse_mul_div(&mut self) -> Result<Expression, String> {
        let mut left = self.parse_factor()?;
        loop {
            let op = match self.peek() {
                Some(Token::Star) => ArithmeticOp::Multiply,
                Some(Token::Slash) => ArithmeticOp::Divide,
                Some(Token::Percent) => ArithmeticOp::Modulo,
                _ => break,
            };
            self.pos += 1;
            left = Expression::Binary(Box::new(left), op, Box::new(self.parse_factor()?));
        }
        Ok(left)
    }

    fn parse_factor(&mut self) -> Result<Expression, String> {
        match self.take() {
            Some(Token::Minus) => match self.take() {
                Some(Token::Number(n)) => Ok(Expression::Literal(Value::Number(-n))),
                Some(Token::Float(n)) => Ok(Expression::Literal(Value::Float(-n))),
                got => Err(format!("unary minus requires a numeric value, got {got:?}")),
            },
            Some(Token::Number(n)) => Ok(Expression::Literal(Value::Number(n))),
            Some(Token::Float(n)) => Ok(Expression::Literal(Value::Float(n))),
            Some(Token::String(s)) => Ok(Expression::Literal(Value::Text(s))),
            Some(Token::LParen) => {
                let expr = self.parse_expression()?;
                match self.take() {
                    Some(Token::RParen) => Ok(expr),
                    got => Err(format!("expected closing parenthesis, got {got:?}")),
                }
            }
            Some(Token::Word(w)) if w.eq_ignore_ascii_case("TRUE") => {
                Ok(Expression::Literal(Value::Bool(true)))
            }
            Some(Token::Word(w)) if w.eq_ignore_ascii_case("FALSE") => {
                Ok(Expression::Literal(Value::Bool(false)))
            }
            Some(Token::Word(w)) if w.eq_ignore_ascii_case("NULL") => {
                Ok(Expression::Literal(Value::Null))
            }
            Some(Token::Word(w)) => Ok(Expression::Column(w.to_ascii_lowercase())),
            got => Err(format!("expected expression, got {got:?}")),
        }
    }

    fn optional_where(&mut self) -> Result<Option<Condition>, String> {
        if self.is_word("WHERE") {
            self.pos += 1;
            Ok(Some(self.condition()?))
        } else {
            Ok(None)
        }
    }

    fn parse_one_statement(&mut self) -> Result<Statement, String> {
        let first = match self.take() {
            Some(Token::Word(w)) => w.to_ascii_uppercase(),
            _ => return Err("expected SQL statement".into()),
        };
        match first.as_str() {
            "EXPLAIN" => {
                let inner = self.parse_one_statement()?;
                Ok(Statement::Explain(Box::new(inner)))
            }
            "BEGIN" => {
                if self.is_word("TRANSACTION") || self.is_word("WORK") {
                    self.pos += 1;
                }
                Ok(Statement::Begin)
            }
            "COMMIT" => {
                if self.is_word("TRANSACTION") || self.is_word("WORK") {
                    self.pos += 1;
                }
                Ok(Statement::Commit)
            }
            "ROLLBACK" => {
                if self.is_word("TRANSACTION") || self.is_word("WORK") {
                    self.pos += 1;
                }
                Ok(Statement::Rollback)
            }
            "CHECKPOINT" => Ok(Statement::Checkpoint),
            "VACUUM" | "GC" => {
                let retain_from = match self.peek() {
                    Some(Token::Number(n)) if *n >= 0 => {
                        let n = *n as u64;
                        self.pos += 1;
                        n
                    }
                    _ => 0,
                };
                Ok(Statement::Vacuum { retain_from })
            }
            "CREATE" => {
                let kind = self.ident()?;
                if kind == "table" {
                    let name = self.ident()?;
                    match self.take() {
                        Some(Token::LParen) => (),
                        x => return Err(format!("expected (, got {x:?}")),
                    };
                    let mut columns = vec![];
                    loop {
                        let name = self.ident()?;
                        let ty = match self.ident()?.as_str() {
                            "integer" | "int" => ColumnType::Integer,
                            "float" => ColumnType::Float,
                            "string" | "text" => ColumnType::String,
                            "boolean" | "bool" => ColumnType::Boolean,
                            "timestamp" => ColumnType::Timestamp,
                            other => return Err(format!("unsupported type {other}")),
                        };
                        let mut pk = false;
                        let mut nn = false;
                        loop {
                            match self.peek() {
                                Some(Token::Word(w)) if w.eq_ignore_ascii_case("PRIMARY") => {
                                    self.pos += 1;
                                    self.word("KEY")?;
                                    pk = true;
                                }
                                Some(Token::Word(w)) if w.eq_ignore_ascii_case("NOT") => {
                                    self.pos += 1;
                                    self.word("NULL")?;
                                    nn = true;
                                }
                                _ => break,
                            }
                        }
                        columns.push(ColumnDef {
                            name,
                            data_type: ty,
                            primary_key: pk,
                            not_null: nn,
                        });
                        if !self.comma() {
                            break;
                        }
                    }
                    match self.take() {
                        Some(Token::RParen) => (),
                        x => return Err(format!("expected ), got {x:?}")),
                    };
                    Ok(Statement::CreateTable { name, columns })
                } else if kind == "branch" {
                    let name = self.ident()?;
                    self.word("FROM")?;
                    Ok(Statement::CreateBranch {
                        name,
                        from: self.ident()?,
                    })
                } else if kind == "index" {
                    let name = self.ident()?;
                    self.word("ON")?;
                    let table = self.ident()?;
                    match self.take() {
                        Some(Token::LParen) => (),
                        x => return Err(format!("expected (, got {x:?}")),
                    };
                    let column = self.ident()?;
                    match self.take() {
                        Some(Token::RParen) => (),
                        x => return Err(format!("expected ), got {x:?}")),
                    };
                    Ok(Statement::CreateIndex {
                        name,
                        table,
                        column,
                    })
                } else {
                    Err(format!("unknown CREATE target {kind}"))
                }
            }
            "INSERT" => {
                self.word("INTO")?;
                let table = self.ident()?;
                let mut columns = Vec::new();
                if self.peek() == Some(&Token::LParen) {
                    self.pos += 1;
                    columns.push(self.ident()?);
                    while self.comma() {
                        columns.push(self.ident()?);
                    }
                    match self.take() {
                        Some(Token::RParen) => (),
                        x => return Err(format!("expected ), got {x:?}")),
                    }
                }
                self.word("VALUES")?;
                match self.take() {
                    Some(Token::LParen) => (),
                    x => return Err(format!("expected (, got {x:?}")),
                };
                let mut values = vec![self.value()?];
                while self.comma() {
                    values.push(self.value()?);
                }
                match self.take() {
                    Some(Token::RParen) => (),
                    x => return Err(format!("expected ), got {x:?}")),
                };
                let mut additional_rows = Vec::new();
                while self.peek() == Some(&Token::Comma) {
                    self.pos += 1;
                    match self.take() {
                        Some(Token::LParen) => (),
                        x => return Err(format!("expected ( before next VALUES row, got {x:?}")),
                    }
                    let mut row = vec![self.value()?];
                    while self.comma() {
                        row.push(self.value()?);
                    }
                    match self.take() {
                        Some(Token::RParen) => (),
                        x => return Err(format!("expected ), got {x:?}")),
                    }
                    additional_rows.push(row);
                }
                Ok(Statement::Insert {
                    table,
                    columns,
                    values,
                    additional_rows,
                })
            }
            "UPDATE" => {
                let table = self.ident()?;
                self.word("SET")?;
                let mut assignments = vec![];
                loop {
                    let col = self.ident()?;
                    match self.take() {
                        Some(Token::Eq) => (),
                        x => return Err(format!("expected =, got {x:?}")),
                    };
                    assignments.push((col, self.value()?));
                    if !self.comma() {
                        break;
                    }
                }
                let filter = self.optional_where()?;
                Ok(Statement::Update {
                    table,
                    assignments,
                    filter,
                })
            }
            "DELETE" => {
                self.word("FROM")?;
                let table = self.ident()?;
                let filter = self.optional_where()?;
                Ok(Statement::Delete { table, filter })
            }
            "SELECT" => {
                let mut columns = vec![];
                if self.peek() == Some(&Token::Star) {
                    self.pos += 1;
                    columns.push(SelectColumn::Wildcard);
                } else {
                    loop {
                        if self.is_word("COUNT")
                            && self.tokens.get(self.pos + 1) == Some(&Token::LParen)
                            && self.tokens.get(self.pos + 2) == Some(&Token::Star)
                            && self.tokens.get(self.pos + 3) == Some(&Token::RParen)
                        {
                            self.pos += 4;
                            if self.is_word("AS") {
                                self.pos += 1;
                                let _ = self.ident()?;
                            }
                            columns.push(SelectColumn::CountStar);
                        } else {
                            let expr = self.parse_expression()?;
                            let col = if self.is_word("AS") {
                                self.pos += 1;
                                let alias = self.ident()?;
                                SelectColumn::AliasedExpr(expr, alias)
                            } else if let Expression::Column(ref name) = expr {
                                SelectColumn::Column(name.clone())
                            } else {
                                SelectColumn::Expr(expr)
                            };
                            columns.push(col);
                        }
                        if !self.comma() {
                            break;
                        }
                    }
                }
                self.word("FROM")?;
                let table = self.ident()?;
                let as_of = if self.is_word("AS") {
                    self.pos += 1;
                    self.word("OF")?;
                    match self.take() {
                        Some(Token::Number(n)) if n >= 0 => Some(n as u64),
                        Some(Token::String(s)) => {
                            if let Ok(n) = s.parse::<u64>() {
                                Some(n)
                            } else if is_valid_timestamp_str(&s) {
                                Some(0)
                            } else {
                                return Err(format!("Invalid timestamp format: '{s}'"));
                            }
                        }
                        x => return Err(format!("AS OF expects commit timestamp, got {x:?}")),
                    }
                } else {
                    None
                };
                let filter = self.optional_where()?;
                let mut order_by = Vec::new();
                if self.is_word("ORDER") {
                    self.pos += 1;
                    self.word("BY")?;
                    loop {
                        let c = self.ident()?;
                        let desc = if self.is_word("DESC") {
                            self.pos += 1;
                            true
                        } else {
                            if self.is_word("ASC") {
                                self.pos += 1;
                            }
                            false
                        };
                        order_by.push((c, desc));
                        if !self.comma() {
                            break;
                        }
                    }
                }
                let limit = if self.is_word("LIMIT") {
                    self.pos += 1;
                    match self.take() {
                        Some(Token::Number(n)) if n >= 0 => Some(n as usize),
                        x => return Err(format!("LIMIT must be non-negative number, got {x:?}")),
                    }
                } else {
                    None
                };
                let offset = if self.is_word("OFFSET") {
                    self.pos += 1;
                    match self.take() {
                        Some(Token::Number(n)) if n >= 0 => Some(n as usize),
                        x => return Err(format!("OFFSET expects number, got {x:?}")),
                    }
                } else {
                    None
                };
                Ok(Statement::Select {
                    columns,
                    table,
                    as_of,
                    filter,
                    order_by,
                    limit,
                    offset,
                })
            }
            "CHECKOUT" => Ok(Statement::Checkout {
                name: self.ident()?,
            }),
            "DIFF" => Ok(Statement::Diff {
                left: self.ident()?,
                right: self.ident()?,
            }),
            "MERGE" => {
                let source = self.ident()?;
                self.word("INTO")?;
                Ok(Statement::Merge {
                    source,
                    target: self.ident()?,
                })
            }
            "SHOW" => {
                self.word("HISTORY")?;
                let table = self.ident()?;
                let filter = self.optional_where()?;
                Ok(Statement::ShowHistory { table, filter })
            }
            "BISECT" => {
                let table = self.ident()?;
                self.word("WHERE")?;
                let filter = self.condition()?;
                Ok(Statement::Bisect { table, filter })
            }
            "WHAT" => {
                self.word("IF")?;
                match self.take() {
                    Some(Token::LParen) => (),
                    x => return Err(format!("expected ( after WHAT IF, got {x:?}")),
                }
                let mutation = self.parse_one_statement()?;
                match self.take() {
                    Some(Token::RParen) => (),
                    x => return Err(format!("expected ) after WHAT IF mutation, got {x:?}")),
                }
                let query = self.parse_one_statement()?;
                Ok(Statement::WhatIf {
                    mutation: Box::new(mutation),
                    query: Box::new(query),
                })
            }
            _ => Err(format!("unsupported statement {first}")),
        }
    }
}

pub fn parse(input: &str) -> Result<Statement, String> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() || tokens.iter().all(|t| *t == Token::Semi) {
        return Err("empty SQL statement".into());
    }
    let mut p = Parser::new(tokens);
    while p.peek() == Some(&Token::Semi) {
        p.pos += 1;
    }
    let stmt = p.parse_one_statement()?;
    while p.peek() == Some(&Token::Semi) {
        p.pos += 1;
    }
    if p.peek().is_some() {
        return Err(format!("unexpected token {:?}", p.peek()));
    }
    Ok(stmt)
}

pub fn parse_statements(input: &str) -> Result<Vec<Statement>, String> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() || tokens.iter().all(|t| *t == Token::Semi) {
        return Err("empty SQL statement".into());
    }
    let mut p = Parser::new(tokens);
    let mut stmts = Vec::new();
    while p.peek().is_some() {
        while p.peek() == Some(&Token::Semi) {
            p.pos += 1;
        }
        if p.peek().is_none() {
            break;
        }
        stmts.push(p.parse_one_statement()?);
        while p.peek() == Some(&Token::Semi) {
            p.pos += 1;
        }
    }
    Ok(stmts)
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => write!(f, "NULL"),
            Self::Bool(v) => write!(f, "{v}"),
            Self::Number(v) => write!(f, "{v}"),
            Self::Float(v) => write!(f, "{v}"),
            Self::Text(v) => write!(f, "{v}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_time_travel() {
        let s =
            parse("SELECT id FROM users AS OF 42 WHERE id >= 1 ORDER BY id DESC LIMIT 2").unwrap();
        assert!(matches!(
            s,
            Statement::Select {
                as_of: Some(42),
                limit: Some(2),
                ..
            }
        ));
        assert!(parse("SELECT * FROM t AS OF 'not-a-date'").is_err());
    }

    #[test]
    fn strings_preserve_case() {
        assert_eq!(
            tokenize("INSERT INTO t (x) VALUES ('Ada')")
                .unwrap()
                .iter()
                .find_map(|t| if let Token::String(s) = t {
                    Some(s.as_str())
                } else {
                    None
                }),
            Some("Ada")
        );
    }

    #[test]
    fn parses_multi_row_insert_and_pagination() {
        let insert = parse("INSERT INTO t (a,b) VALUES (1,2), (3,4), (5,6)").unwrap();
        assert!(
            matches!(insert, Statement::Insert { additional_rows, .. } if additional_rows.len() == 2)
        );
        let select = parse("SELECT * FROM t ORDER BY a DESC, b ASC LIMIT 10 OFFSET 20").unwrap();
        assert!(matches!(
            select,
            Statement::Select {
                order_by,
                limit: Some(10),
                offset: Some(20),
                ..
            } if order_by.len() == 2
        ));
    }

    #[test]
    fn parses_nested_boolean_and_predicate_forms() {
        let select = parse("SELECT * FROM t WHERE ((a=1 OR b=2) AND c=3) OR NOT active").unwrap();
        assert!(matches!(
            select,
            Statement::Select {
                filter: Some(Condition::Or(_, _)),
                ..
            }
        ));
        for sql in [
            "SELECT * FROM t WHERE x IN (1,2)",
            "SELECT * FROM t WHERE x BETWEEN 1 AND 3",
            "SELECT * FROM t WHERE x IS NOT NULL",
            "SELECT * FROM t WHERE ((((((((((a=1))))))))))",
        ] {
            assert!(parse(sql).is_ok(), "failed to parse {sql}");
        }
    }

    #[test]
    fn parses_float_negative_and_arithmetic_expressions() {
        assert!(parse("SELECT * FROM products WHERE price = 19.99").is_ok());
        assert!(parse("SELECT * FROM t WHERE price * quantity > 100").is_ok());
        assert!(parse("SELECT * FROM t WHERE (a + b) * (c - d) = 10").is_ok());
        assert!(parse("SELECT * FROM t WHERE x = -5").is_ok());
    }

    #[test]
    fn parses_transactions_and_computed_columns() {
        assert_eq!(parse("BEGIN").unwrap(), Statement::Begin);
        assert_eq!(parse("BEGIN TRANSACTION").unwrap(), Statement::Begin);
        assert_eq!(parse("COMMIT").unwrap(), Statement::Commit);
        assert_eq!(parse("ROLLBACK").unwrap(), Statement::Rollback);
        assert_eq!(parse("CHECKPOINT").unwrap(), Statement::Checkpoint);

        let select = parse("SELECT id, price * 1.1 AS new_price FROM products").unwrap();
        if let Statement::Select { columns, .. } = select {
            assert_eq!(columns.len(), 2);
            assert_eq!(columns[0], SelectColumn::Column("id".into()));
            assert!(
                matches!(columns[1], SelectColumn::AliasedExpr(_, ref a) if a == "new_price")
            );
        } else {
            panic!("expected select");
        }
    }

    #[test]
    fn comments_are_stripped() {
        assert_eq!(
            parse("-- comment\nSELECT * FROM t /* inline comment */").unwrap(),
            Statement::Select {
                columns: vec![SelectColumn::Wildcard],
                table: "t".into(),
                as_of: None,
                filter: None,
                order_by: vec![],
                limit: None,
                offset: None,
            }
        );
    }
}
