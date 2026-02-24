#[cfg(feature = "tracing")]
use std::collections::HashSet;

#[cfg(feature = "tracing")]
use crabcheck::tracing::{
    ChoiceTrace,
    Operand,
    Tree,
    generate_traced_tree,
    replay_traced_tree,
    shrink_traced_tree,
};

#[cfg(feature = "tracing")]
fn main() {
    let max_size = 40;
    let max_depth = 6;

    let (seed, before_tree, before_trace) =
        find_failing_case(max_size, max_depth).expect("failed to find a failing traced-tree input");

    let before_buggy = buggy_sum(&before_tree);
    let before_correct = correct_sum(&before_tree);

    println!("== Traced Shrinking Hard-Bug Demo ==");
    println!("seed: {}", seed);
    println!("bug: node sum drops right subtree whenever left subtree sum is even");
    println!(
        "before: nodes={}, trace_len={}, buggy_sum={}, correct_sum={}",
        node_count(&before_tree),
        before_trace.len(),
        before_buggy,
        before_correct
    );
    println!("before tree: {:?}", before_tree);
    print_trace_compact("before trace", &before_trace, 20);

    let shrunk_trace =
        shrink_traced_tree(seed, max_size, max_depth, before_trace.clone(), fails_hard_bug);
    let (after_traced, _replayed_trace) =
        replay_traced_tree(seed, max_size, max_depth, &shrunk_trace)
            .expect("shrunk trace should replay");
    let after_tree = after_traced.lift_back();
    let after_buggy = buggy_sum(&after_tree);
    let after_correct = correct_sum(&after_tree);

    println!();
    println!(
        "after: nodes={}, shrunk_plan_len={}, buggy_sum={}, correct_sum={}",
        node_count(&after_tree),
        shrunk_trace.len(),
        after_buggy,
        after_correct
    );
    println!("after tree: {:?}", after_tree);
    print_trace_compact("after shrunk plan", &shrunk_trace, 20);

    let before_ids = trace_ids(&before_trace);
    let after_ids = trace_ids(&shrunk_trace);
    let mut removed_ids = before_ids.difference(&after_ids).copied().collect::<Vec<_>>();
    removed_ids.sort_unstable();

    println!();
    println!("removed decisions: {}", removed_ids.len());
    println!("removed ids: {:?}", removed_ids);

    assert!(fails_hard_bug(&after_tree));
    assert!(!matches!(after_tree, Tree::Leaf(_)));
}

#[cfg(feature = "tracing")]
fn find_failing_case(max_size: usize, max_depth: usize) -> Option<(u64, Tree<usize>, ChoiceTrace)> {
    for seed in 0..100_000_u64 {
        let (traced_tree, trace) = generate_traced_tree(seed, max_size, max_depth).ok()?;
        let plain = traced_tree.lift_back();
        if fails_hard_bug(&plain) && trace.len() >= 12 {
            return Some((seed, plain, trace));
        }
    }

    None
}

#[cfg(feature = "tracing")]
fn fails_hard_bug(tree: &Tree<usize>) -> bool {
    buggy_sum(tree) != correct_sum(tree)
}

#[cfg(feature = "tracing")]
fn buggy_sum(tree: &Tree<usize>) -> usize {
    match tree {
        Tree::Leaf(value) => *value,
        Tree::Node(value, left, right) => {
            let left_sum = buggy_sum(left);
            let right_sum = buggy_sum(right);
            if left_sum % 2 == 0 { value + left_sum } else { value + left_sum + right_sum }
        },
    }
}

#[cfg(feature = "tracing")]
fn correct_sum(tree: &Tree<usize>) -> usize {
    match tree {
        Tree::Leaf(value) => *value,
        Tree::Node(value, left, right) => value + correct_sum(left) + correct_sum(right),
    }
}

#[cfg(feature = "tracing")]
fn node_count(tree: &Tree<usize>) -> usize {
    match tree {
        Tree::Leaf(_) => 1,
        Tree::Node(_, left, right) => 1 + node_count(left) + node_count(right),
    }
}

#[cfg(feature = "tracing")]
fn trace_ids(trace: &ChoiceTrace) -> HashSet<usize> {
    trace.decisions().iter().map(|decision| decision.id.0).collect()
}

#[cfg(feature = "tracing")]
fn format_operand(operand: &Operand) -> String {
    match operand {
        Operand::Const(value) => value.to_string(),
        Operand::FromId(id) => format!("id{}", id.0),
    }
}

#[cfg(feature = "tracing")]
fn print_trace_compact(label: &str, trace: &ChoiceTrace, limit: usize) {
    println!("{} ({} decisions):", label, trace.len());
    for decision in trace.decisions().iter().take(limit) {
        println!(
            "  id{:02} <- choose({}, {}) = {} deps={:?}",
            decision.id.0,
            format_operand(&decision.lo),
            format_operand(&decision.hi),
            decision.picked,
            decision.extra_deps
        );
    }
    if trace.len() > limit {
        println!("  ... {} more decisions", trace.len() - limit);
    }
}

#[cfg(not(feature = "tracing"))]
fn main() {
    eprintln!("Run with: cargo run --example traced_shrinking_hard_bug --features tracing");
}
