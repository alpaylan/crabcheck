#[cfg(feature = "tracing")]
use std::collections::HashSet;

#[cfg(feature = "tracing")]
use crabcheck::tracing::{
    ChoiceTrace,
    Id,
    Operand,
    Tree,
    generate_traced_tree,
    replay_traced_tree,
    shrink_traced_tree,
};

#[cfg(feature = "tracing")]
fn main() {
    let max_size = 30;
    let max_depth = 5;
    let (seed, before_tree, before_trace) = find_case(max_size, max_depth)
        .expect("failed to find a demo case with a non-trivial trace");
    let failing_root_threshold = tree_root(&before_tree);

    println!("== Traced Shrinking Demo ==");
    println!("seed: {}", seed);
    println!("property-style predicate example: root(tree) >= {}", failing_root_threshold);
    println!(
        "before: root={}, nodes={}, trace_len={}",
        tree_root(&before_tree),
        node_count(&before_tree),
        before_trace.len()
    );
    println!("before tree: {:?}", before_tree);
    print_trace("before trace", &before_trace);

    let cascade = before_trace.remove_with_dependents(Id(3));
    let (cascade_tree, cascade_replayed_trace) =
        replay_traced_tree(seed, max_size, max_depth, &cascade)
            .expect("cascade candidate should replay");
    let cascade_plain = cascade_tree.lift_back();

    println!();
    println!("one-step cascade removal: remove decision id03 (root branch choice)");
    println!("cascade plan length: {} (from before={})", cascade.len(), before_trace.len());
    println!(
        "cascade replay result: root={}, nodes={}, replayed_trace_len={}",
        tree_root(&cascade_plain),
        node_count(&cascade_plain),
        cascade_replayed_trace.len()
    );
    println!("cascade tree: {:?}", cascade_plain);
    print_trace("cascade plan (what shrink keeps)", &cascade);

    let shrunk_trace =
        shrink_traced_tree(seed, max_size, max_depth, before_trace.clone(), |_tree| true);

    let (after_traced, replayed_trace) =
        replay_traced_tree(seed, max_size, max_depth, &shrunk_trace)
            .expect("shrunk trace should replay");
    let after_tree = after_traced.lift_back();

    println!();
    println!(
        "after shrunk plan length: {} (from before={})",
        shrunk_trace.len(),
        before_trace.len()
    );
    println!(
        "after replay: root={}, nodes={}, replayed_trace_len={}",
        tree_root(&after_tree),
        node_count(&after_tree),
        replayed_trace.len()
    );
    println!("after tree: {:?}", after_tree);
    print_trace("after shrunk plan (what shrink keeps)", &shrunk_trace);
    println!("full-shrink predicate used in this demo: always failing (true)");

    let before_ids = trace_ids(&before_trace);
    let after_ids = trace_ids(&shrunk_trace);
    let removed_ids = before_ids.difference(&after_ids).copied().collect::<Vec<_>>();

    println!();
    println!("choices removed by shrinking: {}", removed_ids.len());
    println!("removed ids: {:?}", removed_ids);
}

#[cfg(feature = "tracing")]
fn find_case(max_size: usize, max_depth: usize) -> Option<(u64, Tree<usize>, ChoiceTrace)> {
    for seed in 0..10_000_u64 {
        let (traced_tree, trace) = generate_traced_tree(seed, max_size, max_depth).ok()?;
        let plain = traced_tree.lift_back();
        if trace.len() >= 6 {
            return Some((seed, plain, trace));
        }
    }

    None
}

#[cfg(feature = "tracing")]
fn tree_root(tree: &Tree<usize>) -> usize {
    match tree {
        Tree::Leaf(value) | Tree::Node(value, _, _) => *value,
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
fn print_trace(label: &str, trace: &ChoiceTrace) {
    println!("{} ({} decisions):", label, trace.len());
    for decision in trace.decisions() {
        println!(
            "  id{:02} <- choose({}, {}) = {} deps={:?}",
            decision.id.0,
            format_operand(&decision.lo),
            format_operand(&decision.hi),
            decision.picked,
            decision.extra_deps
        );
    }
}

#[cfg(not(feature = "tracing"))]
fn main() {
    eprintln!("Run with: cargo run --example traced_shrinking_demo --features tracing");
}
