//! Binary search trees with an invariant-preserving generator. Run with
//! `cargo run -p crabcheck --example bst` (add `--bug` to plant a bug in
//! `insert` and watch the property fail).
//!
//! The generator builds valid trees directly, by choosing each key inside
//! the range its position allows, and a shrinker keeps them valid: a node
//! shrinks to either child or to a leaf. Every failure replays from its
//! seed and iteration.

use crabcheck::{
    Config,
    GenExt,
    Source,
    TestResult,
    check,
    check_shrinker,
    classify,
    gens,
};

#[derive(Clone, Debug, PartialEq)]
enum Tree {
    Leaf,
    Node(Box<Tree>, i32, i32, Box<Tree>),
}

impl Tree {
    fn insert(&self, key: i32, value: i32, bug: bool) -> Tree {
        match self {
            Tree::Leaf => Tree::Node(Box::new(Tree::Leaf), key, value, Box::new(Tree::Leaf)),
            Tree::Node(l, k, v, r) => {
                if key < *k {
                    Tree::Node(Box::new(l.insert(key, value, bug)), *k, *v, r.clone())
                } else if key > *k {
                    Tree::Node(l.clone(), *k, *v, Box::new(r.insert(key, value, bug)))
                } else if bug {
                    // Planted bug: replacing a key drops its right subtree.
                    Tree::Node(l.clone(), key, value, Box::new(Tree::Leaf))
                } else {
                    Tree::Node(l.clone(), key, value, r.clone())
                }
            },
        }
    }

    fn delete(&self, key: i32) -> Tree {
        match self {
            Tree::Leaf => Tree::Leaf,
            Tree::Node(l, k, v, r) => {
                if key < *k {
                    Tree::Node(Box::new(l.delete(key)), *k, *v, r.clone())
                } else if key > *k {
                    Tree::Node(l.clone(), *k, *v, Box::new(r.delete(key)))
                } else {
                    match (l.as_ref(), r.as_ref()) {
                        (Tree::Leaf, _) => r.as_ref().clone(),
                        (_, Tree::Leaf) => l.as_ref().clone(),
                        _ => {
                            let (mk, mv) = r.min().expect("non-empty");
                            Tree::Node(l.clone(), mk, mv, Box::new(r.delete(mk)))
                        },
                    }
                }
            },
        }
    }

    fn min(&self) -> Option<(i32, i32)> {
        match self {
            Tree::Leaf => None,
            Tree::Node(l, k, v, _) => l.min().or(Some((*k, *v))),
        }
    }

    fn find(&self, key: i32) -> Option<i32> {
        match self {
            Tree::Leaf => None,
            Tree::Node(l, k, v, r) => {
                if key < *k {
                    l.find(key)
                } else if key > *k {
                    r.find(key)
                } else {
                    Some(*v)
                }
            },
        }
    }

    fn to_list(&self) -> Vec<(i32, i32)> {
        match self {
            Tree::Leaf => Vec::new(),
            Tree::Node(l, k, v, r) => {
                let mut out = l.to_list();
                out.push((*k, *v));
                out.extend(r.to_list());
                out
            },
        }
    }

    fn size(&self) -> usize {
        self.to_list().len()
    }
}

fn is_bst(t: &Tree) -> bool {
    t.to_list().windows(2).all(|w| w[0].0 < w[1].0)
}

/// Shrink candidates that are still BSTs: either child, a leaf, or a node
/// with one child shrunk.
fn shrink_bst(t: &Tree) -> Vec<Tree> {
    match t {
        Tree::Leaf => Vec::new(),
        Tree::Node(l, k, v, r) => {
            let mut out = vec![(**l).clone(), (**r).clone(), Tree::Leaf];
            out.extend(
                shrink_bst(l).into_iter().map(|l| Tree::Node(Box::new(l), *k, *v, r.clone())),
            );
            out.extend(
                shrink_bst(r).into_iter().map(|r| Tree::Node(l.clone(), *k, *v, Box::new(r))),
            );
            out
        },
    }
}

/// Generate a valid BST whose keys lie in `lo..=hi`.
fn bst(g: &mut Source, lo: i32, hi: i32, depth: usize) -> Tree {
    if depth == 0 || lo > hi || !g.bool_with(0.8) {
        return Tree::Leaf;
    }
    let key = g.int(lo..=hi);
    let value = g.int(0..=9);
    let left = bst(g, lo, key - 1, depth - 1);
    let right = bst(g, key + 1, hi, depth - 1);
    Tree::Node(Box::new(left), key, value, Box::new(right))
}

fn main() {
    let bug = std::env::args().any(|a| a == "--bug");
    // The planted bug needs two coincidences (inserting an existing key, then
    // looking up a key in its right subtree), so give the search room.
    let config = Config::default().tests(1000).name("bst");
    let trees = (|g: &mut Source| bst(g, -20, 20, 6)).with_shrink(shrink_bst).boxed();

    // The shrinker is code, so verify it: every candidate is a BST with keys in
    // range, and strictly fewer nodes.
    let in_support =
        |t: &Tree| is_bst(t) && t.to_list().iter().all(|(k, _)| (-20..=20).contains(k));
    let verified = check_shrinker(&config, trees.clone(), in_support, |t| t.size() as u64);
    print!("{verified}");
    assert!(verified.passed());
    let key = gens::int(-25i32..=25);
    let value = gens::int(0..=9);

    let insert_valid = check(&config, (trees.clone(), key, value), |(t, k, v)| {
        classify(t.size() == 0, "empty tree");
        classify(t.find(k).is_some(), "key present");
        TestResult::check(is_bst(&t.insert(k, v, bug)), "insert breaks the BST invariant")
    });
    print!("{insert_valid}");

    let insert_post = check(&config, (trees.clone(), key, key, value), |(t, k, k2, v)| {
        let inserted = t.insert(k, v, bug);
        let expected = if k == k2 { Some(v) } else { t.find(k2) };
        TestResult::check(inserted.find(k2) == expected, "insert changes other keys")
    });
    print!("{insert_post}");

    let delete_valid = check(&config, (trees.clone(), key), |(t, k)| is_bst(&t.delete(k)));
    print!("{delete_valid}");

    let delete_post = check(&config, (trees.clone(), key, key), |(t, k, k2)| {
        let deleted = t.delete(k);
        let expected = if k == k2 { None } else { t.find(k2) };
        deleted.find(k2) == expected
    });
    print!("{delete_post}");

    let all_passed =
        [&insert_valid, &insert_post, &delete_valid, &delete_post].iter().all(|r| r.passed());
    if bug {
        assert!(!all_passed, "the planted bug should have been found");
    } else {
        assert!(all_passed);
    }
}
