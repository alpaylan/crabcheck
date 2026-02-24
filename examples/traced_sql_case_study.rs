#[cfg(feature = "tracing")]
use std::collections::HashMap;

#[cfg(feature = "tracing")]
use crabcheck::tracing::{
    ChoiceTrace,
    Id,
    TraceError,
    TraceRunner,
};

#[cfg(feature = "tracing")]
const NUM_TABLES: usize = 4;
#[cfg(feature = "tracing")]
const MAX_COLS: usize = 3;
#[cfg(feature = "tracing")]
const MAX_ROWS: usize = 4;
#[cfg(feature = "tracing")]
const MAX_QUERIES_PER_TABLE: usize = 5;
#[cfg(feature = "tracing")]
const MAX_EXPR_DEPTH: usize = 2;

#[cfg(feature = "tracing")]
#[derive(Clone, Debug, PartialEq, Eq)]
struct Table {
    present: bool,
    cols: usize,
    rows: Vec<Vec<i32>>,
}

#[cfg(feature = "tracing")]
#[derive(Clone, Debug, PartialEq, Eq)]
enum Expr {
    Col(usize),
    Lit(i32),
    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Neg(Box<Expr>),
}

#[cfg(feature = "tracing")]
#[derive(Clone, Debug, PartialEq, Eq)]
enum QueryKind {
    Count,
    Sum(Expr),
    Max(Expr),
}

#[cfg(feature = "tracing")]
#[derive(Clone, Debug, PartialEq, Eq)]
struct Query {
    table: usize,
    kind: QueryKind,
}

#[cfg(feature = "tracing")]
#[derive(Clone, Debug, PartialEq, Eq)]
struct Script {
    tables: Vec<Table>,
    queries: Vec<Query>,
}

#[cfg(feature = "tracing")]
#[derive(Clone, Debug, PartialEq, Eq)]
enum QueryOutput {
    Count(usize),
    Sum(i32),
    Max(Option<i32>),
}

#[cfg(feature = "tracing")]
fn main() {
    let mut tested = 0usize;
    let mut seeds_with_present_tables = 0usize;

    for seed in 0..500_u64 {
        let (script, trace, table_ids) =
            record_script(seed).expect("recording case-study script should succeed");

        let mut any_present = false;
        for table_idx in 0..NUM_TABLES {
            if !script.tables[table_idx].present {
                continue;
            }
            any_present = true;
            tested += 1;
            check_hypothesis(seed, &script, &trace, &table_ids, table_idx).unwrap_or_else(|err| {
                panic!("hypothesis check failed for seed {} table {}: {}", seed, table_idx, err)
            });
        }

        if any_present {
            seeds_with_present_tables += 1;
        }
    }

    println!("== Traced SQL Case Study ==");
    println!("tested removals: {}", tested);
    println!("seeds with at least one present table: {}", seeds_with_present_tables);
    println!("result: all checked removals preserved other-table query behavior");
}

#[cfg(feature = "tracing")]
fn record_script(seed: u64) -> Result<(Script, ChoiceTrace, Vec<Id>), TraceError> {
    let mut runner = TraceRunner::recording(seed);
    let (script, table_ids) = build_script(&mut runner)?;
    Ok((script, runner.finish(), table_ids))
}

#[cfg(feature = "tracing")]
fn replay_script(
    seed: u64,
    plan: &ChoiceTrace,
) -> Result<(Script, ChoiceTrace, Vec<Id>), TraceError> {
    let mut runner = TraceRunner::replay(seed, plan);
    let (script, table_ids) = build_script(&mut runner)?;
    Ok((script, runner.finish(), table_ids))
}

#[cfg(feature = "tracing")]
fn build_script(runner: &mut TraceRunner) -> Result<(Script, Vec<Id>), TraceError> {
    let mut tables = Vec::with_capacity(NUM_TABLES);
    let mut queries = Vec::new();
    let mut table_ids = Vec::with_capacity(NUM_TABLES);

    for table_idx in 0..NUM_TABLES {
        let present = runner.choose_usize(0.into(), 1.into(), &[])?;
        table_ids.push(present.id);

        let cols = runner.choose_usize(1.into(), MAX_COLS.into(), &[present.id])?;
        let rows = runner.choose_usize(0.into(), MAX_ROWS.into(), &[present.id])?;

        let mut cell_grid = vec![vec![0_i32; MAX_COLS]; MAX_ROWS];
        for row in 0..MAX_ROWS {
            for col in 0..MAX_COLS {
                let cell = runner.choose_usize(0.into(), 20.into(), &[present.id])?;
                cell_grid[row][col] = map_small_i32(cell.value);
            }
        }

        let query_count =
            runner.choose_usize(0.into(), MAX_QUERIES_PER_TABLE.into(), &[present.id])?;
        for q in 0..MAX_QUERIES_PER_TABLE {
            let kind = runner.choose_usize(0.into(), 2.into(), &[present.id])?;
            let expr = build_expr(
                runner,
                MAX_EXPR_DEPTH,
                cols.value,
                &[present.id, cols.id, kind.id],
            )?;

            if present.value == 1 && q < query_count.value {
                let kind = match kind.value {
                    0 => QueryKind::Count,
                    1 => QueryKind::Sum(expr.clone()),
                    _ => QueryKind::Max(expr.clone()),
                };
                queries.push(Query { table: table_idx, kind });
            }
        }

        let table = if present.value == 1 {
            let active_rows = (0..rows.value)
                .map(|r| (0..cols.value).map(|c| cell_grid[r][c]).collect::<Vec<_>>())
                .collect::<Vec<_>>();
            Table { present: true, cols: cols.value, rows: active_rows }
        } else {
            Table { present: false, cols: cols.value, rows: vec![] }
        };

        tables.push(table);
    }

    Ok((Script { tables, queries }, table_ids))
}

#[cfg(feature = "tracing")]
fn map_small_i32(value: usize) -> i32 {
    value as i32 - 10
}

#[cfg(feature = "tracing")]
fn build_expr(
    runner: &mut TraceRunner,
    depth: usize,
    cols: usize,
    deps: &[Id],
) -> Result<Expr, TraceError> {
    let op = runner.choose_usize(0.into(), 5.into(), deps)?;
    let col = runner.choose_usize(0.into(), (MAX_COLS - 1).into(), &[op.id])?;
    let lit = runner.choose_usize(0.into(), 20.into(), &[op.id])?;
    let base_col = Expr::Col(col.value % cols.max(1));
    let base_lit = Expr::Lit(map_small_i32(lit.value));

    if depth == 0 {
        return Ok(match op.value {
            0 | 2 | 4 => base_col,
            _ => base_lit,
        });
    }

    let left = build_expr(runner, depth - 1, cols, &[op.id])?;
    let right = build_expr(runner, depth - 1, cols, &[op.id])?;
    Ok(match op.value {
        0 => base_col,
        1 => base_lit,
        2 => Expr::Add(Box::new(left), Box::new(right)),
        3 => Expr::Sub(Box::new(left), Box::new(right)),
        4 => Expr::Mul(Box::new(left), Box::new(right)),
        _ => Expr::Neg(Box::new(left)),
    })
}

#[cfg(feature = "tracing")]
fn eval_expr(expr: &Expr, row: &[i32]) -> i32 {
    match expr {
        Expr::Col(col) => row[*col],
        Expr::Lit(value) => *value,
        Expr::Add(left, right) => eval_expr(left, row) + eval_expr(right, row),
        Expr::Sub(left, right) => eval_expr(left, row) - eval_expr(right, row),
        Expr::Mul(left, right) => eval_expr(left, row) * eval_expr(right, row),
        Expr::Neg(inner) => -eval_expr(inner, row),
    }
}

#[cfg(feature = "tracing")]
fn execute(script: &Script) -> HashMap<usize, Vec<QueryOutput>> {
    let mut out: HashMap<usize, Vec<QueryOutput>> = HashMap::new();

    for query in &script.queries {
        let table = &script.tables[query.table];
        let result = match query.kind {
            QueryKind::Count => QueryOutput::Count(table.rows.len()),
            QueryKind::Sum(ref expr) => {
                let sum = table.rows.iter().map(|row| eval_expr(expr, row)).sum::<i32>();
                QueryOutput::Sum(sum)
            },
            QueryKind::Max(ref expr) => {
                let max = table.rows.iter().map(|row| eval_expr(expr, row)).max();
                QueryOutput::Max(max)
            },
        };

        out.entry(query.table).or_default().push(result);
    }

    out
}

#[cfg(feature = "tracing")]
fn check_hypothesis(
    seed: u64,
    original: &Script,
    trace: &ChoiceTrace,
    table_ids: &[Id],
    removed_table: usize,
) -> Result<(), String> {
    let removed_id = table_ids[removed_table];
    let candidate = trace.remove_with_dependents(removed_id);

    let (replayed, _replayed_trace, _replayed_table_ids) =
        replay_script(seed, &candidate).map_err(|e| format!("replay failed: {:?}", e))?;

    if replayed.tables[removed_table].present {
        return Err(format!(
            "table {} still present after removing id {:?}",
            removed_table, removed_id
        ));
    }

    if replayed.queries.iter().any(|query| query.table == removed_table) {
        return Err(format!("found query mentioning removed table {}", removed_table));
    }

    let original_outputs = execute(original);
    let replayed_outputs = execute(&replayed);

    for table_idx in 0..NUM_TABLES {
        if table_idx == removed_table {
            continue;
        }

        if original.tables[table_idx] != replayed.tables[table_idx] {
            return Err(format!(
                "table {} changed after removing table {}",
                table_idx, removed_table
            ));
        }

        let left = original_outputs.get(&table_idx).cloned().unwrap_or_default();
        let right = replayed_outputs.get(&table_idx).cloned().unwrap_or_default();
        if left != right {
            return Err(format!(
                "query outputs for table {} changed after removing table {}",
                table_idx, removed_table
            ));
        }
    }

    Ok(())
}

#[cfg(not(feature = "tracing"))]
fn main() {
    eprintln!("Run with: cargo run --example traced_sql_case_study --features tracing");
}
