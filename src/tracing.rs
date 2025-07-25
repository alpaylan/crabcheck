// Tracing Generators

use std::{
    any::Any,
    fmt::Debug,
    ops::{Deref, RangeBounds, Sub},
};

use rand::{
    Rng,
    rngs::{self, ThreadRng},
};

use crate::tracing;

#[derive(Clone, Debug)]
enum Tree<T> {
    Leaf(T),
    Node(T, Box<Tree<T>>, Box<Tree<T>>),
}

struct Gen<T> {
    g: Box<dyn FnMut(usize, &mut rngs::ThreadRng) -> T>,
}

impl<T: Clone + 'static> Gen<T> {
    fn ret(t: T) -> Self {
        Gen { g: Box::new(move |_, _| t.clone()) }
    }

    fn bind<U>(self, mut f: impl FnMut(T) -> Gen<U> + 'static) -> Gen<U> {
        let mut g = self.g;
        Gen { g: Box::new(move |size, rng| (f(g(size, rng)).g)(size, rng)) }
    }
}

trait Arbitrary: Clone + 'static {
    fn arbitrary() -> Gen<Self> {
        Gen::bind(Choose::choose(0..=100), |n| Self::arbitrary_sized(n))
    }

    fn arbitrary_sized(n: usize) -> Gen<Self>;
}

trait Choose {
    fn choose(range: std::ops::RangeInclusive<Self>) -> Gen<Self>
    where
        Self: Sized + 'static;
}

impl Choose for usize {
    fn choose(range: std::ops::RangeInclusive<Self>) -> Gen<Self> {
        Gen { g: Box::new(move |_, rng| rng.gen_range(range.clone())) }
    }
}

impl Arbitrary for usize {
    fn arbitrary_sized(n: usize) -> Gen<Self> {
        Choose::choose(0..=n)
    }
}

impl<T: Arbitrary + Clone + 'static> Arbitrary for Tree<T> {
    fn arbitrary_sized(n: usize) -> Gen<Tree<T>> {
        match n {
            0 => Gen::bind(T::arbitrary(), |t| Gen::ret(Tree::Leaf(t))),
            _ => Gen::bind(T::arbitrary(), move |t: T| {
                Gen::bind(Tree::<T>::arbitrary_sized(n - 1), move |left: Tree<T>| {
                    let t = t.clone();
                    Gen::bind(Tree::<T>::arbitrary_sized(n - 1), move |right: Tree<T>| {
                        Gen::ret(Tree::Node(
                            t.clone(),
                            Box::new(left.clone()),
                            Box::new(right.clone()),
                        ))
                    })
                })
            }),
        }
    }
}

// struct Trace<T> {
//     id: Id,
//     value: T,
// }

#[derive(Copy, Clone, Debug)]
struct Id(usize);

impl Deref for Id {
    type Target = usize;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl Id {
    fn new() -> Self {
        static mut ID: usize = 0;
        unsafe {
            ID += 1;
            Id(ID)
        }
    }
}

#[derive(Clone, Debug)]
enum TrTree<T> {
    TrLeaf(Option<Id>, T),
    TrNode(Option<Id>, T, Box<TrTree<T>>, Box<TrTree<T>>),
}

trait LiftBack<T> {
    fn lift_back(self) -> T
    where
        Self: Sized;
}

impl<T1: LiftBack<T2>, T2> LiftBack<Tree<T2>> for TrTree<T1> {
    fn lift_back(self) -> Tree<T2> {
        match self {
            TrTree::TrLeaf(_, t) => Tree::Leaf(t.lift_back()),
            TrTree::TrNode(_, t, left, right) => {
                Tree::Node(t.lift_back(), Box::new(left.lift_back()), Box::new(right.lift_back()))
            },
        }
    }
}

impl LiftBack<usize> for TrUsize {
    fn lift_back(self) -> usize {
        self.n
    }
}

impl<T: Traced> Traced for TrTree<T> {
    fn id(&self) -> Option<Id> {
        match self {
            TrTree::TrLeaf(id, _) => id.clone(),
            TrTree::TrNode(id, _, _, _) => id.clone(),
        }
    }
}

#[derive(Clone, Debug)]
enum Val {
    Id(Id),
    Val(String),
}

#[derive(Clone)]
enum Trace {
    // Bind(Id, Val),
    Ret(Id, ThreadRng, Val),
    Choose(Id, ThreadRng, Val, Val),
    // OneOf(Id, Vec<Val>),
    // Freq(Id, Vec<(u32, Val)>),
}

impl Trace {
    fn id(&self) -> Id {
        match self {
            // Self::Bind(id, _)
            | Self::Ret(id, _, _)
            | Self::Choose(id, _, _, _)
            // | Self::OneOf(id, _)
            // | Self::Freq(id, _) 
            => id.clone(),
        }
    }
    // fn bind(value: Val) -> Self {
    //     Self::Bind(Id::new(), value)
    // }
    fn ret(value: Val, rng: &ThreadRng) -> Self {
        Self::Ret(Id::new(), rng.clone(), value)
    }
    fn choose(lo: Val, hi: Val, rng: &ThreadRng) -> Self {
        Self::Choose(Id::new(), rng.clone(), lo, hi)
    }
    // fn one_of(vals: Vec<Val>) -> Self {
    //     Self::OneOf(Id::new(), vals)
    // }
    // fn freq(choices: Vec<(u32, Val)>) -> Self {
    //     Self::Freq(Id::new(), choices)
    // }
}

struct Traces(Vec<Trace>);

impl Traces {
    fn one(trace: Trace) -> Self {
        Traces(vec![trace])
    }
}

impl From<usize> for TrUsize {
    fn from(n: usize) -> Self {
        TrUsize { id: None, n }
    }
}

struct TrGen<T: Traced> {
    g: Box<dyn FnMut(usize, &mut rngs::ThreadRng) -> (T, Traces)>,
}

impl<T: Traced> TrGen<T> {
    fn ret(t: T) -> Self {
        TrGen {
            g: Box::new(move |_, rng| {
                let trace =
                    t.id().map(|id| Trace::Ret(id, rng.clone(), Val::Val(format!("{:?}", t))));
                (t.clone(), Traces(trace.clone().map_or(vec![], |t| vec![t])))
            }),
        }
    }

    fn bind<U: Traced>(mut self, mut f: impl FnMut(T) -> TrGen<U> + 'static) -> TrGen<U> {
        TrGen {
            g: Box::new(move |size, rng| {
                let g = &mut self.g;
                let mut gen_first = move |size, rng| {
                    let (value, traces) = g(size, rng);
                    (value, traces)
                };

                let (value, traces) = gen_first(size, rng);
                let mut new_gen = f(value);
                let (next_value, next_traces) = (new_gen.g)(size, rng);
                let mut combined_traces = traces.0;
                combined_traces.extend(next_traces.0);
                (next_value, Traces(combined_traces))
            }),
        }
    }
}

trait Traced: Debug + Clone + 'static {
    fn id(&self) -> Option<Id>;
}

trait TrArbitrary: Traced {
    fn arbitrary() -> TrGen<Self> {
        TrGen::bind(TrChoose::choose(0.into(), 100.into()), |n: TrUsize| {
            <Self as TrArbitrary>::arbitrary_sized(n)
        })
    }

    fn arbitrary_sized(n: TrUsize) -> TrGen<Self>;
}

trait TrChoose: Traced {
    fn choose(lo: Self, hi: Self) -> TrGen<Self>;
}

fn one_of<T: Traced>(choices: &'static mut Vec<TrGen<T>>) -> TrGen<T> {
    if choices.is_empty() {
        panic!("Cannot use `one_of` from an empty set");
    }

    // let trace = Trace::one_of(choices.iter().map(|c| Val::Id(c.id)).collect());
    // let id = trace.id();

    TrGen {
        g: Box::new(move |_, rng: &mut rngs::ThreadRng| {
            let index = rng.gen_range(0..choices.len());
            let choice = &mut choices[index];
            let (value, mut traces) = (choice.g)(0, rng);
            // traces.0.push(trace.clone());
            (value, traces)
        }),
    }
}

fn freq<T: Traced>(mut choices: Vec<(u32, TrGen<T>)>) -> TrGen<T> {
    if choices.is_empty() {
        panic!("Cannot use `freq` from an empty set");
    }

    let total_weight: u32 = choices.iter().map(|(weight, _)| weight).sum();
    // let trace = Trace::freq(
    //     choices.iter().map(|(weight, val)| (weight.clone(), Val::Id(val.id))).collect(),
    // );
    // let id = trace.id();

    TrGen {
        g: Box::new(move |_, rng: &mut rngs::ThreadRng| {
            let mut cumulative_weight = 0;
            let choice = choices
                .iter_mut()
                .find(|(weight, _)| {
                    cumulative_weight += weight;
                    cumulative_weight > rng.gen_range(0..total_weight)
                })
                .expect("No choice found in frequency distribution");
            let (value, mut traces) = (choice.1.g)(0, rng);
            // traces.0.insert(0, trace.clone());
            (value, traces)
        }),
    }
}

impl TrChoose for TrUsize {
    fn choose(lo: TrUsize, hi: TrUsize) -> TrGen<Self> {
        TrGen {
            g: Box::new(move |_, rng| {
                let trace = Trace::choose(
                    lo.id.map_or(Val::Val(lo.n.to_string()), |id| Val::Id(id)),
                    hi.id.map_or(Val::Val(hi.n.to_string()), |id| Val::Id(id)),
                    &rng,
                );

                let value = rng.gen_range(lo.n..=hi.n);
                (TrUsize { id: Some(trace.id()), n: value }, Traces(vec![trace.clone()]))
            }),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TrUsize {
    id: Option<Id>,
    n: usize,
}

impl TrUsize {
    fn decr(&self) -> Self {
        TrUsize { id: self.id.clone(), n: self.n - 1 }
    }
}

impl Traced for TrUsize {
    fn id(&self) -> Option<Id> {
        self.id.clone()
    }
}

impl TrArbitrary for TrUsize {
    fn arbitrary_sized(n: TrUsize) -> TrGen<Self> {
        TrChoose::choose(TrUsize { id: None, n: 0 }, n)
    }
}

impl<T: TrArbitrary> TrArbitrary for TrTree<T> {
    fn arbitrary_sized(n: TrUsize) -> TrGen<TrTree<T>> {
        match n.n {
            0 => TrGen::bind(T::arbitrary(), |t| TrGen::ret(TrTree::TrLeaf(Some(Id::new()), t))),
            n_ => freq(vec![
                (
                    1,
                    TrGen::bind(T::arbitrary(), |t| TrGen::ret(TrTree::TrLeaf(Some(Id::new()), t))),
                ),
                (
                    n_ as u32,
                    TrGen::bind(T::arbitrary(), move |t: T| {
                        TrGen::bind(
                            TrTree::<T>::arbitrary_sized(n.decr()),
                            move |left: TrTree<T>| {
                                let t = t.clone();
                                TrGen::bind(
                                    TrTree::<T>::arbitrary_sized(n.decr()),
                                    move |right: TrTree<T>| {
                                        TrGen::ret(TrTree::TrNode(
                                            Some(Id::new()),
                                            t.clone(),
                                            Box::new(left.clone()),
                                            Box::new(right.clone()),
                                        ))
                                    },
                                )
                            },
                        )
                    }),
                ),
            ]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tree_arbitrary() {
        let mut rng = rand::thread_rng();
        // let mut tree_gen = <Tree<usize> as Arbitrary>::arbitrary_sized(3);
        // let tree = (tree_gen.g)(3, &mut rng);

        let mut tree_gen = <TrTree<TrUsize> as TrArbitrary>::arbitrary_sized(3.into());
        let (tree, traces) = (tree_gen.g)(1, &mut rng);

        println!("Generated Tree: {:?}", tree.lift_back());
        println!(
            "Traces: {:#?}",
            traces
                .0
                .iter()
                .map(|t| match t {
                    // Trace::Bind(id, value) => format!("{:?} <- {:?}", id, value),
                    Trace::Ret(id, rng, value) =>
                        format!("{:?} <- {:?} (rng: {:?})", id, value, rng),
                    Trace::Choose(id, rng, lo, hi) =>
                        format!("{:?} <- choose({:?}, {:?}) (rng: {:?}", id, lo, hi, rng),
                    // Trace::OneOf(id, vals) => format!(
                    //     "{:?} <- one_of({})",
                    //     id,
                    //     vals.iter()
                    //         .map(|v| match v {
                    //             Val::Id(id) => format!("{:?}", id),
                    //             Val::Val(val) => val.clone(),
                    //         })
                    //         .collect::<Vec<_>>()
                    //         .join(", ")
                    // ),
                    // Trace::Freq(id, choices) => format!(
                    //     "{:?} <- freq({})",
                    //     id,
                    //     choices
                    //         .iter()
                    //         .map(|(weight, val)| {
                    //             format!(
                    //                 "{}: {:?}",
                    //                 weight,
                    //                 match val {
                    //                     Val::Id(id) => format!("{:?}", id),
                    //                     Val::Val(val) => val.clone(),
                    //                 }
                    //             )
                    //         })
                    //         .collect::<Vec<_>>()
                    //         .join(", ")
                    // ),
                })
                .collect::<Vec<_>>()
        );
    }
}
