#[cfg(feature = "tracing")]
use std::collections::{
    HashMap,
    HashSet,
};
#[cfg(feature = "tracing")]
use std::io::{
    self,
    Write,
};

#[cfg(feature = "tracing")]
use crabcheck::tracing::{
    ChoiceTrace,
    Id,
    Operand,
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
const SLOT_STRIDE: usize = 1_000;

#[cfg(feature = "tracing")]
fn sid_stmt_count() -> Id {
    Id(1)
}

#[cfg(feature = "tracing")]
fn sid_slot_base(slot: usize) -> usize {
    2 + slot * SLOT_STRIDE
}

#[cfg(feature = "tracing")]
fn sid_slot_table(slot: usize) -> Id {
    Id(sid_slot_base(slot))
}

#[cfg(feature = "tracing")]
fn sid_slot_create_cols(slot: usize) -> Id {
    Id(sid_slot_base(slot) + 1)
}

#[cfg(feature = "tracing")]
fn sid_slot_action(slot: usize) -> Id {
    Id(sid_slot_base(slot) + 2)
}

#[cfg(feature = "tracing")]
fn sid_slot_insert_cell(slot: usize, col: usize) -> Id {
    Id(sid_slot_base(slot) + 10 + col)
}

#[cfg(feature = "tracing")]
fn sid_slot_select_kind(slot: usize) -> Id {
    Id(sid_slot_base(slot) + 20)
}

#[cfg(feature = "tracing")]
fn sid_expr(slot: usize, node: usize, field: usize) -> Id {
    // field: 0=op, 1=col, 2=lit
    Id(sid_slot_base(slot) + 100 + node * 10 + field)
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
struct StatementRecord {
    statement: Statement,
    origins: Vec<Id>,
}

#[cfg(feature = "tracing")]
#[derive(Clone, Debug, PartialEq, Eq)]
struct Script {
    statements: Vec<StatementRecord>,
}

#[cfg(feature = "tracing")]
#[derive(Clone, Debug)]
struct Preview {
    action: &'static str,
    candidate: Option<ChoiceTrace>,
    removed_ids: Vec<Id>,
    error: Option<String>,
}

#[cfg(feature = "tracing")]
struct App {
    seed: u64,
    trace: ChoiceTrace,
    script: Script,
    productions: HashMap<Id, String>,
    selected: usize,
    history: Vec<ChoiceTrace>,
    status: String,
}

#[cfg(feature = "tracing")]
fn main() {
    let opts = match parse_args() {
        Ok(opts) => opts,
        Err(err) => {
            eprintln!("{err}");
            eprintln!(
                "usage: cargo run --example traced_sql_shrinking_tui --features tracing -- [--seed N]"
            );
            return;
        },
    };

    let (seed, trace, script, productions) = match pick_start(opts.seed) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("{err}");
            return;
        },
    };

    let mut app = App {
        seed,
        trace,
        script,
        productions,
        selected: 0,
        history: Vec::new(),
        status: "ready".to_string(),
    };

    loop {
        if let Err(err) = render(&app) {
            eprintln!("render error: {err}");
            return;
        }

        let mut line = String::new();
        if io::stdin().read_line(&mut line).is_err() {
            return;
        }
        let cmd = line.trim();

        if cmd == "q" || cmd == "quit" {
            break;
        }
        if cmd == "j" || cmd == "down" {
            if app.selected + 1 < app.trace.len() {
                app.selected += 1;
            }
            continue;
        }
        if cmd == "k" || cmd == "up" {
            app.selected = app.selected.saturating_sub(1);
            continue;
        }
        if cmd == "u" || cmd == "undo" {
            if let Some(prev) = app.history.pop() {
                app.trace = prev;
                match replay_script(app.seed, &app.trace) {
                    Ok((script, productions, _)) => {
                        app.script = script;
                        app.productions = productions;
                        app.selected = app.selected.min(app.trace.len().saturating_sub(1));
                        app.status = "undid previous step".to_string();
                    },
                    Err(err) => {
                        app.status = format!("undo replay failed: {:?}", err);
                    },
                }
            } else {
                app.status = "nothing to undo".to_string();
            }
            continue;
        }
        if cmd == "r" || cmd == "remove" {
            apply_action(&mut app, ActionKind::Remove);
            continue;
        }
        if cmd == "s" || cmd == "shrink" {
            apply_action(&mut app, ActionKind::ShrinkPicked);
            continue;
        }
        if let Some(rest) = cmd.strip_prefix("g ") {
            match rest.trim().parse::<usize>() {
                Ok(index) if index < app.trace.len() => app.selected = index,
                Ok(_) => app.status = format!("index out of range: {rest}"),
                Err(_) => app.status = format!("invalid index: {rest}"),
            }
            continue;
        }
        if cmd.is_empty() {
            continue;
        }
        app.status = format!("unknown command: {cmd}");
    }
}

#[cfg(feature = "tracing")]
#[derive(Copy, Clone)]
enum ActionKind {
    Remove,
    ShrinkPicked,
}

#[cfg(feature = "tracing")]
fn apply_action(app: &mut App, kind: ActionKind) {
    if app.trace.is_empty() {
        app.status = "trace is empty".to_string();
        return;
    }
    let preview = match kind {
        ActionKind::Remove => preview_remove(&app.trace, app.selected),
        ActionKind::ShrinkPicked => preview_shrink(&app.trace, app.selected),
    };

    let Some(candidate) = preview.candidate else {
        app.status = preview.error.unwrap_or_else(|| "no applicable candidate".to_string());
        return;
    };

    if !candidate.is_well_formed() {
        app.status = "candidate is not well-formed".to_string();
        return;
    }
    match replay_script(app.seed, &candidate) {
        Ok((script, productions, _)) => {
            app.history.push(app.trace.clone());
            app.trace = candidate;
            app.script = script;
            app.productions = productions;
            app.selected = app.selected.min(app.trace.len().saturating_sub(1));
            app.status =
                format!("applied {} (removed {} ids)", preview.action, preview.removed_ids.len());
        },
        Err(err) => {
            app.status = format!("replay failed: {:?}", err);
        },
    }
}

#[cfg(feature = "tracing")]
fn render(app: &App) -> io::Result<()> {
    let total_w = terminal_width().max(100);
    let left_w = total_w / 2;
    let right_w = total_w - left_w - 3;

    let left = build_left_panel(app, left_w);
    let right = build_right_panel(app, right_w);
    let rows = left.len().max(right.len());

    print!("\x1B[2J\x1B[H");
    println!("Traced Shrinking TUI (seed={}, trace_len={})", app.seed, app.trace.len());
    println!("status: {}", app.status);
    println!();
    for i in 0..rows {
        let l = left.get(i).map_or("", String::as_str);
        let r = right.get(i).map_or("", String::as_str);
        println!("{:<left_w$} | {}", truncate(l, left_w), truncate(r, right_w));
    }
    println!();
    println!("commands: j/k move, g <idx>, r remove, s shrink-picked, u undo, q quit");
    print!("cmd> ");
    io::stdout().flush()
}

#[cfg(feature = "tracing")]
fn build_left_panel(app: &App, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    out.push("Trace Decisions".to_string());
    out.push(format!("total={}", app.trace.len()));
    out.push(String::new());

    if app.trace.is_empty() {
        out.push("<empty trace>".to_string());
        return out;
    }

    let ranges = evaluate_ranges(&app.trace);
    let window = 22usize;
    let start = app.selected.saturating_sub(window / 2);
    let end = (start + window).min(app.trace.len());
    for idx in start..end {
        let d = &app.trace.decisions()[idx];
        let marker = if idx == app.selected { ">" } else { " " };
        let range = ranges
            .as_ref()
            .and_then(|all| all.get(idx).copied())
            .map(|(lo, hi)| format!("{lo}..{hi}"))
            .unwrap_or_else(|| "?..?".to_string());
        let deps = format_ids(&d.extra_deps);
        out.push(format!(
            "{} [{:>3}] id={:>3} pick={:>3} range={} deps={}",
            marker, idx, d.id.0, d.picked, range, deps
        ));
    }
    if end < app.trace.len() {
        out.push(format!("... {} more", app.trace.len() - end));
    }

    wrap_lines(out, width)
}

#[cfg(feature = "tracing")]
fn build_right_panel(app: &App, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    out.push("Generation + Preview".to_string());
    out.push(String::new());

    let current_sql = render_sql(&app.script);
    out.push(format!("Current SQL ({} lines)", current_sql.len()));
    for line in current_sql.iter().take(10) {
        out.push(format!("  {}", line));
    }
    if current_sql.len() > 10 {
        out.push(format!("  ... {} more", current_sql.len() - 10));
    }
    out.push(String::new());

    if app.trace.is_empty() {
        out.push("<no decision selected>".to_string());
        return wrap_lines(out, width);
    }

    let selected_id = app.trace.decisions()[app.selected].id;
    let selected_label = app
        .productions
        .get(&selected_id)
        .cloned()
        .unwrap_or_else(|| "<unknown production>".to_string());
    out.push(format!("Selected id {:?} production:", selected_id));
    out.push(format!("  {}", selected_label));
    out.push(String::new());

    out.push(format!("Selected id {:?} contributes to:", selected_id));
    let mut matched = 0usize;
    for (idx, rec) in app.script.statements.iter().enumerate() {
        if rec.origins.contains(&selected_id) {
            matched += 1;
            out.push(format!("  [{idx}] {}", statement_to_sql(&rec.statement)));
            if matched >= 8 {
                break;
            }
        }
    }
    if matched == 0 {
        out.push("  (no emitted statement uses this id)".to_string());
    }
    out.push(String::new());

    let remove_preview = preview_remove(&app.trace, app.selected);
    render_preview_block(&mut out, "Remove Selected", app.seed, &app.script, &remove_preview);
    out.push(String::new());
    let shrink_preview = preview_shrink(&app.trace, app.selected);
    render_preview_block(&mut out, "Shrink Picked", app.seed, &app.script, &shrink_preview);

    wrap_lines(out, width)
}

#[cfg(feature = "tracing")]
fn render_preview_block(
    out: &mut Vec<String>,
    title: &str,
    seed: u64,
    current_script: &Script,
    preview: &Preview,
) {
    out.push(format!("{title}:"));
    if let Some(error) = &preview.error {
        out.push(format!("  {error}"));
        return;
    }
    let Some(candidate) = &preview.candidate else {
        out.push("  unavailable".to_string());
        return;
    };

    match replay_script(seed, candidate) {
        Ok((candidate_script, _productions, _trace)) => {
            let current_sql = render_sql(current_script);
            let next_sql = render_sql(&candidate_script);
            let diff = sql_line_diff(&current_sql, &next_sql);
            let removed = diff.iter().filter(|d| matches!(d, DiffLine::Remove(_))).count();
            let added = diff.iter().filter(|d| matches!(d, DiffLine::Add(_))).count();
            out.push(format!(
                "  trace_len {} -> {}, removed_ids={}",
                candidate.len() + preview.removed_ids.len(),
                candidate.len(),
                preview.removed_ids.len()
            ));
            out.push(format!(
                "  sql_lines {} -> {}, diff(-{}, +{})",
                current_sql.len(),
                next_sql.len(),
                removed,
                added
            ));
            for entry in diff.into_iter().filter(|d| !matches!(d, DiffLine::Keep)).take(8) {
                match entry {
                    DiffLine::Remove(line) => out.push(format!("  - {}", line)),
                    DiffLine::Add(line) => out.push(format!("  + {}", line)),
                    DiffLine::Keep => {},
                }
            }
            if removed == 0 && added == 0 {
                out.push("  (no sql change)".to_string());
            }
        },
        Err(err) => out.push(format!("  replay error: {:?}", err)),
    }
}

#[cfg(feature = "tracing")]
fn preview_remove(trace: &ChoiceTrace, selected: usize) -> Preview {
    if selected >= trace.len() {
        return Preview {
            action: "remove",
            candidate: None,
            removed_ids: Vec::new(),
            error: Some("selected index out of range".to_string()),
        };
    }
    let id = trace.decisions()[selected].id;
    let candidate = trace.remove_with_dependents(id);
    let removed_ids = removed_ids(trace, &candidate);
    if candidate.len() >= trace.len() {
        return Preview {
            action: "remove",
            candidate: None,
            removed_ids,
            error: Some("remove candidate did not shrink trace".to_string()),
        };
    }
    Preview { action: "remove", candidate: Some(candidate), removed_ids, error: None }
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
fn preview_shrink(trace: &ChoiceTrace, selected: usize) -> Preview {
    if selected >= trace.len() {
        return Preview {
            action: "shrink-picked",
            candidate: None,
            removed_ids: Vec::new(),
            error: Some("selected index out of range".to_string()),
        };
    }

    let ranges = match evaluate_ranges(trace) {
        Some(ranges) => ranges,
        None => {
            return Preview {
                action: "shrink-picked",
                candidate: None,
                removed_ids: Vec::new(),
                error: Some("trace ranges are not resolvable".to_string()),
            };
        },
    };
    let decision = &trace.decisions()[selected];
    let (lo, _) = ranges[selected];
    if decision.picked <= lo {
        return Preview {
            action: "shrink-picked",
            candidate: None,
            removed_ids: Vec::new(),
            error: Some(format!("picked is already minimal ({})", decision.picked)),
        };
    }

    let mut decisions = trace.decisions().to_vec();
    decisions[selected].picked = decision.picked - 1;
    let candidate = ChoiceTrace::new(decisions);
    if !candidate.is_well_formed() {
        return Preview {
            action: "shrink-picked",
            candidate: None,
            removed_ids: Vec::new(),
            error: Some("candidate became ill-formed".to_string()),
        };
    }
    Preview {
        action: "shrink-picked",
        candidate: Some(candidate),
        removed_ids: Vec::new(),
        error: None,
    }
}

#[cfg(feature = "tracing")]
fn parse_args() -> Result<Opts, String> {
    let mut seed = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--seed" => {
                let value = args.next().ok_or_else(|| "--seed requires a value".to_string())?;
                let parsed =
                    value.parse::<u64>().map_err(|_| format!("invalid seed value: {value}"))?;
                seed = Some(parsed);
            },
            _ => return Err(format!("unrecognized argument: {arg}")),
        }
    }
    Ok(Opts { seed })
}

#[cfg(feature = "tracing")]
struct Opts {
    seed: Option<u64>,
}

#[cfg(feature = "tracing")]
fn pick_start(
    seed: Option<u64>,
) -> Result<(u64, ChoiceTrace, Script, HashMap<Id, String>), String> {
    match seed {
        Some(seed) => {
            let (script, productions, trace) =
                record_script(seed).map_err(|e| format!("{:?}", e))?;
            Ok((seed, trace, script, productions))
        },
        None => {
            for seed in 0..10_000_u64 {
                let Ok((script, productions, trace)) = record_script(seed) else {
                    continue;
                };
                if has_sql_content(&script) {
                    return Ok((seed, trace, script, productions));
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
fn terminal_width() -> usize {
    std::env::var("COLUMNS").ok().and_then(|s| s.parse::<usize>().ok()).unwrap_or(140)
}

#[cfg(feature = "tracing")]
fn truncate(s: &str, w: usize) -> String {
    if s.len() <= w {
        return s.to_string();
    }
    if w <= 1 {
        return "".to_string();
    }
    format!("{}…", &s[..w - 1])
}

#[cfg(feature = "tracing")]
fn wrap_lines(lines: Vec<String>, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in lines {
        if line.len() <= width {
            out.push(line);
            continue;
        }
        let mut start = 0usize;
        while start < line.len() {
            let end = (start + width).min(line.len());
            out.push(line[start..end].to_string());
            start = end;
        }
    }
    out
}

#[cfg(feature = "tracing")]
fn format_ids(ids: &[Id]) -> String {
    if ids.is_empty() {
        return "[]".to_string();
    }
    let mut s = String::from("[");
    for (i, id) in ids.iter().enumerate().take(4) {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&id.0.to_string());
    }
    if ids.len() > 4 {
        s.push_str(",...");
    }
    s.push(']');
    s
}

#[cfg(feature = "tracing")]
#[derive(Clone, Debug)]
enum DiffLine {
    Keep,
    Remove(String),
    Add(String),
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

    let mut diff = Vec::new();
    let mut i = 0usize;
    let mut j = 0usize;
    while i < n && j < m {
        if before[i] == after[j] {
            diff.push(DiffLine::Keep);
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
fn evaluate_ranges(trace: &ChoiceTrace) -> Option<Vec<(usize, usize)>> {
    let mut values = HashMap::new();
    let mut ranges = Vec::with_capacity(trace.len());
    for decision in trace.decisions() {
        let lo = resolve_operand(&decision.lo, &values)?;
        let hi = resolve_operand(&decision.hi, &values)?;
        if lo > hi || decision.picked < lo || decision.picked > hi {
            return None;
        }
        values.insert(decision.id, decision.picked);
        ranges.push((lo, hi));
    }
    Some(ranges)
}

#[cfg(feature = "tracing")]
fn resolve_operand(operand: &Operand, values: &HashMap<Id, usize>) -> Option<usize> {
    match operand {
        Operand::Const(value) => Some(*value),
        Operand::FromId(id) => values.get(id).copied(),
    }
}

#[cfg(feature = "tracing")]
fn record_script(seed: u64) -> Result<(Script, HashMap<Id, String>, ChoiceTrace), TraceError> {
    let mut runner = TraceRunner::recording(seed);
    let (script, productions) = build_script(&mut runner)?;
    Ok((script, productions, runner.finish()))
}

#[cfg(feature = "tracing")]
fn replay_script(
    seed: u64,
    plan: &ChoiceTrace,
) -> Result<(Script, HashMap<Id, String>, ChoiceTrace), TraceError> {
    let mut runner = TraceRunner::replay(seed, plan);
    let (script, productions) = build_script(&mut runner)?;
    Ok((script, productions, runner.finish()))
}

#[cfg(feature = "tracing")]
fn build_script(runner: &mut TraceRunner) -> Result<(Script, HashMap<Id, String>), TraceError> {
    let stmt_count =
        runner.choose_usize_with_id(sid_stmt_count(), 0.into(), MAX_STMTS.into(), &[])?;
    let mut productions = HashMap::new();
    productions.insert(stmt_count.id, format!("program stmt_count (number of emitted statements)"));
    let mut statements = Vec::new();
    let mut table_cols = [1usize; NUM_TABLES];
    let mut table_create_dep = [None; NUM_TABLES];

    for slot in 0..stmt_count.value {
        let table_choice = runner.choose_usize_with_id(
            sid_slot_table(slot),
            0.into(),
            (NUM_TABLES - 1).into(),
            &[stmt_count.id],
        )?;
        productions
            .insert(table_choice.id, format!("slot[{slot}] table choice (0..{})", NUM_TABLES - 1));
        let table = table_choice.value;
        let create_dep = table_create_dep[table];

        match create_dep {
            None => {
                let cols = runner.choose_usize_with_id(
                    sid_slot_create_cols(slot),
                    1.into(),
                    MAX_COLS.into(),
                    &[stmt_count.id, table_choice.id],
                )?;
                productions.insert(cols.id, format!("slot[{slot}] create t{table} column count"));
                let mut origins = vec![stmt_count.id, table_choice.id, cols.id];
                normalize_ids(&mut origins);
                statements.push(StatementRecord {
                    statement: Statement::Create { table, cols: cols.value },
                    origins,
                });
                table_cols[table] = cols.value;
                // Use the create's column-count decision as the dependency anchor for this table's future use.
                table_create_dep[table] = Some(cols.id);
            },
            Some(dep) => {
                let action = runner.choose_usize_with_id(
                    sid_slot_action(slot),
                    0.into(),
                    1.into(),
                    &[stmt_count.id, table_choice.id, dep],
                )?;
                let action_label = match action.value {
                    0 => "insert",
                    _ => "select",
                };
                productions
                    .insert(action.id, format!("slot[{slot}] action on t{table}: {action_label}"));
                match action.value {
                    0 => {
                        let cols = table_cols[table];
                        let mut values = Vec::with_capacity(cols);
                        let mut origins = vec![stmt_count.id, table_choice.id, action.id, dep];
                        for c in 0..MAX_COLS {
                            let cell = runner.choose_usize_with_id(
                                sid_slot_insert_cell(slot, c),
                                0.into(),
                                20.into(),
                                &[stmt_count.id, action.id, dep],
                            )?;
                            productions
                                .insert(cell.id, format!("slot[{slot}] insert t{table} cell c{c}"));
                            if c < cols {
                                values.push(map_small_i32(cell.value));
                                origins.push(cell.id);
                            }
                        }
                        normalize_ids(&mut origins);
                        statements.push(StatementRecord {
                            statement: Statement::Insert { table, values },
                            origins,
                        });
                    },
                    _ => {
                        let kind = runner.choose_usize_with_id(
                            sid_slot_select_kind(slot),
                            0.into(),
                            2.into(),
                            &[stmt_count.id, table_choice.id, action.id, dep],
                        )?;
                        productions.insert(
                            kind.id,
                            format!("slot[{slot}] select kind on t{table} (0=count,1=sum,2=max)"),
                        );
                        let (expr, mut expr_ids) = build_expr(
                            runner,
                            MAX_EXPR_DEPTH,
                            table_cols[table],
                            &[stmt_count.id, action.id, dep, kind.id],
                            &mut productions,
                            &format!("slot[{slot}] expr for t{table}"),
                            slot,
                            1,
                        )?;
                        let mut origins =
                            vec![stmt_count.id, table_choice.id, action.id, dep, kind.id];
                        origins.append(&mut expr_ids);
                        normalize_ids(&mut origins);
                        let kind = match kind.value {
                            0 => SelectKind::Count,
                            1 => SelectKind::Sum(expr),
                            _ => SelectKind::Max(expr),
                        };
                        statements.push(StatementRecord {
                            statement: Statement::Select { table, kind },
                            origins,
                        });
                    },
                }
            },
        }
    }

    Ok((Script { statements }, productions))
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
    productions: &mut HashMap<Id, String>,
    label: &str,
    slot: usize,
    node: usize,
) -> Result<(Expr, Vec<Id>), TraceError> {
    let op = runner.choose_usize_with_id(sid_expr(slot, node, 0), 0.into(), 5.into(), deps)?;
    productions.insert(op.id, format!("{label} op (0=col,1=lit,2=add,3=sub,4=mul,5=neg)"));
    let col = runner.choose_usize_with_id(
        sid_expr(slot, node, 1),
        0.into(),
        (MAX_COLS - 1).into(),
        &[op.id],
    )?;
    productions.insert(col.id, format!("{label} column index"));
    let lit =
        runner.choose_usize_with_id(sid_expr(slot, node, 2), 0.into(), 20.into(), &[op.id])?;
    productions.insert(lit.id, format!("{label} literal (mapped to -10..10)"));
    let base_col = Expr::Col(col.value % cols.max(1));
    let base_lit = Expr::Lit(map_small_i32(lit.value));

    if depth == 0 {
        return Ok(match op.value {
            0 | 2 | 4 => (base_col, vec![op.id, col.id]),
            _ => (base_lit, vec![op.id, lit.id]),
        });
    }

    let (left, mut left_ids) = build_expr(
        runner,
        depth - 1,
        cols,
        &[op.id],
        productions,
        &format!("{label}.L"),
        slot,
        node * 2,
    )?;
    let (right, mut right_ids) = build_expr(
        runner,
        depth - 1,
        cols,
        &[op.id],
        productions,
        &format!("{label}.R"),
        slot,
        node * 2 + 1,
    )?;
    let (expr, mut ids) = match op.value {
        0 => (base_col, vec![op.id, col.id]),
        1 => (base_lit, vec![op.id, lit.id]),
        2 => {
            let mut ids = vec![op.id];
            ids.append(&mut left_ids);
            ids.append(&mut right_ids);
            (Expr::Add(Box::new(left), Box::new(right)), ids)
        },
        3 => {
            let mut ids = vec![op.id];
            ids.append(&mut left_ids);
            ids.append(&mut right_ids);
            (Expr::Sub(Box::new(left), Box::new(right)), ids)
        },
        4 => {
            let mut ids = vec![op.id];
            ids.append(&mut left_ids);
            ids.append(&mut right_ids);
            (Expr::Mul(Box::new(left), Box::new(right)), ids)
        },
        _ => {
            let mut ids = vec![op.id];
            ids.append(&mut left_ids);
            (Expr::Neg(Box::new(left)), ids)
        },
    };
    normalize_ids(&mut ids);
    Ok((expr, ids))
}

#[cfg(feature = "tracing")]
fn render_expr(expr: &Expr) -> String {
    match expr {
        Expr::Col(col) => format!("c{}", col),
        Expr::Lit(v) => v.to_string(),
        Expr::Add(l, r) => format!("({} + {})", render_expr(l), render_expr(r)),
        Expr::Sub(l, r) => format!("({} - {})", render_expr(l), render_expr(r)),
        Expr::Mul(l, r) => format!("({} * {})", render_expr(l), render_expr(r)),
        Expr::Neg(x) => format!("(-{})", render_expr(x)),
    }
}

#[cfg(feature = "tracing")]
fn render_sql(script: &Script) -> Vec<String> {
    let mut lines = Vec::new();
    for rec in &script.statements {
        lines.push(statement_to_sql(&rec.statement));
    }
    if lines.is_empty() {
        lines.push("-- <empty script>".to_string());
    }
    lines
}

#[cfg(feature = "tracing")]
fn statement_to_sql(statement: &Statement) -> String {
    match statement {
        Statement::Create { table, cols } => {
            let mut defs = Vec::with_capacity(*cols);
            for c in 0..*cols {
                defs.push(format!("c{} INT", c));
            }
            format!("CREATE TABLE t{} ({});", table, defs.join(", "))
        },
        Statement::Insert { table, values } => {
            let payload = values.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ");
            format!("INSERT INTO t{} VALUES ({});", table, payload)
        },
        Statement::Select { table, kind } => {
            let expr = match kind {
                SelectKind::Count => "COUNT(*)".to_string(),
                SelectKind::Sum(expr) => format!("SUM({})", render_expr(expr)),
                SelectKind::Max(expr) => format!("MAX({})", render_expr(expr)),
            };
            format!("SELECT {} FROM t{};", expr, table)
        },
    }
}

#[cfg(feature = "tracing")]
fn normalize_ids(ids: &mut Vec<Id>) {
    ids.sort_unstable();
    ids.dedup();
}

#[cfg(not(feature = "tracing"))]
fn main() {
    eprintln!("Run with: cargo run --example traced_sql_shrinking_tui --features tracing");
}
