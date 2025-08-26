use {
    bst::Tree,
    crabcheck::quickcheck::{
        Arbitrary,
        Mutate,
    },
    rand::Rng,
    spec::{
        prop_delete_model,
        prop_delete_post,
        prop_delete_valid,
        prop_insert_model,
        prop_insert_post,
        prop_insert_valid,
        prop_union_model,
        prop_union_post,
        prop_union_valid,
    },
    std::i32,
};

#[cfg(feature = "profiling")]
use crabcheck::profiling::quickcheck;
#[cfg(not(feature = "profiling"))]
use crabcheck::quickcheck::quickcheck;

pub mod bst;
pub mod spec;

fn gen_tree<R: Rng>(r: &mut R, size: u32, lo: i32, hi: i32) -> Tree {
    if size == 0 || hi - lo <= 1 {
        return Tree::E;
    }
    let k = r.gen_range(lo + 1..hi);
    let left = gen_tree(r, size - 1, lo, k);
    let right = gen_tree(r, size - 1, k + 1, hi);
    Tree::T(Box::new(left), k, k, Box::new(right))
}

impl<R: Rng> Arbitrary<R> for Tree {
    fn generate(r: &mut R, n: usize) -> Self {
        gen_tree(r, (n as f32).log2() as u32, -(n as i32), n as i32)
    }
}

fn mut_tree<R: Rng>(rng: &mut R, t: &Tree, n: usize, lo: i32, hi: i32) -> Tree {
    if n == 0 || hi <= lo + 1 {
        return Tree::E;
    }
    match t {
        Tree::E => Tree::E,
        Tree::T(l, k, v, r) => {
            let choice = rng.gen_range(0..=3);
            match choice {
                0 => {
                    // just mutate the value
                    let new_v = v.mutate(rng, n);
                    Tree::T(l.clone(), *k, new_v, r.clone())
                },
                1 => {
                    // mutate the key between the left and right
                    let left_root =
                        if let Tree::T(_, k, _, _) = l.as_ref() { *k } else { i32::MIN.max(lo) };

                    let right_root =
                        if let Tree::T(_, k, _, _) = r.as_ref() { *k } else { i32::MAX.min(hi) };

                    if left_root + 1 >= right_root {
                        return Tree::E;
                    }

                    let new_k = rng.gen_range(left_root + 1..right_root);
                    Tree::T(l.clone(), new_k, *v, r.clone())
                },
                2 => {
                    // mutate the left tree
                    let new_l = mut_tree(rng, &l, n - 1, lo, *k);
                    Tree::T(Box::new(new_l), *k, *v, r.clone())
                },
                _ => {
                    // mutate the right tree
                    let new_r = mut_tree(rng, &r, n - 1, *k + 1, hi);
                    Tree::T(l.clone(), *k, *v, Box::new(new_r))
                },
            }
        },
    }
}

impl<R: Rng> Mutate<R> for Tree {
    fn mutate(&self, rng: &mut R, n: usize) -> Self {
        mut_tree(rng, self, n, i32::MIN, i32::MAX)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        panic!("Usage: {} <input>", args[0]);
    }
    let input = &args[1];
    let input = input.as_str();

    let r = match input {
        "insert_valid" => quickcheck(|(t, k, v)| prop_insert_valid(t, k, v)),
        "delete_valid" => quickcheck(|(t, k)| prop_delete_valid(&t, k)),
        "union_valid" => quickcheck(|(t1, t2)| prop_union_valid(&t1, &t2)),
        "insert_post" => quickcheck(|(t, k, k2, v)| prop_insert_post(t, k, k2, v)),
        "delete_post" => quickcheck(|(t, k, k2)| prop_delete_post(t, k, k2)),
        "union_post" => quickcheck(|(t1, t2, k)| prop_union_post(&t1, &t2, k)),
        "insert_model" => quickcheck(|(t, k, v)| prop_insert_model(&t, k, v)),
        "delete_model" => quickcheck(|(t, k)| prop_delete_model(&t, k)),
        "union_model" => quickcheck(|(t1, t2)| prop_union_model(&t1, &t2)),
        _ => panic!("Unknown input"),
    };

    assert!(r.counterexample.is_some(), "bug is not triggered");
    Ok(())
}
