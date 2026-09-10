//! The [`Gen`] trait, generator combinators, [`BoxGen`], and the
//! constructors for the built-in generators.
//!
//! A generator is anything that can produce a value from a [`Source`].
//! Closures `Fn(&mut Source) -> T` are generators, so the quickest way to
//! write one is a function:
//!
//! ```
//! use crabcheck::{gens, Gen, GenExt, Source};
//!
//! fn pair(g: &mut Source) -> (u8, u8) {
//!     let lo = g.int(0u8..=100);
//!     let hi = g.int(lo..=100);
//!     (lo, hi)
//! }
//!
//! let sorted_pairs = gens::vec(pair).map(|mut v| { v.sort(); v });
//! let mut source = Source::new(1, 10);
//! let v = sorted_pairs.generate(&mut source);
//! assert!(v.windows(2).all(|w| w[0] <= w[1]));
//! ```
//!
//! A generator also owns its **shrinker** and **mutator** ([`Gen::shrink`],
//! [`Gen::mutate`]). The built-in generators come with both, kept inside
//! their own range; a closure has neither until you attach them with
//! [`GenExt::with_shrink`] / [`GenExt::with_mutate`], or borrow the value
//! type's [`Shrink`] / [`Mutate`] impls with [`GenExt::typed`]. The
//! checkers [`check_shrinker`](crate::check_shrinker) and
//! [`check_mutator`](crate::check_mutator) verify what you attach.

use std::{
    fmt,
    marker::PhantomData,
    sync::Arc,
};

pub use crate::primitives::*;
use crate::{
    Source,
    hooks::{
        Mutate,
        Shrink,
    },
};

/// Something that produces values from a [`Source`], and knows how to make
/// them simpler (shrink) or nearby (mutate).
///
/// Implemented for closures `Fn(&mut Source) -> T + Send + Sync` (generation
/// only), for [`BoxGen`], for tuples and arrays of generators, and for the
/// types returned by the constructors in this module.
pub trait Gen: Send + Sync {
    /// The type of value produced.
    type Value;

    /// Produce a value, drawing every decision from `g`.
    fn generate(&self, g: &mut Source) -> Self::Value;

    /// Simpler candidates for `value`, most promising first. Candidates
    /// should stay inside this generator's support and be strictly smaller.
    /// Default: none.
    fn shrink(&self, _value: &Self::Value) -> Vec<Self::Value> {
        Vec::new()
    }

    /// A nearby variant of `value` for targeted search, drawing randomness
    /// from `g`. `None` when no mutation applies. Default: `None`.
    fn mutate(&self, _value: &Self::Value, _g: &mut Source) -> Option<Self::Value> {
        None
    }
}

impl<T, F> Gen for F
where
    F: Fn(&mut Source) -> T + Send + Sync,
{
    type Value = T;

    fn generate(&self, g: &mut Source) -> T {
        self(g)
    }
}

/// A shareable, type-erased generator of `T`. Cloning is cheap (it is an
/// `Arc`).
pub struct BoxGen<T>(Arc<dyn Gen<Value = T>>);

impl<T> BoxGen<T> {
    /// Erase `generator`'s type.
    pub fn new(generator: impl Gen<Value = T> + 'static) -> Self {
        BoxGen(Arc::new(generator))
    }
}

impl<T> Clone for BoxGen<T> {
    fn clone(&self) -> Self {
        BoxGen(Arc::clone(&self.0))
    }
}

impl<T> fmt::Debug for BoxGen<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BoxGen<{}>", std::any::type_name::<T>())
    }
}

impl<T> Gen for BoxGen<T> {
    type Value = T;

    fn generate(&self, g: &mut Source) -> T {
        self.0.generate(g)
    }

    fn shrink(&self, value: &T) -> Vec<T> {
        self.0.shrink(value)
    }

    fn mutate(&self, value: &T, g: &mut Source) -> Option<T> {
        self.0.mutate(value, g)
    }
}

/// Combinators available on every generator.
pub trait GenExt: Gen + Sized {
    /// Apply `f` to every generated value. The result has no shrinker or
    /// mutator of its own, because `f` cannot be inverted: use [`iso`](Self::iso)
    /// when it can be, [`typed`](Self::typed) when the output type has hooks,
    /// or attach hooks with [`with_shrink`](Self::with_shrink) and
    /// [`with_mutate`](Self::with_mutate).
    fn map<U, F>(self, f: F) -> Map<Self, F>
    where
        F: Fn(Self::Value) -> U + Send + Sync,
    {
        Map { inner: self, f }
    }

    /// Apply a bijection: `forward` on generation, `back` before shrinking or
    /// mutating, so the inner generator's hooks carry over. Ideal for
    /// newtypes: `gens::int(1u8..=6).iso(Die, |d| d.0)`.
    fn iso<U, F, B>(self, forward: F, back: B) -> Iso<Self, F, B>
    where
        F: Fn(Self::Value) -> U + Send + Sync,
        B: Fn(&U) -> Self::Value + Send + Sync,
    {
        Iso { inner: self, forward, back }
    }

    /// Generate a value, then generate the final value with a generator
    /// chosen from it. No hooks of its own: the first value is not kept, so
    /// nothing can be shrunk within the second generator's support. Use
    /// [`dependent`](GenExt::dependent) to keep both values and their hooks,
    /// or `map`/`iso` when a pure function will do.
    fn and_then<H, F>(self, f: F) -> AndThen<Self, F>
    where
        H: Gen,
        F: Fn(Self::Value) -> H + Send + Sync,
    {
        AndThen { inner: self, f }
    }

    /// Pair each value with one drawn from a generator chosen by it, keeping
    /// both and their hooks. See [`dependent`].
    fn dependent<H, F>(self, f: F) -> Dependent<Self, F>
    where
        Self::Value: Clone,
        H: Gen,
        F: Fn(&Self::Value) -> H + Send + Sync,
    {
        Dependent { outer: self, f }
    }

    /// Keep only values for which `predicate` holds. After
    /// [`Source::filter_max_attempts`] rejections the input is
    /// [discarded](Source::discard) with reason `filter:<label>`. Shrink and
    /// mutation candidates are filtered by the same predicate.
    fn filter<F>(self, label: &'static str, predicate: F) -> Filter<Self, F>
    where
        F: Fn(&Self::Value) -> bool + Send + Sync,
    {
        Filter { inner: self, label, predicate }
    }

    /// Attach a shrinker: `candidates(value)` are tried before the inner
    /// generator's own candidates.
    fn with_shrink<S>(self, candidates: S) -> WithShrink<Self, S>
    where
        S: Fn(&Self::Value) -> Vec<Self::Value> + Send + Sync,
    {
        WithShrink { inner: self, candidates }
    }

    /// Attach a mutator, tried before the inner generator's own.
    fn with_mutate<M>(self, mutator: M) -> WithMutate<Self, M>
    where
        M: Fn(&Self::Value, &mut Source) -> Option<Self::Value> + Send + Sync,
    {
        WithMutate { inner: self, mutator }
    }

    /// Use the value type's [`Shrink`] and [`Mutate`] impls as this
    /// generator's hooks (they replace, not extend, the inner ones).
    fn typed(self) -> Typed<Self>
    where
        Self::Value: Shrink + Mutate,
    {
        Typed { inner: self }
    }

    /// Collect a sequence generator into another collection type, keeping
    /// its hooks: `gens::vec((k, v)).collect::<HashMap<_, _>>()`.
    fn collect<C>(self) -> Collect<Self, C> {
        Collect { inner: self, _c: PhantomData }
    }

    /// Erase the generator's type so it can be stored, cloned and shared.
    fn boxed(self) -> BoxGen<Self::Value>
    where
        Self: 'static,
        Self::Value: 'static,
    {
        BoxGen::new(self)
    }
}

impl<G: Gen> GenExt for G {}

/// See [`GenExt::map`].
pub struct Map<G, F> {
    inner: G,
    f: F,
}

impl<G, U, F> Gen for Map<G, F>
where
    G: Gen,
    F: Fn(G::Value) -> U + Send + Sync,
{
    type Value = U;

    fn generate(&self, g: &mut Source) -> U {
        (self.f)(self.inner.generate(g))
    }
}

/// See [`GenExt::iso`].
pub struct Iso<G, F, B> {
    inner: G,
    forward: F,
    back: B,
}

impl<G, U, F, B> Gen for Iso<G, F, B>
where
    G: Gen,
    F: Fn(G::Value) -> U + Send + Sync,
    B: Fn(&U) -> G::Value + Send + Sync,
{
    type Value = U;

    fn generate(&self, g: &mut Source) -> U {
        (self.forward)(self.inner.generate(g))
    }

    fn shrink(&self, value: &U) -> Vec<U> {
        self.inner.shrink(&(self.back)(value)).into_iter().map(&self.forward).collect()
    }

    fn mutate(&self, value: &U, g: &mut Source) -> Option<U> {
        self.inner.mutate(&(self.back)(value), g).map(&self.forward)
    }
}

/// See [`GenExt::dependent`].
pub struct Dependent<G, F> {
    outer: G,
    f: F,
}

/// Seed of the source that regenerates the dependent half while shrinking;
/// candidates must be a pure function of the value.
const REGENERATE_SEED: u64 = 0x6465_7065_6e64_656e;

impl<G, H, F> Gen for Dependent<G, F>
where
    G: Gen,
    G::Value: Clone,
    H: Gen,
    F: Fn(&G::Value) -> H + Send + Sync,
{
    type Value = (G::Value, H::Value);

    fn generate(&self, g: &mut Source) -> Self::Value {
        let a = self.outer.generate(g);
        let b = (self.f)(&a).generate(g);
        (a, b)
    }

    /// Shrink the first value and regenerate the second from its new
    /// generator at size 0 (the smallest dependent value, which later steps
    /// shrink further), then shrink the second value under its own
    /// generator. Every candidate lies in the joint support.
    fn shrink(&self, (a, b): &Self::Value) -> Vec<Self::Value> {
        let mut out = Vec::new();
        for (i, a2) in self.outer.shrink(a).into_iter().enumerate() {
            let mut source = Source::new(crate::runner::mix(REGENERATE_SEED, i as u64), 0);
            let b2 = (self.f)(&a2).generate(&mut source);
            out.push((a2, b2));
        }
        out.extend((self.f)(a).shrink(b).into_iter().map(|b2| (a.clone(), b2)));
        out
    }

    /// Mutate the second value under its generator, or the first value with
    /// the second regenerated from the new generator; whichever the coin
    /// picks first, falling back to the other.
    fn mutate(&self, (a, b): &Self::Value, g: &mut Source) -> Option<Self::Value> {
        let inner_first = g.bool();
        let inner = |g: &mut Source| (self.f)(a).mutate(b, g).map(|b2| (a.clone(), b2));
        let outer = |g: &mut Source| {
            self.outer.mutate(a, g).map(|a2| {
                let b2 = (self.f)(&a2).generate(g);
                (a2, b2)
            })
        };
        if inner_first { inner(g).or_else(|| outer(g)) } else { outer(g).or_else(|| inner(g)) }
    }
}

/// A pair whose second value is drawn from a generator chosen by the first:
/// `f(&a)` returns the generator for `b`. Both values are kept, so the pair
/// shrinks and mutates within the joint support: `b` under `f(&a)`, and `a`
/// under `outer` with `b` regenerated from `f(&a')`. This is how generation
/// depends on computation without losing the hooks that
/// [`and_then`](GenExt::and_then) drops.
///
/// ```
/// use crabcheck::{check, gens, Config};
///
/// // A non-empty vector and a valid index into it.
/// let indexed = gens::dependent(gens::vec(gens::int(0..=100)).len(1..=8), |v| gens::int(0..v.len()));
/// let report = check(&Config::default().tests(200), indexed, |(v, i)| v[i] <= 100);
/// report.assert_passed();
/// ```
pub fn dependent<G, H, F>(outer: G, f: F) -> Dependent<G, F>
where
    G: Gen,
    G::Value: Clone,
    H: Gen,
    F: Fn(&G::Value) -> H + Send + Sync,
{
    Dependent { outer, f }
}

/// See [`GenExt::and_then`].
pub struct AndThen<G, F> {
    inner: G,
    f: F,
}

impl<G, H, F> Gen for AndThen<G, F>
where
    G: Gen,
    H: Gen,
    F: Fn(G::Value) -> H + Send + Sync,
{
    type Value = H::Value;

    fn generate(&self, g: &mut Source) -> H::Value {
        let outer = self.inner.generate(g);
        (self.f)(outer).generate(g)
    }
}

/// See [`GenExt::filter`].
pub struct Filter<G, F> {
    inner: G,
    label: &'static str,
    predicate: F,
}

impl<G, F> Gen for Filter<G, F>
where
    G: Gen,
    F: Fn(&G::Value) -> bool + Send + Sync,
{
    type Value = G::Value;

    fn generate(&self, g: &mut Source) -> G::Value {
        let max_attempts = g.filter_max_attempts();
        let mut last = None;
        for _ in 0..max_attempts {
            let value = self.inner.generate(g);
            if (self.predicate)(&value) {
                return value;
            }
            last = Some(value);
        }
        g.discard(format!("filter:{}", self.label));
        last.expect("filter_max_attempts is at least 1")
    }

    fn shrink(&self, value: &G::Value) -> Vec<G::Value> {
        self.inner.shrink(value).into_iter().filter(|v| (self.predicate)(v)).collect()
    }

    fn mutate(&self, value: &G::Value, g: &mut Source) -> Option<G::Value> {
        for _ in 0..g.filter_max_attempts() {
            if let Some(m) = self.inner.mutate(value, g)
                && (self.predicate)(&m)
            {
                return Some(m);
            }
        }
        None
    }
}

/// See [`GenExt::with_shrink`].
pub struct WithShrink<G, S> {
    inner: G,
    candidates: S,
}

impl<G, S> Gen for WithShrink<G, S>
where
    G: Gen,
    S: Fn(&G::Value) -> Vec<G::Value> + Send + Sync,
{
    type Value = G::Value;

    fn generate(&self, g: &mut Source) -> G::Value {
        self.inner.generate(g)
    }

    fn shrink(&self, value: &G::Value) -> Vec<G::Value> {
        let mut out = (self.candidates)(value);
        out.extend(self.inner.shrink(value));
        out
    }

    fn mutate(&self, value: &G::Value, g: &mut Source) -> Option<G::Value> {
        self.inner.mutate(value, g)
    }
}

/// See [`GenExt::with_mutate`].
pub struct WithMutate<G, M> {
    inner: G,
    mutator: M,
}

impl<G, M> Gen for WithMutate<G, M>
where
    G: Gen,
    M: Fn(&G::Value, &mut Source) -> Option<G::Value> + Send + Sync,
{
    type Value = G::Value;

    fn generate(&self, g: &mut Source) -> G::Value {
        self.inner.generate(g)
    }

    fn shrink(&self, value: &G::Value) -> Vec<G::Value> {
        self.inner.shrink(value)
    }

    fn mutate(&self, value: &G::Value, g: &mut Source) -> Option<G::Value> {
        (self.mutator)(value, g).or_else(|| self.inner.mutate(value, g))
    }
}

/// See [`GenExt::typed`].
pub struct Typed<G> {
    inner: G,
}

impl<G> Gen for Typed<G>
where
    G: Gen,
    G::Value: Shrink + Mutate,
{
    type Value = G::Value;

    fn generate(&self, g: &mut Source) -> G::Value {
        self.inner.generate(g)
    }

    fn shrink(&self, value: &G::Value) -> Vec<G::Value> {
        value.shrink()
    }

    fn mutate(&self, value: &G::Value, g: &mut Source) -> Option<G::Value> {
        value.mutate(g)
    }
}

/// See [`GenExt::collect`].
pub struct Collect<G, C> {
    inner: G,
    _c: PhantomData<fn() -> C>,
}

impl<T, G, C> Gen for Collect<G, C>
where
    G: Gen<Value = Vec<T>>,
    C: FromIterator<T> + IntoIterator<Item = T> + Clone + Send + Sync,
{
    type Value = C;

    fn generate(&self, g: &mut Source) -> C {
        self.inner.generate(g).into_iter().collect()
    }

    fn shrink(&self, value: &C) -> Vec<C> {
        let items: Vec<T> = value.clone().into_iter().collect();
        self.inner.shrink(&items).into_iter().map(|v| v.into_iter().collect()).collect()
    }

    fn mutate(&self, value: &C, g: &mut Source) -> Option<C> {
        let items: Vec<T> = value.clone().into_iter().collect();
        self.inner.mutate(&items, g).map(|v| v.into_iter().collect())
    }
}

macro_rules! impl_tuple_gen {
    ($(($G:ident, $i:tt)),+) => {
        impl<$($G: Gen,)+> Gen for ($($G,)+)
        where
            $($G::Value: Clone,)+
        {
            type Value = ($($G::Value,)+);

            fn generate(&self, g: &mut Source) -> Self::Value {
                ($(self.$i.generate(g),)+)
            }

            fn shrink(&self, value: &Self::Value) -> Vec<Self::Value> {
                let mut out = Vec::new();
                $(
                    for smaller in self.$i.shrink(&value.$i) {
                        let mut candidate = value.clone();
                        candidate.$i = smaller;
                        out.push(candidate);
                    }
                )+
                out
            }

            fn mutate(&self, value: &Self::Value, g: &mut Source) -> Option<Self::Value> {
                let arity = [$($i,)+].len();
                let start = g.index(arity);
                for offset in 0..arity {
                    let which = (start + offset) % arity;
                    $(
                        if which == $i && let Some(m) = self.$i.mutate(&value.$i, g) {
                            let mut out = value.clone();
                            out.$i = m;
                            return Some(out);
                        }
                    )+
                }
                None
            }
        }
    };
}

impl_tuple_gen!((A, 0));
impl_tuple_gen!((A, 0), (B, 1));
impl_tuple_gen!((A, 0), (B, 1), (C, 2));
impl_tuple_gen!((A, 0), (B, 1), (C, 2), (D, 3));
impl_tuple_gen!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4));
impl_tuple_gen!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5));
impl_tuple_gen!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6));
impl_tuple_gen!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7));

impl<G: Gen, const N: usize> Gen for [G; N]
where
    G::Value: Clone,
{
    type Value = [G::Value; N];

    fn generate(&self, g: &mut Source) -> Self::Value {
        std::array::from_fn(|i| self[i].generate(g))
    }

    fn shrink(&self, value: &Self::Value) -> Vec<Self::Value> {
        let mut out = Vec::new();
        for i in 0..N {
            for smaller in self[i].shrink(&value[i]) {
                let mut candidate = value.clone();
                candidate[i] = smaller;
                out.push(candidate);
            }
        }
        out
    }

    fn mutate(&self, value: &Self::Value, g: &mut Source) -> Option<Self::Value> {
        if N == 0 {
            return None;
        }
        let i = g.index(N);
        let mutated = self[i].mutate(&value[i], g)?;
        let mut out = value.clone();
        out[i] = mutated;
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::gens,
    };

    #[test]
    fn closures_tuples_and_arrays_are_generators() {
        let mut g = Source::new(1, 10);
        let pair = (|g: &mut Source| g.int(0..=5), gens::bool());
        let (a, b) = pair.generate(&mut g);
        assert!((0..=5).contains(&a));
        let _: bool = b;
        let arr = [gens::int(0u8..=1), gens::int(0u8..=1), gens::int(0u8..=1)].generate(&mut g);
        assert!(arr.iter().all(|&x| x <= 1));
        // A closure has no shrinker; the tuple still shrinks its other parts.
        assert_eq!(pair.shrink(&(3, true)), vec![(3, false)]);
        assert_eq!(arr.len(), 3);
    }

    #[test]
    fn iso_carries_hooks_through_a_bijection() {
        #[derive(Debug, Clone, PartialEq)]
        struct Die(u8);
        let die = gens::int(1u8..=6).iso(Die, |d| d.0);
        assert_eq!(die.shrink(&Die(6)), vec![Die(1), Die(4), Die(5)]);
        let mut g = Source::new(2, 1);
        for _ in 0..50 {
            let m = die.mutate(&Die(3), &mut g).unwrap();
            assert!((1..=6).contains(&m.0));
        }
        assert!(gens::int(1u8..=6).map(Die).shrink(&Die(6)).is_empty(), "map has no shrinker");
    }

    #[test]
    fn filter_retries_then_discards_and_filters_hooks() {
        let mut g = Source::new(3, 10).with_filter_max_attempts(5);
        let never = gens::int(0..=9).filter("never", |_| false);
        let _ = never.generate(&mut g);
        assert_eq!(g.discarded(), Some("filter:never"));
        let even = gens::int(0..=9).filter("even", |x| x % 2 == 0);
        let mut g = Source::new(4, 10);
        for _ in 0..50 {
            assert_eq!(even.generate(&mut g) % 2, 0);
        }
        assert!(even.shrink(&8).iter().all(|x| x % 2 == 0));
        assert!(even.shrink(&8).contains(&0));
        for _ in 0..50 {
            if let Some(m) = even.mutate(&8, &mut g) {
                assert_eq!(m % 2, 0);
            }
        }
    }

    #[test]
    fn with_shrink_typed_and_collect() {
        let generator = gens::int(0..=100).with_shrink(|v: &i32| vec![v / 2, v - 1]);
        assert_eq!(generator.shrink(&10)[..2], [5, 9]);
        let boxed = generator.boxed();
        assert!(boxed.clone().shrink(&10).len() >= 2);

        let typed = (|g: &mut Source| g.int(0i32..=100)).typed();
        assert_eq!(typed.shrink(&4), vec![0, 2, 3]);
        let mut g = Source::new(5, 10);
        assert!(typed.mutate(&4, &mut g).is_some());

        let set = gens::vec(gens::int(0u8..=3)).collect::<std::collections::BTreeSet<u8>>();
        let value = set.generate(&mut g);
        assert!(value.len() <= 4);
        let s: std::collections::BTreeSet<u8> = [1, 2, 3].into_iter().collect();
        let candidates = set.shrink(&s);
        assert!(candidates.iter().all(|c| c.len() <= 3));
        assert!(candidates.iter().any(|c| c.len() == 2));
    }
}

#[cfg(test)]
mod dependent_tests {
    use crate::{
        Config,
        Gen,
        Source,
        check,
        check_generator,
        check_mutator,
        check_shrinker,
        gens,
    };

    fn cfg() -> Config {
        Config::default().tests(300).seed(7)
    }

    /// `(n, k)` with `k < n`.
    fn bounded() -> impl Gen<Value = (i32, i32)> {
        gens::dependent(gens::int(1..=20), |&n| gens::int(0..n))
    }

    #[test]
    fn dependent_pairs_stay_in_the_joint_support() {
        let support = |(n, k): &(i32, i32)| (1..=20).contains(n) && (0..*n).contains(k);
        check_generator(&cfg(), bounded(), support).assert_passed();
        check_shrinker(&cfg(), bounded(), support, |(n, k)| (*n as u64) * 32 + *k as u64)
            .assert_passed();
        check_mutator(&cfg(), bounded(), support).assert_passed();
    }

    #[test]
    fn shrinking_regenerates_the_dependent_half() {
        let g = bounded();
        let candidates = g.shrink(&(7, 6));
        assert!(!candidates.is_empty());
        assert!(candidates.iter().all(|(n, k)| *n <= 7 && *k < *n), "{candidates:?}");
        assert!(candidates.iter().any(|(n, _)| *n < 7), "the outer value shrinks");
        assert!(candidates.iter().any(|(n, k)| *n == 7 && *k < 6), "the inner value shrinks");
        assert_eq!(candidates, g.shrink(&(7, 6)), "candidates are a pure function of the value");
    }

    #[test]
    fn a_failing_dependent_input_shrinks_to_the_minimum() {
        let indexed =
            gens::dependent(gens::vec(gens::int(0..=100)).len(1..=8), |v| gens::int(0..v.len()));
        let report = check(&cfg(), indexed, |(v, i)| v[i] < 50);
        let failure = report.failure.as_ref().expect("fails");
        let shrunk =
            failure.shrunk.as_ref().map(|c| c.debug.as_str()).unwrap_or(&failure.original.debug);
        assert_eq!(shrunk, "([50], 0)", "{report}");
    }

    #[test]
    fn mutation_keeps_the_pair_consistent() {
        let g = bounded();
        let mut source = Source::new(3, 10);
        let mut moved_outer = false;
        let mut moved_inner = false;
        for _ in 0..200 {
            let (n, k) = g.generate(&mut source);
            if let Some((n2, k2)) = g.mutate(&(n, k), &mut source) {
                assert!((1..=20).contains(&n2) && k2 < n2, "({n2}, {k2}) from ({n}, {k})");
                moved_outer |= n2 != n;
                moved_inner |= n2 == n && k2 != k;
            }
        }
        assert!(moved_outer && moved_inner);
    }
}
