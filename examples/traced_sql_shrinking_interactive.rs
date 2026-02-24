#[cfg(feature = "tracing")]
use std::collections::HashSet;
#[cfg(feature = "tracing")]
use std::io::{
    self,
    Write,
};

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
const MAX_STMTS: usize = 16;
#[cfg(feature = "tracing")]
const MAX_EXPR_DEPTH: usize = 2;
#[cfg(feature = "tracing")]
const MIN_START_STATEMENTS: usize = 6;

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
enum SelectKind {
    Count,
    Sum(Expr),
    Max(Expr),
}

#[cfg(feature = "tracing")]
#[derive(Clone, Debug, PartialEq, Eq)]
enum Statement {
    Create { table: usize, cols: usize },
    Insert { table: usize, values: Vec<i32> },
    Select { table: usize, kind: SelectKind },
}

#[cfg(feature = "tracing")]
#[derive(Clone, Debug, PartialEq, Eq)]
struct Script {
    statements: Vec<Statement>,
}

#[cfg(feature = "tracing")]
#[derive(Clone, Debug)]
enum StepAction {
    RemoveDecision { id: Id, removed: Vec<Id> },
}

#[cfg(feature = "tracing")]
fn main() {
    let opts = match parse_args() {
        Ok(opts) => opts,
        Err(err) => {
            eprintln!("{err}");
            eprintln!(
                "usage: cargo run --example traced_sql_shrinking_interactive --features tracing -- [--seed N] [--auto]"
            );
            return;
        },
    };

    let start = match pick_start(opts.seed) {
        Ok(start) => start,
        Err(err) => {
            eprintln!("{err}");
            return;
        },
    };

    let mut current_trace = start.trace;
    let seed = start.seed;
    let mut current_script = start.script;

    println!("== Traced SQL Shrinking (Interactive) ==");
    println!("seed={seed}");
    println!("trace_len={}", current_trace.len());
    println!();
    print_script_block("Current SQL", &current_script);
    println!();
    print_help();

    let mut auto = opts.auto;
    let mut step = 0usize;
    loop {
        if !auto {
            match prompt() {
                Prompt::Next => {},
                Prompt::Auto => auto = true,
                Prompt::Quit => {
                    println!("stopped at step {step}");
                    break;
                },
            }
        }

        let Some((next_trace, action)) = shrink_one_step(seed, &current_trace) else {
            println!("no more removable decisions that replay successfully");
            break;
        };
        let (next_script, _replayed_trace) =
            replay_script(seed, &next_trace).expect("accepted shrink step must replay");

        step += 1;
        println!();
        println!("--- step {step} ---");
        print_action(&action);
        println!("trace_len: {} -> {}", current_trace.len(), next_trace.len());

        let current_before = render_sql(&current_script);
        let current_after = render_sql(&next_script);
        let old_lines = current_before.len();
        let new_lines = current_after.len();
        println!("current SQL statements: {old_lines} -> {new_lines}");
        print_sql_diff("Current SQL diff", &current_before, &current_after);
        print_script_block("Current SQL", &next_script);

        current_trace = next_trace;
        current_script = next_script;
    }

    println!();
    println!("final trace_len={}", current_trace.len());
}

#[cfg(feature = "tracing")]
struct Opts {
    seed: Option<u64>,
    auto: bool,
}

#[cfg(feature = "tracing")]
struct Start {
    seed: u64,
    trace: ChoiceTrace,
    script: Script,
}

#[cfg(feature = "tracing")]
fn parse_args() -> Result<Opts, String> {
    let mut seed = None;
    let mut auto = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--seed" => {
                let value = args.next().ok_or_else(|| "--seed requires a value".to_string())?;
                let parsed =
                    value.parse::<u64>().map_err(|_| format!("invalid seed value: {value}"))?;
                seed = Some(parsed);
            },
            "--auto" => auto = true,
            _ => return Err(format!("unrecognized argument: {arg}")),
        }
    }

    Ok(Opts { seed, auto })
}

#[cfg(feature = "tracing")]
fn pick_start(seed: Option<u64>) -> Result<Start, String> {
    match seed {
        Some(seed) => {
            let (script, trace) = record_script(seed).map_err(|e| format!("{e:?}"))?;
            Ok(Start { seed, trace, script })
        },
        None => {
            for seed in 0..10_000_u64 {
                let Ok((script, trace)) = record_script(seed) else {
                    continue;
                };
                if has_sql_content(&script) {
                    return Ok(Start { seed, trace, script });
                }
            }
            Err("could not find a suitable seed in 0..10000".to_string())
        },
    }
}

#[cfg(feature = "tracing")]
fn has_sql_content(script: &Script) -> bool {
    script.statements.len() >= MIN_START_STATEMENTS
}

#[cfg(feature = "tracing")]
fn shrink_one_step(seed: u64, current: &ChoiceTrace) -> Option<(ChoiceTrace, StepAction)> {
    let current_ids = current.decisions().iter().map(|d| d.id).collect::<Vec<_>>();
    for id in &current_ids {
        let candidate = current.remove_with_dependents(*id);
        if candidate.len() >= current.len() || !candidate.is_well_formed() {
            continue;
        }
        if replay_script(seed, &candidate).is_ok() {
            let removed = removed_ids(current, &candidate);
            return Some((candidate, StepAction::RemoveDecision { id: *id, removed }));
        }
    }

    None
}

#[cfg(feature = "tracing")]
fn removed_ids(before: &ChoiceTrace, after: &ChoiceTrace) -> Vec<Id> {
    let before_ids = before.decisions().iter().map(|decision| decision.id).collect::<HashSet<_>>();
    let after_ids = after.decisions().iter().map(|decision| decision.id).collect::<HashSet<_>>();
    let mut removed = before_ids.difference(&after_ids).copied().collect::<Vec<_>>();
    removed.sort_unstable();
    removed
}

#[cfg(feature = "tracing")]
fn print_action(action: &StepAction) {
    match action {
        StepAction::RemoveDecision { id, removed } => {
            println!("action: remove id {id:?}, cascade removed {}", removed.len());
            println!("removed ids: {removed:?}");
        },
    }
}

#[cfg(feature = "tracing")]
fn print_script_block(title: &str, script: &Script) {
    println!("{title}:");
    let lines = render_sql(script);
    for (i, line) in lines.iter().enumerate() {
        println!("{:>3}. {line}", i + 1);
    }
    println!();
}

#[cfg(feature = "tracing")]
fn render_sql(script: &Script) -> Vec<String> {
    let mut lines = Vec::new();

    for statement in &script.statements {
        match statement {
            Statement::Create { table, cols } => {
                let mut col_defs = Vec::with_capacity(*cols);
                for c in 0..*cols {
                    col_defs.push(format!("c{c} INT"));
                }
                lines.push(format!("CREATE TABLE t{table} ({});", col_defs.join(", ")));
            },
            Statement::Insert { table, values } => {
                let payload = values.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ");
                lines.push(format!("INSERT INTO t{table} VALUES ({payload});"));
            },
            Statement::Select { table, kind } => {
                let expr = match kind {
                    SelectKind::Count => "COUNT(*)".to_string(),
                    SelectKind::Sum(expr) => format!("SUM({})", render_expr(expr)),
                    SelectKind::Max(expr) => format!("MAX({})", render_expr(expr)),
                };
                lines.push(format!("SELECT {expr} FROM t{table};"));
            },
        }
    }

    if lines.is_empty() {
        lines.push("-- <empty script>".to_string());
    }
    lines
}

#[cfg(feature = "tracing")]
fn render_expr(expr: &Expr) -> String {
    match expr {
        Expr::Col(col) => format!("c{col}"),
        Expr::Lit(value) => value.to_string(),
        Expr::Add(left, right) => format!("({} + {})", render_expr(left), render_expr(right)),
        Expr::Sub(left, right) => format!("({} - {})", render_expr(left), render_expr(right)),
        Expr::Mul(left, right) => format!("({} * {})", render_expr(left), render_expr(right)),
        Expr::Neg(inner) => format!("(-{})", render_expr(inner)),
    }
}

#[cfg(feature = "tracing")]
#[derive(Clone, Debug)]
enum DiffLine {
    Keep(String),
    Remove(String),
    Add(String),
}

#[cfg(feature = "tracing")]
fn print_sql_diff(title: &str, before: &[String], after: &[String]) {
    let diff = sql_line_diff(before, after);
    let removed = diff.iter().filter(|entry| matches!(entry, DiffLine::Remove(_))).count();
    let added = diff.iter().filter(|entry| matches!(entry, DiffLine::Add(_))).count();

    println!("{title} (removed {removed}, added {added}):");
    if removed == 0 && added == 0 {
        println!("  (no statement changes)");
        println!();
        return;
    }

    for entry in diff {
        match entry {
            DiffLine::Keep(line) => {
                let _ = line;
            },
            DiffLine::Remove(line) => println!("  - {line}"),
            DiffLine::Add(line) => println!("  + {line}"),
        }
    }
    println!();
}

#[cfg(feature = "tracing")]
fn sql_line_diff(before: &[String], after: &[String]) -> Vec<DiffLine> {
    let n = before.len();
    let m = after.len();
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];

    for i in (0..n).rev() {
        for j in (0..m).rev() {
            if before[i] == after[j] {
                lcs[i][j] = lcs[i + 1][j + 1] + 1;
            } else {
                lcs[i][j] = lcs[i + 1][j].max(lcs[i][j + 1]);
            }
        }
    }

    let mut i = 0usize;
    let mut j = 0usize;
    let mut diff = Vec::new();
    while i < n && j < m {
        if before[i] == after[j] {
            diff.push(DiffLine::Keep(before[i].clone()));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            diff.push(DiffLine::Remove(before[i].clone()));
            i += 1;
        } else {
            diff.push(DiffLine::Add(after[j].clone()));
            j += 1;
        }
    }

    while i < n {
        diff.push(DiffLine::Remove(before[i].clone()));
        i += 1;
    }
    while j < m {
        diff.push(DiffLine::Add(after[j].clone()));
        j += 1;
    }

    diff
}

#[cfg(feature = "tracing")]
fn print_help() {
    println!("controls:");
    println!("  [enter] next shrink step");
    println!("  a       run all remaining steps");
    println!("  q       quit");
}

#[cfg(feature = "tracing")]
enum Prompt {
    Next,
    Auto,
    Quit,
}

#[cfg(feature = "tracing")]
fn prompt() -> Prompt {
    print!("shrink> ");
    let _ = io::stdout().flush();
    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_err() {
        return Prompt::Quit;
    }
    match line.trim() {
        "" => Prompt::Next,
        "a" | "auto" => Prompt::Auto,
        "q" | "quit" => Prompt::Quit,
        _ => Prompt::Next,
    }
}

#[cfg(feature = "tracing")]
fn record_script(seed: u64) -> Result<(Script, ChoiceTrace), TraceError> {
    let mut runner = TraceRunner::recording(seed);
    let script = build_script(&mut runner)?;
    Ok((script, runner.finish()))
}

#[cfg(feature = "tracing")]
fn replay_script(seed: u64, plan: &ChoiceTrace) -> Result<(Script, ChoiceTrace), TraceError> {
    let mut runner = TraceRunner::replay(seed, plan);
    let script = build_script(&mut runner)?;
    Ok((script, runner.finish()))
}

#[cfg(feature = "tracing")]
fn build_script(runner: &mut TraceRunner) -> Result<Script, TraceError> {
    let mut statements = Vec::new();
    let mut table_cols = [1usize; NUM_TABLES];
    let mut table_create_dep = [None; NUM_TABLES];

    for _slot in 0..MAX_STMTS {
        let active = runner.choose_usize(0.into(), 1.into(), &[])?;
        let table_choice = runner.choose_usize(0.into(), (NUM_TABLES - 1).into(), &[active.id])?;
        let table = table_choice.value;
        let create_dep = table_create_dep[table];

        let mut action_deps = vec![table_choice.id];
        let action_hi = if let Some(dep) = create_dep {
            action_deps.push(dep);
            2usize
        } else {
            1usize
        };
        let action = runner.choose_usize(0.into(), action_hi.into(), &action_deps)?;

        if active.value == 0 {
            continue;
        }

        match create_dep {
            None => {
                if action.value == 1 {
                    let cols = runner.choose_usize(1.into(), MAX_COLS.into(), &[action.id])?;
                    statements.push(Statement::Create { table, cols: cols.value });
                    table_cols[table] = cols.value;
                    table_create_dep[table] = Some(action.id);
                }
            },
            Some(dep) => match action.value {
                0 => {},
                1 => {
                    let cols = table_cols[table];
                    let mut values = Vec::with_capacity(cols);
                    for c in 0..MAX_COLS {
                        let cell = runner.choose_usize(0.into(), 20.into(), &[action.id, dep])?;
                        if c < cols {
                            values.push(map_small_i32(cell.value));
                        }
                    }
                    statements.push(Statement::Insert { table, values });
                },
                _ => {
                    let kind = runner.choose_usize(0.into(), 2.into(), &[action.id, dep])?;
                    let expr = build_expr(
                        runner,
                        MAX_EXPR_DEPTH,
                        table_cols[table],
                        &[action.id, dep, kind.id],
                    )?;
                    let kind = match kind.value {
                        0 => SelectKind::Count,
                        1 => SelectKind::Sum(expr),
                        _ => SelectKind::Max(expr),
                    };
                    statements.push(Statement::Select { table, kind });
                },
            },
        }
    }

    Ok(Script { statements })
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

#[cfg(not(feature = "tracing"))]
fn main() {
    eprintln!(
        "Run with: cargo run --example traced_sql_shrinking_interactive --features tracing -- [--seed N]"
    );
}
