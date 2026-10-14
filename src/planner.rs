//! Logical-to-physical planning with primary-index selection and optimizer rules.
use crate::parser::{ColumnDef, Condition, Op, SelectColumn, Value};

#[derive(Clone, Debug, PartialEq)]
pub enum LogicalPlan {
    Scan {
        table: String,
    },
    IndexScan {
        table: String,
        key: i64,
    },
    IndexRangeScan {
        table: String,
        low: i64,
        high: i64,
    },
    Filter {
        predicate: Condition,
        input: Box<LogicalPlan>,
    },
    Project {
        columns: Vec<SelectColumn>,
        input: Box<LogicalPlan>,
    },
    Sort {
        keys: Vec<(String, bool)>,
        input: Box<LogicalPlan>,
    },
    Limit {
        count: usize,
        input: Box<LogicalPlan>,
    },
    Offset {
        count: usize,
        input: Box<LogicalPlan>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum PhysicalPlan {
    TableScan {
        table: String,
    },
    IndexScan {
        table: String,
        key: i64,
    },
    IndexRangeScan {
        table: String,
        low: i64,
        high: i64,
    },
    Filter {
        predicate: Condition,
        input: Box<PhysicalPlan>,
    },
    Projection {
        columns: Vec<SelectColumn>,
        input: Box<PhysicalPlan>,
    },
    Sort {
        keys: Vec<(String, bool)>,
        input: Box<PhysicalPlan>,
    },
    Limit {
        count: usize,
        input: Box<PhysicalPlan>,
    },
    Offset {
        count: usize,
        input: Box<PhysicalPlan>,
    },
}

pub fn optimize(plan: LogicalPlan, schema: &[ColumnDef], has_index: bool) -> LogicalPlan {
    match plan {
        LogicalPlan::Filter { predicate, input } => {
            if let LogicalPlan::Scan { table } = *input {
                if let Some(key) = primary_index_key(Some(&predicate), schema, has_index) {
                    return LogicalPlan::IndexScan { table, key };
                }
                if let Some((low, high)) = primary_index_range(Some(&predicate), schema, has_index) {
                    return LogicalPlan::IndexRangeScan { table, low, high };
                }
                LogicalPlan::Filter {
                    predicate,
                    input: Box::new(LogicalPlan::Scan { table }),
                }
            } else {
                LogicalPlan::Filter {
                    predicate,
                    input: Box::new(optimize(*input, schema, has_index)),
                }
            }
        }
        LogicalPlan::Project { columns, input } => LogicalPlan::Project {
            columns,
            input: Box::new(optimize(*input, schema, has_index)),
        },
        LogicalPlan::Sort { keys, input } => LogicalPlan::Sort {
            keys,
            input: Box::new(optimize(*input, schema, has_index)),
        },
        LogicalPlan::Limit { count, input } => LogicalPlan::Limit {
            count,
            input: Box::new(optimize(*input, schema, has_index)),
        },
        LogicalPlan::Offset { count, input } => LogicalPlan::Offset {
            count,
            input: Box::new(optimize(*input, schema, has_index)),
        },
        scan => scan,
    }
}

pub fn physical(plan: LogicalPlan) -> PhysicalPlan {
    match plan {
        LogicalPlan::Scan { table } => PhysicalPlan::TableScan { table },
        LogicalPlan::IndexScan { table, key } => PhysicalPlan::IndexScan { table, key },
        LogicalPlan::IndexRangeScan { table, low, high } => {
            PhysicalPlan::IndexRangeScan { table, low, high }
        }
        LogicalPlan::Filter { predicate, input } => PhysicalPlan::Filter {
            predicate,
            input: Box::new(physical(*input)),
        },
        LogicalPlan::Project { columns, input } => PhysicalPlan::Projection {
            columns,
            input: Box::new(physical(*input)),
        },
        LogicalPlan::Sort { keys, input } => PhysicalPlan::Sort {
            keys,
            input: Box::new(physical(*input)),
        },
        LogicalPlan::Limit { count, input } => PhysicalPlan::Limit {
            count,
            input: Box::new(physical(*input)),
        },
        LogicalPlan::Offset { count, input } => PhysicalPlan::Offset {
            count,
            input: Box::new(physical(*input)),
        },
    }
}

pub fn primary_index_key(
    filter: Option<&Condition>,
    schema: &[ColumnDef],
    available: bool,
) -> Option<i64> {
    if !available {
        return None;
    }
    let Condition::Compare(column, Op::Eq, Value::Number(key)) = filter? else {
        return None;
    };
    schema
        .iter()
        .find(|c| c.name == *column && c.primary_key)
        .map(|_| *key)
}

pub fn primary_index_range(
    filter: Option<&Condition>,
    schema: &[ColumnDef],
    available: bool,
) -> Option<(i64, i64)> {
    if !available {
        return None;
    }
    let Condition::Between(column, Value::Number(low), Value::Number(high)) = filter? else {
        return None;
    };
    schema
        .iter()
        .find(|c| c.name == *column && c.primary_key)
        .map(|_| (*low, *high))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_plan_lowers_to_physical() {
        let p = LogicalPlan::Project {
            columns: vec![SelectColumn::Column("id".into())],
            input: Box::new(LogicalPlan::Scan { table: "t".into() }),
        };
        assert_eq!(
            physical(optimize(p, &[], false)),
            PhysicalPlan::Projection {
                columns: vec![SelectColumn::Column("id".into())],
                input: Box::new(PhysicalPlan::TableScan { table: "t".into() })
            }
        );
    }

    #[test]
    fn selects_index_only_for_primary_key_equality() {
        let schema = vec![ColumnDef {
            name: "id".into(),
            data_type: crate::parser::ColumnType::Integer,
            primary_key: true,
            not_null: true,
        }];
        assert_eq!(
            primary_index_key(
                Some(&Condition::Compare("id".into(), Op::Eq, Value::Number(4))),
                &schema,
                true
            ),
            Some(4)
        );
        assert_eq!(
            primary_index_key(
                Some(&Condition::Compare("id".into(), Op::Gt, Value::Number(4))),
                &schema,
                true
            ),
            None
        );
    }

    #[test]
    fn selects_index_range_for_between() {
        let schema = vec![ColumnDef {
            name: "id".into(),
            data_type: crate::parser::ColumnType::Integer,
            primary_key: true,
            not_null: true,
        }];
        assert_eq!(
            primary_index_range(
                Some(&Condition::Between("id".into(), Value::Number(10), Value::Number(20))),
                &schema,
                true
            ),
            Some((10, 20))
        );
    }
}
