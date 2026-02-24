#[cfg(feature = "tracing")]
use std::{
    collections::HashSet,
    env,
    fmt,
};

#[cfg(feature = "tracing")]
use crabcheck::tracing::{
    ChoiceTrace,
    Operand,
    TrTree,
    TraceError,
    TraceRunner,
    TracedUsize,
    Tree as TracedShape,
    shrink_trace,
};
#[cfg(feature = "tracing")]
use serde_json::json;

#[cfg(feature = "tracing")]
#[path = "bst/bst.rs"]
mod bst;

#[cfg(feature = "tracing")]
use bst::Tree;

#[cfg(feature = "tracing")]
const FUEL: usize = 10_000;

#[cfg(feature = "tracing")]
#[derive(Clone, Copy, Debug)]
enum Mutation {
    Base,
    Insert1,
    Insert2,
    Insert3,
    Delete4,
    Delete5,
    Union6,
    Union7,
    Union8,
}

#[cfg(feature = "tracing")]
impl Mutation {
    fn parse(input: &str) -> Option<Self> {
        match input {
            "base" => Some(Self::Base),
            "insert_1" => Some(Self::Insert1),
            "insert_2" => Some(Self::Insert2),
            "insert_3" => Some(Self::Insert3),
            "delete_4" => Some(Self::Delete4),
            "delete_5" => Some(Self::Delete5),
            "union_6" => Some(Self::Union6),
            "union_7" => Some(Self::Union7),
            "union_8" => Some(Self::Union8),
            _ => None,
        }
    }
}

#[cfg(feature = "tracing")]
impl fmt::Display for Mutation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Base => "base",
            Self::Insert1 => "insert_1",
            Self::Insert2 => "insert_2",
            Self::Insert3 => "insert_3",
            Self::Delete4 => "delete_4",
            Self::Delete5 => "delete_5",
            Self::Union6 => "union_6",
            Self::Union7 => "union_7",
            Self::Union8 => "union_8",
        };
        write!(f, "{}", name)
    }
}

#[cfg(feature = "tracing")]
#[derive(Clone, Copy, Debug)]
enum Property {
    InsertValid,
    DeleteValid,
    UnionValid,
    InsertPost,
    DeletePost,
    UnionPost,
    InsertModel,
    DeleteModel,
    UnionModel,
    InsertInsert,
    InsertDelete,
    InsertUnion,
    DeleteInsert,
    DeleteDelete,
    DeleteUnion,
    UnionDeleteInsert,
    UnionUnionAssoc,
}

#[cfg(feature = "tracing")]
impl Property {
    fn parse(input: &str) -> Option<Self> {
        match input {
            "insert_valid" => Some(Self::InsertValid),
            "InsertValid" => Some(Self::InsertValid),
            "delete_valid" => Some(Self::DeleteValid),
            "DeleteValid" => Some(Self::DeleteValid),
            "union_valid" => Some(Self::UnionValid),
            "UnionValid" => Some(Self::UnionValid),
            "insert_post" => Some(Self::InsertPost),
            "InsertPost" => Some(Self::InsertPost),
            "delete_post" => Some(Self::DeletePost),
            "DeletePost" => Some(Self::DeletePost),
            "union_post" => Some(Self::UnionPost),
            "UnionPost" => Some(Self::UnionPost),
            "insert_model" => Some(Self::InsertModel),
            "InsertModel" => Some(Self::InsertModel),
            "delete_model" => Some(Self::DeleteModel),
            "DeleteModel" => Some(Self::DeleteModel),
            "union_model" => Some(Self::UnionModel),
            "UnionModel" => Some(Self::UnionModel),
            "insert_insert" => Some(Self::InsertInsert),
            "InsertInsert" => Some(Self::InsertInsert),
            "insert_delete" => Some(Self::InsertDelete),
            "InsertDelete" => Some(Self::InsertDelete),
            "insert_union" => Some(Self::InsertUnion),
            "InsertUnion" => Some(Self::InsertUnion),
            "delete_insert" => Some(Self::DeleteInsert),
            "DeleteInsert" => Some(Self::DeleteInsert),
            "delete_delete" => Some(Self::DeleteDelete),
            "DeleteDelete" => Some(Self::DeleteDelete),
            "delete_union" => Some(Self::DeleteUnion),
            "DeleteUnion" => Some(Self::DeleteUnion),
            "union_delete_insert" => Some(Self::UnionDeleteInsert),
            "UnionDeleteInsert" => Some(Self::UnionDeleteInsert),
            "union_union_assoc" => Some(Self::UnionUnionAssoc),
            "UnionUnionAssoc" => Some(Self::UnionUnionAssoc),
            _ => None,
        }
    }
}

#[cfg(feature = "tracing")]
impl fmt::Display for Property {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::InsertValid => "insert_valid",
            Self::DeleteValid => "delete_valid",
            Self::UnionValid => "union_valid",
            Self::InsertPost => "insert_post",
            Self::DeletePost => "delete_post",
            Self::UnionPost => "union_post",
            Self::InsertModel => "insert_model",
            Self::DeleteModel => "delete_model",
            Self::UnionModel => "union_model",
            Self::InsertInsert => "insert_insert",
            Self::InsertDelete => "insert_delete",
            Self::InsertUnion => "insert_union",
            Self::DeleteInsert => "delete_insert",
            Self::DeleteDelete => "delete_delete",
            Self::DeleteUnion => "delete_union",
            Self::UnionDeleteInsert => "union_delete_insert",
            Self::UnionUnionAssoc => "union_union_assoc",
        };
        write!(f, "{}", name)
    }
}

#[cfg(feature = "tracing")]
#[derive(Clone, Debug)]
struct Case {
    t1: Tree,
    t2: Tree,
    t3: Tree,
    k1: i32,
    k2: i32,
    v1: i32,
    v2: i32,
}

#[cfg(feature = "tracing")]
fn main() {
    let args = env::args().collect::<Vec<_>>();
    let summary_mode = env::var("TRACED_BST_SUMMARY").map(|v| v == "1").unwrap_or(false);
    if args.len() != 3 {
        print_usage(&args[0]);
        return;
    }

    let mutation = match Mutation::parse(&args[1]) {
        Some(value) => value,
        None => {
            eprintln!("Unknown mutation: {}", args[1]);
            print_usage(&args[0]);
            return;
        },
    };

    let property = match Property::parse(&args[2]) {
        Some(value) => value,
        None => {
            eprintln!("Unknown property: {}", args[2]);
            print_usage(&args[0]);
            return;
        },
    };

    let max_size = 30;
    let max_depth = 5;
    let max_seed = 100_000;

    let seed = match find_seed(mutation, property, max_size, max_depth, max_seed) {
        Some(value) => value,
        None => {
            if summary_mode {
                let payload = json!({
                    "mutation": mutation.to_string(),
                    "property": property.to_string(),
                    "found_failing_case": false
                });
                println!("{}", payload);
            } else {
                println!("No failing case found for mutation={} property={}", mutation, property);
            }
            return;
        },
    };

    let (before_case, before_trace) =
        generate_case(seed, max_size, max_depth).expect("recording should succeed");
    let before_status = eval_property(mutation, property, &before_case);
    if !matches!(before_status, Some(false)) {
        if summary_mode {
            let payload = json!({
                "mutation": mutation.to_string(),
                "property": property.to_string(),
                "found_failing_case": false,
                "seed": seed,
                "reason": format!("regeneration status {:?}", before_status)
            });
            println!("{}", payload);
        } else {
            println!(
                "Seed {} stopped failing after regeneration (status={:?}); try rerun.",
                seed, before_status
            );
        }
        return;
    }

    let shrunk_trace = shrink_trace(before_trace.clone(), |candidate| {
        replay_case(seed, max_size, max_depth, candidate)
            .map(|case| matches!(eval_property(mutation, property, &case), Some(false)))
            .unwrap_or(false)
    });

    let after_case =
        replay_case(seed, max_size, max_depth, &shrunk_trace).expect("replay should succeed");
    let after_status = eval_property(mutation, property, &after_case);

    if !summary_mode {
        println!("== Traced Shrinking Demo: BST ==");
        println!("mutation: {}", mutation);
        println!("property: {}", property);
        println!("seed: {}", seed);
        println!();

        println!(
            "before: t1_size={}, t2_size={}, t3_size={}, trace_len={}, status={:?}",
            bst::size(&before_case.t1),
            bst::size(&before_case.t2),
            bst::size(&before_case.t3),
            before_trace.len(),
            before_status
        );
        println!(
            "before args: (k1={}, k2={}, v1={}, v2={})",
            before_case.k1, before_case.k2, before_case.v1, before_case.v2
        );
        println!("before t1: {:?}", before_case.t1);
        print_trace_compact("before trace", &before_trace, 18);

        println!();
        println!(
            "after: t1_size={}, t2_size={}, t3_size={}, shrunk_plan_len={}, status={:?}",
            bst::size(&after_case.t1),
            bst::size(&after_case.t2),
            bst::size(&after_case.t3),
            shrunk_trace.len(),
            after_status
        );
        println!(
            "after args: (k1={}, k2={}, v1={}, v2={})",
            after_case.k1, after_case.k2, after_case.v1, after_case.v2
        );
        println!("after t1: {:?}", after_case.t1);
        print_trace_compact("after shrunk plan", &shrunk_trace, 18);
    }

    let before_ids = trace_ids(&before_trace);
    let after_ids = trace_ids(&shrunk_trace);
    let mut removed_ids = before_ids.difference(&after_ids).copied().collect::<Vec<_>>();
    removed_ids.sort_unstable();

    if summary_mode {
        let payload = json!({
            "mutation": mutation.to_string(),
            "property": property.to_string(),
            "found_failing_case": true,
            "seed": seed,
            "before_status": format!("{:?}", before_status),
            "after_status": format!("{:?}", after_status),
            "before_t1_size": bst::size(&before_case.t1),
            "after_t1_size": bst::size(&after_case.t1),
            "before_t2_size": bst::size(&before_case.t2),
            "after_t2_size": bst::size(&after_case.t2),
            "before_t3_size": bst::size(&before_case.t3),
            "after_t3_size": bst::size(&after_case.t3),
            "before_trace_len": before_trace.len(),
            "after_trace_len": shrunk_trace.len(),
            "removed_decisions": removed_ids.len(),
            "before_args": {
                "k1": before_case.k1,
                "k2": before_case.k2,
                "v1": before_case.v1,
                "v2": before_case.v2
            },
            "after_args": {
                "k1": after_case.k1,
                "k2": after_case.k2,
                "v1": after_case.v1,
                "v2": after_case.v2
            }
        });
        println!("{}", payload);
        return;
    }

    println!();
    println!("removed decisions: {}", removed_ids.len());
    println!("removed ids: {:?}", removed_ids);

    assert!(matches!(before_status, Some(false)));
    assert!(matches!(after_status, Some(false)));
}

#[cfg(feature = "tracing")]
fn print_usage(bin: &str) {
    eprintln!("Usage: {} <mutation> <property>", bin);
    eprintln!(
        "Mutations: base, insert_1, insert_2, insert_3, delete_4, delete_5, union_6, union_7, union_8"
    );
    eprintln!(
        "Properties: insert_valid, delete_valid, union_valid, insert_post, delete_post, union_post, insert_model, delete_model, union_model, insert_insert, insert_delete, insert_union, delete_insert, delete_delete, delete_union, union_delete_insert, union_union_assoc"
    );
}

#[cfg(feature = "tracing")]
fn find_seed(
    mutation: Mutation,
    property: Property,
    max_size: usize,
    max_depth: usize,
    max_seed: u64,
) -> Option<u64> {
    (0..max_seed).find(|seed| {
        generate_case(*seed, max_size, max_depth)
            .map(|(case, trace)| {
                matches!(eval_property(mutation, property, &case), Some(false))
                    && bst::size(&case.t1) >= 4
                    && trace.len() >= 15
            })
            .unwrap_or(false)
    })
}

#[cfg(feature = "tracing")]
fn generate_case(
    seed: u64,
    max_size: usize,
    max_depth: usize,
) -> Result<(Case, ChoiceTrace), TraceError> {
    let mut runner = TraceRunner::recording(seed);
    let case = build_case(&mut runner, max_size, max_depth)?;
    Ok((case, runner.finish()))
}

#[cfg(feature = "tracing")]
fn replay_case(
    seed: u64,
    max_size: usize,
    max_depth: usize,
    plan: &ChoiceTrace,
) -> Result<Case, TraceError> {
    let mut runner = TraceRunner::replay(seed, plan);
    build_case(&mut runner, max_size, max_depth)
}

#[cfg(feature = "tracing")]
fn build_case(
    runner: &mut TraceRunner,
    max_size: usize,
    max_depth: usize,
) -> Result<Case, TraceError> {
    let t1 = build_tree_arg(runner, max_size, max_depth)?;
    let t2 = build_tree_arg(runner, max_size, max_depth)?;
    let t3 = build_tree_arg(runner, max_size, max_depth)?;

    let k1 = choose_i32(runner, -20, 20)?;
    let k2 = choose_i32(runner, -20, 20)?;
    let v1 = choose_i32(runner, -20, 20)?;
    let v2 = choose_i32(runner, -20, 20)?;

    Ok(Case { t1, t2, t3, k1, k2, v1, v2 })
}

#[cfg(feature = "tracing")]
fn build_tree_arg(
    runner: &mut TraceRunner,
    max_size: usize,
    max_depth: usize,
) -> Result<Tree, TraceError> {
    let root_size = runner.choose_usize(1.into(), max_size.into(), &[])?;
    let traced_tree = TrTree::<TracedUsize>::arbitrary_sized(runner, max_depth, root_size)?;
    let traced_shape = traced_tree.lift_back();

    let base = choose_i32(runner, -15, 15)?;
    let mut next_key = base;
    Ok(shape_to_bst(&traced_shape, &mut next_key))
}

#[cfg(feature = "tracing")]
fn choose_i32(runner: &mut TraceRunner, lo: i32, hi: i32) -> Result<i32, TraceError> {
    let width = (hi - lo) as usize;
    let choice = runner.choose_usize(0.into(), width.into(), &[])?;
    let order = zero_centered_range(lo, hi);
    Ok(order[choice.value])
}

#[cfg(feature = "tracing")]
fn zero_centered_range(lo: i32, hi: i32) -> Vec<i32> {
    let mut values = (lo..=hi).collect::<Vec<_>>();
    // Prefer values close to zero; break ties by positive first (0, 1, -1, 2, -2, ...).
    values.sort_by_key(|v| (v.abs(), if *v >= 0 { 0 } else { 1 }));
    values
}

#[cfg(feature = "tracing")]
fn shape_to_bst(shape: &TracedShape<usize>, next_key: &mut i32) -> Tree {
    match shape {
        TracedShape::Leaf(value) => {
            let key = *next_key;
            *next_key += 1;
            let val = (*value as i32 % 41) - 20;
            Tree::T(Box::new(Tree::E), key, val, Box::new(Tree::E))
        },
        TracedShape::Node(value, left, right) => {
            let left_tree = shape_to_bst(left, next_key);
            let key = *next_key;
            *next_key += 1;
            let right_tree = shape_to_bst(right, next_key);
            let val = (*value as i32 % 41) - 20;
            Tree::T(Box::new(left_tree), key, val, Box::new(right_tree))
        },
    }
}

#[cfg(feature = "tracing")]
fn eval_property(mutation: Mutation, property: Property, case: &Case) -> Option<bool> {
    match property {
        Property::InsertValid => {
            let t = case.t1.clone();
            if !is_bst(&t) {
                None
            } else {
                Some(is_bst(&insert_mut(mutation, case.k1, case.v1, t)))
            }
        },
        Property::DeleteValid => {
            let t = case.t1.clone();
            if !is_bst(&t) { None } else { Some(is_bst(&delete_mut(mutation, case.k1, t))) }
        },
        Property::UnionValid => {
            let t1 = case.t1.clone();
            let t2 = case.t2.clone();
            if !is_bst(&t1) || !is_bst(&t2) {
                None
            } else {
                Some(is_bst(&union_mut(mutation, t1, t2)))
            }
        },
        Property::InsertPost => {
            let t = case.t1.clone();
            if !is_bst(&t) {
                None
            } else {
                let lhs = bst::find(case.k2, &insert_mut(mutation, case.k1, case.v1, t.clone()));
                let rhs = if case.k1 == case.k2 { Some(case.v1) } else { bst::find(case.k2, &t) };
                Some(lhs == rhs)
            }
        },
        Property::DeletePost => {
            let t = case.t1.clone();
            if !is_bst(&t) {
                None
            } else {
                let lhs = bst::find(case.k2, &delete_mut(mutation, case.k1, t.clone()));
                let rhs = if case.k1 == case.k2 { None } else { bst::find(case.k2, &t) };
                Some(lhs == rhs)
            }
        },
        Property::UnionPost => {
            let t1 = case.t1.clone();
            let t2 = case.t2.clone();
            if !is_bst(&t1) || !is_bst(&t2) {
                None
            } else {
                let lhs = bst::find(case.k1, &union_mut(mutation, t1.clone(), t2.clone()));
                let rhs = bst::find(case.k1, &t1).or(bst::find(case.k1, &t2));
                Some(lhs == rhs)
            }
        },
        Property::InsertModel => {
            let t = case.t1.clone();
            if !is_bst(&t) {
                None
            } else {
                let lhs = to_list(&insert_mut(mutation, case.k1, case.v1, t.clone()));
                let rhs = l_insert((case.k1, case.v1), &delete_key(case.k1, &to_list(&t)));
                Some(lhs == rhs)
            }
        },
        Property::DeleteModel => {
            let t = case.t1.clone();
            if !is_bst(&t) {
                None
            } else {
                let lhs = to_list(&delete_mut(mutation, case.k1, t.clone()));
                let rhs = delete_key(case.k1, &to_list(&t));
                Some(lhs == rhs)
            }
        },
        Property::UnionModel => {
            let t1 = case.t1.clone();
            let t2 = case.t2.clone();
            if !is_bst(&t1) || !is_bst(&t2) {
                None
            } else {
                let lhs = to_list(&union_mut(mutation, t1.clone(), t2.clone()));
                let rhs = l_sort(&l_union_by(|x, _| x, &to_list(&t1), &to_list(&t2)));
                Some(lhs == rhs)
            }
        },
        Property::InsertInsert => {
            let t = case.t1.clone();
            if !is_bst(&t) {
                None
            } else {
                let lhs = insert_mut(
                    mutation,
                    case.k1,
                    case.v1,
                    insert_mut(mutation, case.k2, case.v2, t.clone()),
                );
                let rhs = if case.k1 == case.k2 {
                    insert_mut(mutation, case.k1, case.v1, t.clone())
                } else {
                    insert_mut(
                        mutation,
                        case.k2,
                        case.v2,
                        insert_mut(mutation, case.k1, case.v1, t.clone()),
                    )
                };
                Some(lhs_to_list_eq(&lhs, &rhs))
            }
        },
        Property::InsertDelete => {
            let t = case.t1.clone();
            if !is_bst(&t) {
                None
            } else {
                let lhs = insert_mut(
                    mutation,
                    case.k1,
                    case.v1,
                    delete_mut(mutation, case.k2, t.clone()),
                );
                let rhs = if case.k1 == case.k2 {
                    insert_mut(mutation, case.k1, case.v1, t.clone())
                } else {
                    delete_mut(mutation, case.k2, insert_mut(mutation, case.k1, case.v1, t.clone()))
                };
                Some(lhs_to_list_eq(&lhs, &rhs))
            }
        },
        Property::InsertUnion => {
            let t1 = case.t1.clone();
            let t2 = case.t2.clone();
            if !is_bst(&t1) || !is_bst(&t2) {
                None
            } else {
                let lhs = insert_mut(
                    mutation,
                    case.k1,
                    case.v1,
                    union_mut(mutation, t1.clone(), t2.clone()),
                );
                let rhs = union_mut(
                    mutation,
                    insert_mut(mutation, case.k1, case.v1, t1.clone()),
                    t2.clone(),
                );
                Some(lhs_to_list_eq(&lhs, &rhs))
            }
        },
        Property::DeleteInsert => {
            let t = case.t1.clone();
            if !is_bst(&t) {
                None
            } else {
                let lhs = delete_mut(
                    mutation,
                    case.k1,
                    insert_mut(mutation, case.k2, case.v1, t.clone()),
                );
                let rhs = if case.k1 == case.k2 {
                    delete_mut(mutation, case.k1, t.clone())
                } else {
                    insert_mut(mutation, case.k2, case.v1, delete_mut(mutation, case.k1, t.clone()))
                };
                Some(lhs_to_list_eq(&lhs, &rhs))
            }
        },
        Property::DeleteDelete => {
            let t = case.t1.clone();
            if !is_bst(&t) {
                None
            } else {
                let lhs = delete_mut(mutation, case.k1, delete_mut(mutation, case.k2, t.clone()));
                let rhs = delete_mut(mutation, case.k2, delete_mut(mutation, case.k1, t.clone()));
                Some(lhs_to_list_eq(&lhs, &rhs))
            }
        },
        Property::DeleteUnion => {
            let t1 = case.t1.clone();
            let t2 = case.t2.clone();
            if !is_bst(&t1) || !is_bst(&t2) {
                None
            } else {
                let lhs =
                    delete_mut(mutation, case.k1, union_mut(mutation, t1.clone(), t2.clone()));
                let rhs = union_mut(
                    mutation,
                    delete_mut(mutation, case.k1, t1.clone()),
                    delete_mut(mutation, case.k1, t2.clone()),
                );
                Some(lhs_to_list_eq(&lhs, &rhs))
            }
        },
        Property::UnionDeleteInsert => {
            let t1 = case.t1.clone();
            let t2 = case.t2.clone();
            if !is_bst(&t1) || !is_bst(&t2) {
                None
            } else {
                let lhs = union_mut(
                    mutation,
                    delete_mut(mutation, case.k1, t1.clone()),
                    insert_mut(mutation, case.k1, case.v1, t2.clone()),
                );
                let rhs = insert_mut(
                    mutation,
                    case.k1,
                    case.v1,
                    union_mut(mutation, t1.clone(), t2.clone()),
                );
                Some(lhs_to_list_eq(&lhs, &rhs))
            }
        },
        Property::UnionUnionAssoc => {
            let t1 = case.t1.clone();
            let t2 = case.t2.clone();
            let t3 = case.t3.clone();
            if !is_bst(&t1) || !is_bst(&t2) || !is_bst(&t3) {
                None
            } else {
                let lhs =
                    union_mut(mutation, t1.clone(), union_mut(mutation, t2.clone(), t3.clone()));
                let rhs =
                    union_mut(mutation, union_mut(mutation, t1.clone(), t2.clone()), t3.clone());
                Some(lhs_to_list_eq(&lhs, &rhs))
            }
        },
    }
}

#[cfg(feature = "tracing")]
fn lhs_to_list_eq(lhs: &Tree, rhs: &Tree) -> bool {
    to_list(lhs) == to_list(rhs)
}

#[cfg(feature = "tracing")]
fn insert_mut(mutation: Mutation, k: i32, v: i32, t: Tree) -> Tree {
    use Tree::*;

    match mutation {
        Mutation::Insert1 | Mutation::Insert2 | Mutation::Insert3 => {
            match t {
                E => T(Box::new(E), k, v, Box::new(E)),
                T(l, k2, v2, r) => {
                    match mutation {
                        Mutation::Insert1 => T(Box::new(E), k, v, Box::new(E)),
                        Mutation::Insert2 => {
                            if k < k2 {
                                T(Box::new(insert_mut(mutation, k, v, *l)), k2, v2, r)
                            } else {
                                T(l, k2, v, r)
                            }
                        },
                        Mutation::Insert3 => {
                            if k < k2 {
                                T(Box::new(insert_mut(mutation, k, v, *l)), k2, v2, r)
                            } else if k2 < k {
                                T(l, k2, v2, Box::new(insert_mut(mutation, k, v, *r)))
                            } else {
                                T(l, k2, v2, r)
                            }
                        },
                        _ => unreachable!(),
                    }
                },
            }
        },
        _ => bst::insert(k, v, t),
    }
}

#[cfg(feature = "tracing")]
fn delete_mut(mutation: Mutation, k: i32, t: Tree) -> Tree {
    use Tree::*;

    match mutation {
        Mutation::Delete4 | Mutation::Delete5 => {
            match t {
                E => E,
                T(l, k2, v2, r) => {
                    match mutation {
                        Mutation::Delete4 => {
                            let _ = v2;
                            if k < k2 {
                                delete_mut(mutation, k, *l)
                            } else if k2 < k {
                                delete_mut(mutation, k, *r)
                            } else {
                                bst::join(*l, *r)
                            }
                        },
                        Mutation::Delete5 => {
                            if k2 < k {
                                T(Box::new(delete_mut(mutation, k, *l)), k2, v2, r)
                            } else if k < k2 {
                                T(l, k2, v2, Box::new(delete_mut(mutation, k, *r)))
                            } else {
                                bst::join(*l, *r)
                            }
                        },
                        _ => unreachable!(),
                    }
                },
            }
        },
        _ => bst::delete(k, t),
    }
}

#[cfg(feature = "tracing")]
fn union_mut(mutation: Mutation, l: Tree, r: Tree) -> Tree {
    union_mut_fuel(mutation, l, r, FUEL)
}

#[cfg(feature = "tracing")]
fn union_mut_fuel(mutation: Mutation, l: Tree, r: Tree, f: usize) -> Tree {
    use Tree::*;

    if f == 0 {
        return E;
    }
    let f1 = f - 1;

    match mutation {
        Mutation::Union6 | Mutation::Union7 | Mutation::Union8 => {
            match (l, r) {
                (E, r) => r,
                (l, E) => l,
                (T(l1, k1, v1, r1), T(l2, k2, v2, r2)) => {
                    match mutation {
                        Mutation::Union6 => {
                            T(
                                l1,
                                k1,
                                v1,
                                Box::new(T(
                                    Box::new(union_mut_fuel(mutation, *r1, *l2, f1)),
                                    k2,
                                    v2,
                                    r2,
                                )),
                            )
                        },
                        Mutation::Union7 => {
                            if k1 == k2 {
                                T(
                                    Box::new(union_mut_fuel(mutation, *l1, *l2, f1)),
                                    k1,
                                    v1,
                                    Box::new(union_mut_fuel(mutation, *r1, *r2, f1)),
                                )
                            } else if k1 < k2 {
                                T(
                                    l1,
                                    k1,
                                    v1,
                                    Box::new(T(
                                        Box::new(union_mut_fuel(mutation, *r1, *l2, f1)),
                                        k2,
                                        v2,
                                        r2,
                                    )),
                                )
                            } else {
                                union_mut_fuel(mutation, T(l2, k2, v2, r2), T(l1, k1, v1, r1), f1)
                            }
                        },
                        Mutation::Union8 => {
                            if k1 == k2 {
                                T(
                                    Box::new(union_mut_fuel(mutation, *l1, *l2, f1)),
                                    k1,
                                    v1,
                                    Box::new(union_mut_fuel(mutation, *r1, *r2, f1)),
                                )
                            } else if k1 < k2 {
                                T(
                                    Box::new(union_mut_fuel(
                                        mutation,
                                        *l1,
                                        bst::below(k1, *l2.clone()),
                                        f1,
                                    )),
                                    k1,
                                    v1,
                                    Box::new(union_mut_fuel(
                                        mutation,
                                        *r1,
                                        T(Box::new(bst::above(k1, *l2)), k2, v2, r2),
                                        f1,
                                    )),
                                )
                            } else {
                                union_mut_fuel(mutation, T(l2, k2, v2, r2), T(l1, k1, v1, r1), f1)
                            }
                        },
                        _ => unreachable!(),
                    }
                },
            }
        },
        _ => bst::union_(l, r, f),
    }
}

#[cfg(feature = "tracing")]
fn keys(t: &Tree) -> Vec<i32> {
    match t {
        Tree::E => vec![],
        Tree::T(l, k, _, r) => {
            let mut out = vec![*k];
            out.extend(keys(l));
            out.extend(keys(r));
            out
        },
    }
}

#[cfg(feature = "tracing")]
fn is_bst(t: &Tree) -> bool {
    match t {
        Tree::E => true,
        Tree::T(l, k, _, r) => {
            is_bst(l)
                && is_bst(r)
                && keys(l).into_iter().all(|k2| k2 < *k)
                && keys(r).into_iter().all(|k2| k2 > *k)
        },
    }
}

#[cfg(feature = "tracing")]
fn to_list(t: &Tree) -> Vec<(i32, i32)> {
    match t {
        Tree::E => vec![],
        Tree::T(l, k, v, r) => {
            let mut out = to_list(l);
            out.push((*k, *v));
            out.extend(to_list(r));
            out
        },
    }
}

#[cfg(feature = "tracing")]
fn delete_key(k: i32, xs: &[(i32, i32)]) -> Vec<(i32, i32)> {
    xs.iter().filter(|(k2, _)| *k2 != k).copied().collect()
}

#[cfg(feature = "tracing")]
fn l_insert((k, v): (i32, i32), xs: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let mut inserted = false;
    let mut out = Vec::with_capacity(xs.len() + 1);
    for &(k2, v2) in xs {
        if !inserted && k < k2 {
            out.push((k, v));
            inserted = true;
        }
        if k == k2 && !inserted {
            out.push((k, v));
            inserted = true;
        } else {
            out.push((k2, v2));
        }
    }
    if !inserted {
        out.push((k, v));
    }
    out
}

#[cfg(feature = "tracing")]
fn l_sort(xs: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let mut out = vec![];
    for &(k, v) in xs {
        out = l_insert((k, v), &out);
    }
    out
}

#[cfg(feature = "tracing")]
fn l_find(k: i32, xs: &[(i32, i32)]) -> Option<i32> {
    xs.iter().find(|(k2, _)| *k2 == k).map(|(_, v)| *v)
}

#[cfg(feature = "tracing")]
fn l_union_by<F>(f: F, l1: &[(i32, i32)], l2: &[(i32, i32)]) -> Vec<(i32, i32)>
where
    F: Fn(i32, i32) -> i32,
{
    let mut result = l2.to_vec();
    for &(k, v) in l1 {
        result.retain(|(k2, _)| *k2 != k);
        let v2 = l_find(k, l2).map(|existing| f(v, existing)).unwrap_or(v);
        result = l_insert((k, v2), &result);
    }
    result
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
    eprintln!(
        "Run with: cargo run --example traced_shrinking_bst_insert_post --features tracing -- <mutation> <property>"
    );
}
