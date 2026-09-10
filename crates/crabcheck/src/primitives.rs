//! Constructors for the built-in generators. Re-exported as `crabcheck::gens`.
//!
//! Every generator here carries a shrinker and a mutator that stay inside
//! its own range, so the values they propose are values the generator could
//! have produced.

use std::{
    collections::{
        BTreeMap,
        BTreeSet,
        HashMap,
        HashSet,
    },
    fmt,
    hash::Hash,
    ops::RangeBounds,
    sync::Arc,
};

use crate::{
    Source,
    gens::{
        BoxGen,
        Gen,
        GenExt,
    },
    hooks::{
        mutate_f64,
        mutate_int,
        mutate_seq,
        shrink_char,
        shrink_f64,
        shrink_int,
        shrink_seq,
    },
    source::{
        Int,
        resolve_range,
    },
};

// --------------------------------------------------------------------------
// Scalars
// --------------------------------------------------------------------------

/// Always the same value. Nothing to shrink or mutate.
pub fn constant<T: Clone + Send + Sync>(value: T) -> Constant<T> {
    Constant(value)
}

/// See [`constant`].
#[derive(Clone, Debug)]
pub struct Constant<T>(T);

impl<T: Clone + Send + Sync> Gen for Constant<T> {
    type Value = T;

    fn generate(&self, _g: &mut Source) -> T {
        self.0.clone()
    }
}

/// One of `values`, each equally likely. Shrinks toward earlier values, so
/// list the simplest first.
///
/// # Panics
/// If `values` is empty.
pub fn pick_from<T: Clone + PartialEq + Send + Sync>(values: Vec<T>) -> PickFrom<T> {
    assert!(!values.is_empty(), "crabcheck: pick_from needs at least one value");
    PickFrom(values)
}

/// See [`pick_from`].
#[derive(Clone, Debug)]
pub struct PickFrom<T>(Vec<T>);

impl<T: Clone + PartialEq + Send + Sync> Gen for PickFrom<T> {
    type Value = T;

    fn generate(&self, g: &mut Source) -> T {
        g.pick(&self.0).clone()
    }

    fn shrink(&self, value: &T) -> Vec<T> {
        match self.0.iter().position(|v| v == value) {
            Some(index) => self.0[..index].to_vec(),
            None => Vec::new(),
        }
    }

    fn mutate(&self, value: &T, g: &mut Source) -> Option<T> {
        let other = g.pick(&self.0);
        (other != value).then(|| other.clone())
    }
}

/// An integer uniform in `range`. Shrinks toward the value closest to zero,
/// mutates by small steps or a fresh draw, both within the range.
pub fn int<T: Int>(range: impl RangeBounds<T>) -> IntGen<T> {
    let (lo, hi) = resolve_range::<T>(&range);
    IntGen { lo, hi, _t: std::marker::PhantomData }
}

/// See [`int`].
#[derive(Clone, Copy, Debug)]
pub struct IntGen<T> {
    lo: i128,
    hi: i128,
    _t: std::marker::PhantomData<fn() -> T>,
}

impl<T: Int> Gen for IntGen<T> {
    type Value = T;

    fn generate(&self, g: &mut Source) -> T {
        g.int(T::from_i128(self.lo)..=T::from_i128(self.hi))
    }

    fn shrink(&self, value: &T) -> Vec<T> {
        shrink_int(*value, self.lo, self.hi)
    }

    fn mutate(&self, value: &T, g: &mut Source) -> Option<T> {
        Some(mutate_int(*value, self.lo, self.hi, g))
    }
}

/// The edge cases of an integer range: `lo`, `hi`, `lo + 1`, `hi - 1`, and
/// `0`, `1`, `-1` when they lie in the range, each equally likely. This is
/// the bias that `any::<T>()` mixes into a uniform draw; use it directly to
/// build your own mix with [`frequency`]. Shrinks to entries of smaller
/// magnitude.
pub fn edges<T: Int>(range: impl RangeBounds<T>) -> Edges<T> {
    let (lo, hi) = resolve_range::<T>(&range);
    let mut values: Vec<i128> =
        [lo, hi, lo + 1, hi - 1, 0, 1, -1].into_iter().filter(|v| (lo..=hi).contains(v)).collect();
    values.sort_unstable_by_key(|v| (v.abs(), *v));
    values.dedup();
    Edges { values: values.into_iter().map(T::from_i128).collect() }
}

/// See [`edges`].
#[derive(Clone, Debug)]
pub struct Edges<T> {
    values: Vec<T>,
}

impl<T: Int> Gen for Edges<T> {
    type Value = T;

    fn generate(&self, g: &mut Source) -> T {
        *g.pick(&self.values)
    }

    fn shrink(&self, value: &T) -> Vec<T> {
        // Entries of strictly smaller magnitude, so the candidates are
        // simpler under the same measure integers use.
        let magnitude = value.to_i128().abs();
        self.values.iter().copied().filter(|v| v.to_i128().abs() < magnitude).collect()
    }

    fn mutate(&self, value: &T, g: &mut Source) -> Option<T> {
        let other = *g.pick(&self.values);
        (other != *value).then_some(other)
    }
}

/// A fair boolean; shrinks toward `false`.
pub fn bool() -> BoolGen {
    BoolGen { p: 0.5 }
}

/// `true` with probability `p`; shrinks toward `false`.
pub fn bool_with(p: f64) -> BoolGen {
    BoolGen { p }
}

/// See [`bool()`].
#[derive(Clone, Copy, Debug)]
pub struct BoolGen {
    p: f64,
}

impl Gen for BoolGen {
    type Value = bool;

    fn generate(&self, g: &mut Source) -> bool {
        if self.p == 0.5 { g.bool() } else { g.bool_with(self.p) }
    }

    fn shrink(&self, value: &bool) -> Vec<bool> {
        if *value { vec![false] } else { Vec::new() }
    }

    fn mutate(&self, value: &bool, _g: &mut Source) -> Option<bool> {
        Some(!*value)
    }
}

/// An `f64` uniform in `[0, 1)`. See [`Source::f64`].
pub fn f64() -> F64Gen {
    F64Gen
}

/// See [`f64()`].
#[derive(Clone, Copy, Debug)]
pub struct F64Gen;

impl Gen for F64Gen {
    type Value = f64;

    fn generate(&self, g: &mut Source) -> f64 {
        g.f64()
    }

    fn shrink(&self, value: &f64) -> Vec<f64> {
        shrink_f64(*value).into_iter().filter(|c| (0.0..1.0).contains(c)).collect()
    }

    fn mutate(&self, value: &f64, g: &mut Source) -> Option<f64> {
        Some(
            if g.bool() {
                g.f64()
            } else {
                (value + (g.f64() - 0.5) * 0.1).clamp(0.0, 0.999_999_999)
            },
        )
    }
}

/// An `f32` uniform in `[0, 1)`. See [`Source::f32`].
pub fn f32() -> F32Gen {
    F32Gen
}

/// See [`f32()`].
#[derive(Clone, Copy, Debug)]
pub struct F32Gen;

impl Gen for F32Gen {
    type Value = f32;

    fn generate(&self, g: &mut Source) -> f32 {
        g.f32()
    }

    fn shrink(&self, value: &f32) -> Vec<f32> {
        F64Gen.shrink(&f64::from(*value)).into_iter().map(|v| v as f32).collect()
    }

    fn mutate(&self, value: &f32, g: &mut Source) -> Option<f32> {
        F64Gen.mutate(&f64::from(*value), g).map(|v| v as f32)
    }
}

/// An `f64` with a uniformly random bit pattern (any value including NaN,
/// infinities and subnormals). See [`Source::f64_bits`]. Shrinks toward
/// `0.0`, then positive, integral, halved.
pub fn f64_bits() -> F64Bits {
    F64Bits
}

/// See [`f64_bits`].
#[derive(Clone, Copy, Debug)]
pub struct F64Bits;

impl Gen for F64Bits {
    type Value = f64;

    fn generate(&self, g: &mut Source) -> f64 {
        g.f64_bits()
    }

    fn shrink(&self, value: &f64) -> Vec<f64> {
        shrink_f64(*value)
    }

    fn mutate(&self, value: &f64, g: &mut Source) -> Option<f64> {
        Some(mutate_f64(*value, g))
    }
}

/// An `f32` with a uniformly random bit pattern. See [`Source::f32_bits`].
pub fn f32_bits() -> F32Bits {
    F32Bits
}

/// See [`f32_bits`].
#[derive(Clone, Copy, Debug)]
pub struct F32Bits;

impl Gen for F32Bits {
    type Value = f32;

    fn generate(&self, g: &mut Source) -> f32 {
        g.f32_bits()
    }

    fn shrink(&self, value: &f32) -> Vec<f32> {
        shrink_f64(f64::from(*value)).into_iter().map(|v| v as f32).collect()
    }

    fn mutate(&self, value: &f32, g: &mut Source) -> Option<f32> {
        Some(mutate_f64(f64::from(*value), g) as f32)
    }
}

/// A finite `f64` drawn from the whole finite range by bit pattern.
pub fn f64_finite() -> impl Gen<Value = f64> {
    f64_bits().filter("finite", |x| x.is_finite())
}

/// A finite `f32` drawn from the whole finite range by bit pattern.
pub fn f32_finite() -> impl Gen<Value = f32> {
    f32_bits().filter("finite", |x| x.is_finite())
}

/// An `f64` uniform in `lo..=hi` (finite bounds). Shrinks toward the end
/// closest to zero.
///
/// # Panics
/// If a bound is not finite or `lo > hi`.
pub fn f64_range(range: std::ops::RangeInclusive<f64>) -> F64Range {
    let (lo, hi) = (*range.start(), *range.end());
    assert!(
        lo.is_finite() && hi.is_finite() && lo <= hi,
        "crabcheck: f64_range needs finite lo <= hi"
    );
    F64Range { lo, hi }
}

/// See [`f64_range`].
#[derive(Clone, Copy, Debug)]
pub struct F64Range {
    lo: f64,
    hi: f64,
}

impl Gen for F64Range {
    type Value = f64;

    fn generate(&self, g: &mut Source) -> f64 {
        let u = g.f64();
        (self.lo + (self.hi - self.lo) * u).clamp(self.lo, self.hi)
    }

    fn shrink(&self, value: &f64) -> Vec<f64> {
        let target = 0.0f64.clamp(self.lo, self.hi);
        let mut out: Vec<f64> = if *value == target { Vec::new() } else { vec![target] };
        out.extend(
            shrink_f64(*value)
                .into_iter()
                .map(|c| c.clamp(self.lo, self.hi))
                .filter(|c| c.to_bits() != value.to_bits() && c.to_bits() != target.to_bits()),
        );
        out
    }

    fn mutate(&self, value: &f64, g: &mut Source) -> Option<f64> {
        let width = self.hi - self.lo;
        Some(
            if g.bool() {
                self.generate(g)
            } else {
                (value + (g.f64() - 0.5) * width * 0.1).clamp(self.lo, self.hi)
            },
        )
    }
}

/// Any Unicode scalar value, weighted toward ASCII letters and digits.
/// Shrinks toward `'a'`.
pub fn char() -> CharGen {
    CharGen::Any
}

/// A printable ASCII character (`' '..='~'`). Shrinks toward `'a'`.
pub fn ascii_char() -> CharGen {
    CharGen::AsciiPrintable
}

/// An ASCII letter or digit. Shrinks toward `'a'`.
pub fn alphanumeric() -> CharGen {
    CharGen::Alphanumeric
}

/// See [`char()`], [`ascii_char`] and [`alphanumeric`].
#[derive(Clone, Copy, Debug)]
pub enum CharGen {
    /// Any Unicode scalar value.
    Any,
    /// `' '..='~'`.
    AsciiPrintable,
    /// `a-z`, `A-Z`, `0-9`.
    Alphanumeric,
}

/// Character classes, simplest first. Each is `(weight, lo, hi)` of code
/// points.
const CHAR_CLASSES: [(u32, u32, u32); 7] = [
    (40, 'a' as u32, 'z' as u32),
    (10, 'A' as u32, 'Z' as u32),
    (10, '0' as u32, '9' as u32),
    (20, 0x20, 0x7e),
    (5, 0x00, 0x7f),
    (10, 0x80, 0xD7FF),
    (5, 0xE000, 0x10FFFF),
];

impl CharGen {
    fn classes(self) -> &'static [(u32, u32, u32)] {
        match self {
            CharGen::Any => &CHAR_CLASSES,
            CharGen::AsciiPrintable => &CHAR_CLASSES[..4],
            CharGen::Alphanumeric => &CHAR_CLASSES[..3],
        }
    }

    fn admits(self, c: char) -> bool {
        let code = c as u32;
        self.classes().iter().any(|(_, lo, hi)| (*lo..=*hi).contains(&code))
    }
}

impl Gen for CharGen {
    type Value = char;

    fn generate(&self, g: &mut Source) -> char {
        let classes = self.classes();
        let weights: Vec<(u32, usize)> =
            classes.iter().enumerate().map(|(i, c)| (c.0, i)).collect();
        let class = *g.weighted(&weights);
        let (_, lo, hi) = classes[class];
        char::from_u32(g.int(lo..=hi)).expect("char classes exclude surrogates")
    }

    fn shrink(&self, value: &char) -> Vec<char> {
        shrink_char(*value).into_iter().filter(|c| self.admits(*c)).collect()
    }

    fn mutate(&self, value: &char, g: &mut Source) -> Option<char> {
        let other = self.generate(g);
        (other != *value).then_some(other)
    }
}

// --------------------------------------------------------------------------
// Collections
// --------------------------------------------------------------------------

/// A `Vec` of values from `element`, with length in `0..=size` by default
/// (see [`VecGen::len`]). Lengths are uniform. Shrinks by dropping blocks
/// of elements and shrinking elements in place; mutates by perturbing,
/// inserting, removing, duplicating or swapping elements.
pub fn vec<G: Gen>(element: G) -> VecGen<G> {
    VecGen { element, min: 0, max: None }
}

/// See [`vec()`].
#[derive(Clone, Debug)]
pub struct VecGen<G> {
    element: G,
    min: usize,
    max: Option<usize>,
}

impl<G> VecGen<G> {
    /// Restrict the length to `range`. An unbounded upper end means
    /// `max(min, size)`.
    pub fn len(mut self, range: impl RangeBounds<usize>) -> Self {
        use std::ops::Bound;
        self.min = match range.start_bound() {
            Bound::Included(&n) => n,
            Bound::Excluded(&n) => n + 1,
            Bound::Unbounded => 0,
        };
        self.max = match range.end_bound() {
            Bound::Included(&n) => Some(n),
            Bound::Excluded(&n) => Some(n.saturating_sub(1)),
            Bound::Unbounded => None,
        };
        self
    }

    fn bounds(&self, size: usize) -> (usize, usize) {
        let max = self.max.unwrap_or(size).max(self.min);
        (self.min, max)
    }
}

impl<G: Gen> Gen for VecGen<G>
where
    G::Value: Clone,
{
    type Value = Vec<G::Value>;

    fn generate(&self, g: &mut Source) -> Vec<G::Value> {
        let (min, max) = self.bounds(g.size());
        let len = g.int(min..=max);
        (0..len).map(|_| self.element.generate(g)).collect()
    }

    fn shrink(&self, value: &Vec<G::Value>) -> Vec<Vec<G::Value>> {
        shrink_seq(value, self.min, |x| self.element.shrink(x))
    }

    fn mutate(&self, value: &Vec<G::Value>, g: &mut Source) -> Option<Vec<G::Value>> {
        let max = self.max.unwrap_or(usize::MAX).max(self.min);
        let mut fresh = |g: &mut Source| self.element.generate(g);
        mutate_seq(value, self.min, max, g, Some(&mut fresh), |x, g| self.element.mutate(x, g))
    }
}

/// A `String` of [`char()`]s with length in `0..=size`.
pub fn string() -> impl Gen<Value = String> {
    string_of(char())
}

/// A `String` of characters from `chars`, with length in `0..=size`.
pub fn string_of<G: Gen<Value = char>>(chars: G) -> impl Gen<Value = String> {
    vec(chars).iso(|cs| cs.into_iter().collect(), |s: &String| s.chars().collect())
}

/// A `String` of characters from `chars`, with length in `range`.
pub fn string_len<G: Gen<Value = char>>(
    chars: G,
    range: impl RangeBounds<usize>,
) -> impl Gen<Value = String> {
    vec(chars).len(range).iso(|cs| cs.into_iter().collect(), |s: &String| s.chars().collect())
}

/// `Some(value)` or `None`; shrinks toward `None`.
pub fn option<G: Gen>(inner: G) -> OptionGen<G> {
    OptionGen { inner, p_some: 0.75 }
}

/// See [`option`].
#[derive(Clone, Debug)]
pub struct OptionGen<G> {
    inner: G,
    p_some: f64,
}

impl<G> OptionGen<G> {
    /// Probability of generating `Some` (default 0.75).
    pub fn some_with(mut self, p: f64) -> Self {
        self.p_some = p;
        self
    }
}

impl<G: Gen> Gen for OptionGen<G> {
    type Value = Option<G::Value>;

    fn generate(&self, g: &mut Source) -> Option<G::Value> {
        if g.bool_with(self.p_some) { Some(self.inner.generate(g)) } else { None }
    }

    fn shrink(&self, value: &Option<G::Value>) -> Vec<Option<G::Value>> {
        match value {
            None => Vec::new(),
            Some(x) => {
                std::iter::once(None).chain(self.inner.shrink(x).into_iter().map(Some)).collect()
            },
        }
    }

    fn mutate(&self, value: &Option<G::Value>, g: &mut Source) -> Option<Option<G::Value>> {
        match value {
            None => Some(Some(self.inner.generate(g))),
            Some(x) => {
                if g.bool_with(0.125) {
                    Some(None)
                } else {
                    self.inner.mutate(x, g).map(Some).or_else(|| Some(Some(self.inner.generate(g))))
                }
            },
        }
    }
}

/// `Ok` or `Err`; shrinks within the variant.
pub fn result<GT: Gen, GE: Gen>(ok: GT, err: GE) -> ResultGen<GT, GE> {
    ResultGen { ok, err }
}

/// See [`result`].
#[derive(Clone, Debug)]
pub struct ResultGen<GT, GE> {
    ok: GT,
    err: GE,
}

impl<GT: Gen, GE: Gen> Gen for ResultGen<GT, GE> {
    type Value = Result<GT::Value, GE::Value>;

    fn generate(&self, g: &mut Source) -> Self::Value {
        if g.bool() { Err(self.err.generate(g)) } else { Ok(self.ok.generate(g)) }
    }

    fn shrink(&self, value: &Self::Value) -> Vec<Self::Value> {
        match value {
            Ok(x) => self.ok.shrink(x).into_iter().map(Ok).collect(),
            Err(e) => self.err.shrink(e).into_iter().map(Err).collect(),
        }
    }

    fn mutate(&self, value: &Self::Value, g: &mut Source) -> Option<Self::Value> {
        if g.bool_with(0.125) {
            return Some(match value {
                Ok(_) => Err(self.err.generate(g)),
                Err(_) => Ok(self.ok.generate(g)),
            });
        }
        match value {
            Ok(x) => self.ok.mutate(x, g).map(Ok),
            Err(e) => self.err.mutate(e, g).map(Err),
        }
    }
}

/// A `HashMap` built from generated pairs (duplicate keys collapse).
pub fn hash_map<GK, GV>(keys: GK, values: GV) -> impl Gen<Value = HashMap<GK::Value, GV::Value>>
where
    GK: Gen,
    GK::Value: Hash + Eq + Clone + Send + Sync,
    GV: Gen,
    GV::Value: Clone + Send + Sync,
{
    vec((keys, values)).collect()
}

/// A `BTreeMap` built from generated pairs (duplicate keys collapse).
pub fn btree_map<GK, GV>(keys: GK, values: GV) -> impl Gen<Value = BTreeMap<GK::Value, GV::Value>>
where
    GK: Gen,
    GK::Value: Ord + Clone + Send + Sync,
    GV: Gen,
    GV::Value: Clone + Send + Sync,
{
    vec((keys, values)).collect()
}

/// A `HashSet` built from generated elements (duplicates collapse).
pub fn hash_set<G>(elements: G) -> impl Gen<Value = HashSet<G::Value>>
where
    G: Gen,
    G::Value: Hash + Eq + Clone + Send + Sync,
{
    vec(elements).collect()
}

/// A `BTreeSet` built from generated elements (duplicates collapse).
pub fn btree_set<G>(elements: G) -> impl Gen<Value = BTreeSet<G::Value>>
where
    G: Gen,
    G::Value: Ord + Clone + Send + Sync,
{
    vec(elements).collect()
}

// --------------------------------------------------------------------------
// Choice between generators
// --------------------------------------------------------------------------

/// Pick one of `alternatives` uniformly, then generate from it. Shrink
/// candidates come from every alternative's shrinker (a value cannot tell
/// which alternative produced it), so keep alternatives' supports
/// compatible or add a `filter`.
///
/// # Panics
/// If `alternatives` is empty.
pub fn one_of<T>(alternatives: Vec<BoxGen<T>>) -> Frequency<T> {
    assert!(!alternatives.is_empty(), "crabcheck: one_of needs at least one alternative");
    Frequency { alternatives: alternatives.into_iter().map(|g| (1, g)).collect() }
}

/// Pick one of `alternatives` with probability proportional to its weight,
/// then generate from it. See [`one_of`] for how it shrinks.
///
/// # Panics
/// If all weights are zero.
pub fn frequency<T>(alternatives: Vec<(u32, BoxGen<T>)>) -> Frequency<T> {
    assert!(
        alternatives.iter().any(|(w, _)| *w > 0),
        "crabcheck: frequency needs a non-zero weight"
    );
    Frequency { alternatives }
}

/// See [`one_of`] and [`frequency`].
pub struct Frequency<T> {
    alternatives: Vec<(u32, BoxGen<T>)>,
}

impl<T> Gen for Frequency<T> {
    type Value = T;

    fn generate(&self, g: &mut Source) -> T {
        let chosen = g.weighted(&self.alternatives).clone();
        chosen.generate(g)
    }

    fn shrink(&self, value: &T) -> Vec<T> {
        self.alternatives
            .iter()
            .filter(|(w, _)| *w > 0)
            .flat_map(|(_, alt)| alt.shrink(value))
            .collect()
    }

    fn mutate(&self, value: &T, g: &mut Source) -> Option<T> {
        let alt = g.weighted(&self.alternatives).clone();
        alt.mutate(value, g).or_else(|| Some(alt.generate(g)))
    }
}

impl<T> Clone for Frequency<T> {
    fn clone(&self) -> Self {
        Frequency { alternatives: self.alternatives.clone() }
    }
}

impl<T> fmt::Debug for Frequency<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frequency").field("alternatives", &self.alternatives.len()).finish()
    }
}

/// A recursive generator with bounded depth. `branch` receives a generator
/// for the next level down and builds the current level from it; at depth
/// `max_depth` the generator is `leaf`. Shrinking and mutation delegate to
/// the generator `branch` builds at the top level.
///
/// ```
/// use crabcheck::{gens, BoxGen, GenExt, Gen, Source};
///
/// #[derive(Debug, Clone)]
/// enum Expr { Lit(i8), Add(Box<Expr>, Box<Expr>) }
///
/// let expr = gens::recursive(4, gens::int(-9i8..=9).map(Expr::Lit).boxed(), |inner| {
///     gens::frequency(vec![
///         (1, gens::int(-9i8..=9).map(Expr::Lit).boxed()),
///         (2, (inner.clone(), inner).map(|(a, b)| Expr::Add(Box::new(a), Box::new(b))).boxed()),
///     ])
///     .boxed()
/// });
/// let _ = expr.generate(&mut Source::new(1, 10));
/// ```
pub fn recursive<T: 'static>(
    max_depth: usize,
    leaf: BoxGen<T>,
    branch: impl Fn(BoxGen<T>) -> BoxGen<T> + Send + Sync + 'static,
) -> Recursive<T> {
    Recursive { depth: max_depth, leaf, branch: Arc::new(branch) }
}

/// See [`recursive`].
pub struct Recursive<T> {
    depth: usize,
    leaf: BoxGen<T>,
    branch: Arc<dyn Fn(BoxGen<T>) -> BoxGen<T> + Send + Sync>,
}

impl<T> Clone for Recursive<T> {
    fn clone(&self) -> Self {
        Recursive { depth: self.depth, leaf: self.leaf.clone(), branch: Arc::clone(&self.branch) }
    }
}

impl<T: 'static> Recursive<T> {
    fn top(&self) -> BoxGen<T> {
        if self.depth == 0 {
            return self.leaf.clone();
        }
        let next = Recursive {
            depth: self.depth - 1,
            leaf: self.leaf.clone(),
            branch: Arc::clone(&self.branch),
        };
        (self.branch)(BoxGen::new(next))
    }
}

impl<T: 'static> Gen for Recursive<T> {
    type Value = T;

    fn generate(&self, g: &mut Source) -> T {
        self.top().generate(g)
    }

    fn shrink(&self, value: &T) -> Vec<T> {
        self.top().shrink(value)
    }

    fn mutate(&self, value: &T, g: &mut Source) -> Option<T> {
        self.top().mutate(value, g)
    }
}

/// Build a generator from the current [`Source::size`], for example to scale
/// a numeric range with the size parameter. Generation only: the built
/// generator's hooks cannot be reached without a size.
pub fn sized<G: Gen>(f: impl Fn(usize) -> G + Send + Sync) -> impl Gen<Value = G::Value> {
    move |g: &mut Source| {
        let generator = f(g.size());
        generator.generate(g)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vec_lengths_follow_size_and_len() {
        for size in [0usize, 1, 5, 20] {
            let mut g = Source::new(size as u64, size);
            for _ in 0..50 {
                let v = vec(int(0..=9)).generate(&mut g);
                assert!(v.len() <= size, "size {size} produced len {}", v.len());
            }
        }
        let mut g = Source::new(1, 3);
        for _ in 0..50 {
            let v = vec(int(0..=9)).len(2..=4).generate(&mut g);
            assert!((2..=4).contains(&v.len()));
            let v = vec(int(0..=9)).len(5..).generate(&mut g);
            assert_eq!(v.len(), 5, "unbounded max with size < min collapses to min");
        }
    }

    #[test]
    fn vec_hooks_respect_bounds() {
        let generator = vec(int(0..=9)).len(2..=4);
        let candidates = generator.shrink(&vec![3, 4, 5]);
        assert!(candidates.iter().all(|c| c.len() >= 2));
        assert!(candidates.contains(&vec![4, 5]) && candidates.contains(&vec![0, 4, 5]));
        let mut g = Source::new(1, 10);
        for _ in 0..200 {
            if let Some(m) = generator.mutate(&vec![3, 4, 5], &mut g) {
                assert!((2..=4).contains(&m.len()), "{m:?}");
                assert!(m.iter().all(|x| (0..=9).contains(x)));
            }
        }
    }

    #[test]
    fn scalar_hooks_stay_in_range() {
        assert_eq!(int(10..=30).shrink(&20), vec![10, 15, 18, 19]);
        assert_eq!(edges(-5i32..=5).shrink(&5), vec![0, -1, 1, -4, 4]);
        assert_eq!(edges(-5i32..=5).shrink(&-1), vec![0]);
        assert_eq!(pick_from(vec!["a", "b", "c"]).shrink(&"c"), vec!["a", "b"]);
        assert_eq!(bool().shrink(&true), vec![false]);
        assert_eq!(f64_range(2.0..=8.0).shrink(&6.0), vec![2.0, 3.0]);
        assert_eq!(ascii_char().shrink(&'z'), vec!['a', 'n', 't', 'w', 'y']);
        assert!(alphanumeric().shrink(&'0').iter().all(|c| c.is_ascii_alphanumeric()));
        let mut g = Source::new(2, 5);
        for _ in 0..200 {
            assert!((10..=30).contains(&int(10..=30).mutate(&20, &mut g).unwrap()));
            let c = alphanumeric().mutate(&'q', &mut g);
            assert!(c.is_none_or(|c| c.is_ascii_alphanumeric()));
            let x = f64_range(2.0..=8.0).mutate(&6.0, &mut g).unwrap();
            assert!((2.0..=8.0).contains(&x));
            let u = f64().mutate(&0.5, &mut g).unwrap();
            assert!((0.0..1.0).contains(&u));
        }
    }

    #[test]
    fn option_result_frequency_and_strings() {
        let opt = option(int(0..=9));
        assert_eq!(opt.shrink(&Some(2)), vec![None, Some(0), Some(1)]);
        let res = result(int(0..=9), bool());
        assert_eq!(res.shrink(&Ok(2)), vec![Ok(0), Ok(1)]);
        assert_eq!(res.shrink(&Err(true)), vec![Err(false)]);
        let alts = frequency(vec![(0, constant(0).boxed()), (1, int(0..=9).boxed())]);
        assert_eq!(alts.shrink(&4), vec![0, 2, 3]);
        let s = string();
        let candidates = s.shrink(&"bb".to_string());
        assert_eq!(candidates[0], "");
        assert!(candidates.contains(&"b".to_string()));
        let mut g = Source::new(3, 8);
        let m = s.mutate(&"hello".to_string(), &mut g);
        assert!(m.is_some());
        let text = string_len(alphanumeric(), 3..=3).generate(&mut g);
        assert_eq!(text.chars().count(), 3);
    }

    #[test]
    fn char_classes_are_valid_and_biased() {
        let mut g = Source::new(4, 10);
        let mut ascii_letters = 0;
        for _ in 0..1000 {
            let c = char().generate(&mut g);
            if c.is_ascii_lowercase() {
                ascii_letters += 1;
            }
            let a = ascii_char().generate(&mut g);
            assert!(a.is_ascii_graphic() || a == ' ', "{a:?}");
            assert!(alphanumeric().generate(&mut g).is_ascii_alphanumeric());
        }
        assert!(ascii_letters > 250, "lowercase letters drawn {ascii_letters} times");
    }

    #[test]
    fn edges_cover_the_range_ends_and_zero() {
        let mut seen = std::collections::BTreeSet::new();
        let mut g = Source::new(3, 10);
        for _ in 0..200 {
            seen.insert(edges(-5i32..=5).generate(&mut g));
        }
        assert_eq!(seen.into_iter().collect::<Vec<_>>(), vec![-5, -4, -1, 0, 1, 4, 5]);
        let full: Vec<u8> = (0..500).map(|_| edges::<u8>(..).generate(&mut g)).collect();
        assert!(
            full.contains(&0) && full.contains(&255) && full.contains(&1) && full.contains(&254)
        );
    }

    #[test]
    fn recursive_respects_depth_and_shrinks() {
        #[derive(Debug, Clone, PartialEq)]
        enum T {
            Leaf(u8),
            Node(Box<T>, Box<T>),
        }
        fn depth(t: &T) -> usize {
            match t {
                T::Leaf(_) => 0,
                T::Node(a, b) => 1 + depth(a).max(depth(b)),
            }
        }
        let tree = recursive(
            3,
            int(0u8..=9).iso(T::Leaf, |t| if let T::Leaf(v) = t { *v } else { 0 }).boxed(),
            |inner| {
                frequency(vec![
                    (
                        1,
                        int(0u8..=9)
                            .iso(T::Leaf, |t| if let T::Leaf(v) = t { *v } else { 0 })
                            .boxed(),
                    ),
                    (
                        3,
                        (inner.clone(), inner)
                            .iso(
                                |(a, b)| T::Node(Box::new(a), Box::new(b)),
                                |t| {
                                    if let T::Node(a, b) = t {
                                        ((**a).clone(), (**b).clone())
                                    } else {
                                        (T::Leaf(0), T::Leaf(0))
                                    }
                                },
                            )
                            .boxed(),
                    ),
                ])
                .boxed()
            },
        );
        let mut g = Source::new(2, 10);
        let mut max_seen = 0;
        for _ in 0..100 {
            let t = tree.generate(&mut g);
            max_seen = max_seen.max(depth(&t));
            assert!(depth(&t) <= 3);
        }
        assert_eq!(max_seen, 3);
        let candidates = tree.shrink(&T::Node(Box::new(T::Leaf(4)), Box::new(T::Leaf(0))));
        assert!(
            candidates.contains(&T::Node(Box::new(T::Leaf(0)), Box::new(T::Leaf(0)))),
            "{candidates:?}"
        );
    }

    #[test]
    fn maps_sets_and_sized() {
        let mut g = Source::new(6, 10);
        let m = hash_map(int(0u8..=3), bool()).generate(&mut g);
        assert!(m.len() <= 4);
        let s = btree_set(int(0u8..=3)).generate(&mut g);
        assert!(s.len() <= 4);
        let scaled = sized(|size| int(0..=size as i64));
        for _ in 0..100 {
            assert!(scaled.generate(&mut g) <= 10);
        }
        let _ = btree_map(int(0u8..=3), bool()).generate(&mut g);
        let _ = hash_set(int(0u8..=3)).generate(&mut g);
        assert!(f64_finite().generate(&mut g).is_finite());
        assert!(f32_finite().generate(&mut g).is_finite());
        for _ in 0..100 {
            let x = f64_range(-2.5..=7.0).generate(&mut g);
            assert!((-2.5..=7.0).contains(&x));
        }
    }
}
