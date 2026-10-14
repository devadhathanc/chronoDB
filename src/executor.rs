//! Volcano-style pull operators used by physical query execution.
use crate::parser::Value;

pub type Row = Vec<Value>;

pub trait Operator {
    fn open(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn next(&mut self) -> Result<Option<Row>, String>;
    fn close(&mut self) -> Result<(), String> {
        Ok(())
    }
}

pub struct Values {
    rows: Vec<Row>,
    cursor: usize,
}

impl Values {
    pub fn new(rows: Vec<Row>) -> Self {
        Self { rows, cursor: 0 }
    }
}

impl Operator for Values {
    fn next(&mut self) -> Result<Option<Row>, String> {
        let row = self.rows.get(self.cursor).cloned();
        self.cursor += usize::from(row.is_some());
        Ok(row)
    }
}

pub struct Filter {
    input: Box<dyn Operator>,
    predicate: Box<dyn Fn(&Row) -> bool>,
}

impl Filter {
    pub fn new(input: Box<dyn Operator>, predicate: impl Fn(&Row) -> bool + 'static) -> Self {
        Self {
            input,
            predicate: Box::new(predicate),
        }
    }
}

impl Operator for Filter {
    fn open(&mut self) -> Result<(), String> {
        self.input.open()
    }
    fn next(&mut self) -> Result<Option<Row>, String> {
        loop {
            match self.input.next()? {
                Some(r) if (self.predicate)(&r) => return Ok(Some(r)),
                Some(_) => {}
                None => return Ok(None),
            }
        }
    }
    fn close(&mut self) -> Result<(), String> {
        self.input.close()
    }
}

pub struct Project {
    input: Box<dyn Operator>,
    evaluators: Vec<Box<dyn Fn(&Row) -> Result<Value, String>>>,
}

impl Project {
    pub fn new(input: Box<dyn Operator>, columns: Vec<usize>) -> Self {
        let evaluators = columns
            .into_iter()
            .map(|i| {
                let f: Box<dyn Fn(&Row) -> Result<Value, String>> = Box::new(move |r: &Row| {
                    r.get(i)
                        .cloned()
                        .ok_or_else(|| format!("projection index {i} is out of range"))
                });
                f
            })
            .collect();
        Self { input, evaluators }
    }

    pub fn with_evaluators(
        input: Box<dyn Operator>,
        evaluators: Vec<Box<dyn Fn(&Row) -> Result<Value, String>>>,
    ) -> Self {
        Self { input, evaluators }
    }
}

impl Operator for Project {
    fn open(&mut self) -> Result<(), String> {
        self.input.open()
    }
    fn next(&mut self) -> Result<Option<Row>, String> {
        match self.input.next()? {
            Some(r) => {
                let mut row = Vec::with_capacity(self.evaluators.len());
                for eval in &self.evaluators {
                    row.push(eval(&r)?);
                }
                Ok(Some(row))
            }
            None => Ok(None),
        }
    }
    fn close(&mut self) -> Result<(), String> {
        self.input.close()
    }
}

pub fn compare_values(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::Null, Value::Null) => Some(std::cmp::Ordering::Equal),
        (Value::Null, _) => Some(std::cmp::Ordering::Less),
        (_, Value::Null) => Some(std::cmp::Ordering::Greater),
        (Value::Number(a), Value::Number(b)) => Some(a.cmp(b)),
        (Value::Text(a), Value::Text(b)) => Some(a.cmp(b)),
        (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
        (Value::Float(a), Value::Float(b)) => a.partial_cmp(b),
        (Value::Number(a), Value::Float(b)) => (*a as f64).partial_cmp(b),
        (Value::Float(a), Value::Number(b)) => a.partial_cmp(&(*b as f64)),
        _ => None,
    }
}

pub struct Sort {
    input: Box<dyn Operator>,
    keys: Vec<(usize, bool)>,
    rows: Vec<Row>,
    cursor: usize,
    loaded: bool,
}

impl Sort {
    pub fn new(input: Box<dyn Operator>, key: usize, descending: bool) -> Self {
        Self {
            input,
            keys: vec![(key, descending)],
            rows: vec![],
            cursor: 0,
            loaded: false,
        }
    }

    pub fn new_multi(input: Box<dyn Operator>, keys: Vec<(usize, bool)>) -> Self {
        Self {
            input,
            keys,
            rows: vec![],
            cursor: 0,
            loaded: false,
        }
    }
}

impl Operator for Sort {
    fn open(&mut self) -> Result<(), String> {
        self.input.open()?;
        self.rows.clear();
        self.cursor = 0;
        self.loaded = false;
        Ok(())
    }
    fn next(&mut self) -> Result<Option<Row>, String> {
        if !self.loaded {
            while let Some(r) = self.input.next()? {
                for &(key, _) in &self.keys {
                    if key >= r.len() {
                        return Err("sort key is out of range".into());
                    }
                }
                self.rows.push(r);
            }
            self.rows.sort_by(|a, b| {
                for &(key, desc) in &self.keys {
                    let va = a.get(key).unwrap_or(&Value::Null);
                    let vb = b.get(key).unwrap_or(&Value::Null);
                    let ord = compare_values(va, vb).unwrap_or(std::cmp::Ordering::Equal);
                    if ord != std::cmp::Ordering::Equal {
                        return if desc { ord.reverse() } else { ord };
                    }
                }
                std::cmp::Ordering::Equal
            });
            self.loaded = true;
        }
        let row = self.rows.get(self.cursor).cloned();
        self.cursor += usize::from(row.is_some());
        Ok(row)
    }
    fn close(&mut self) -> Result<(), String> {
        self.input.close()
    }
}

pub struct Offset {
    input: Box<dyn Operator>,
    remaining: usize,
}

impl Offset {
    pub fn new(input: Box<dyn Operator>, offset: usize) -> Self {
        Self {
            input,
            remaining: offset,
        }
    }
}

impl Operator for Offset {
    fn open(&mut self) -> Result<(), String> {
        self.input.open()
    }
    fn next(&mut self) -> Result<Option<Row>, String> {
        while self.remaining > 0 {
            if self.input.next()?.is_none() {
                return Ok(None);
            }
            self.remaining -= 1;
        }
        self.input.next()
    }
    fn close(&mut self) -> Result<(), String> {
        self.input.close()
    }
}

pub struct Limit {
    input: Box<dyn Operator>,
    remaining: usize,
}

impl Limit {
    pub fn new(input: Box<dyn Operator>, limit: usize) -> Self {
        Self {
            input,
            remaining: limit,
        }
    }
}

impl Operator for Limit {
    fn open(&mut self) -> Result<(), String> {
        self.input.open()
    }
    fn next(&mut self) -> Result<Option<Row>, String> {
        if self.remaining == 0 {
            return Ok(None);
        }
        let row = self.input.next()?;
        if row.is_some() {
            self.remaining -= 1;
        }
        Ok(row)
    }
    fn close(&mut self) -> Result<(), String> {
        self.input.close()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iterator_pipeline_filters_sorts_projects_and_limits() {
        let rows = vec![
            vec![Value::Number(3), Value::Text("c".into())],
            vec![Value::Number(1), Value::Text("a".into())],
            vec![Value::Number(2), Value::Text("b".into())],
        ];
        let source = Box::new(Values::new(rows));
        let filter = Box::new(Filter::new(
            source,
            |r| matches!(r[0], Value::Number(n) if n > 1),
        ));
        let sort = Box::new(Sort::new(filter, 0, true));
        let project = Box::new(Project::new(sort, vec![1]));
        let mut limit = Limit::new(project, 1);
        limit.open().unwrap();
        assert_eq!(limit.next().unwrap(), Some(vec![Value::Text("c".into())]));
        assert_eq!(limit.next().unwrap(), None);
    }
}
