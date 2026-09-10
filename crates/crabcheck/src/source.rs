//! [`Source`]: the seeded random source a generator draws from.
//!
//! A `Source` is a deterministic pseudo-random generator (ChaCha8) seeded by
//! the runner, plus the size parameter for the current input. Every draw is
//! uniform; edge-case bias is composed from visible generators in
//! [`gens`](crate::gens) rather than hidden in the primitives.
//!
//! **Determinism contract.** A generator must be a pure function of the
//! decisions it draws from its `Source` (and of the size). That is what
//! makes a failure reproducible from its seed and iteration alone.
//!
//! `Source` implements [`RngCore`], so any distribution built on the `rand`
//! traits can draw from it as well.

use std::{
    borrow::Cow,
    fmt,
    ops::{
        Bound,
        RangeBounds,
    },
};

use {
    rand_chacha::ChaCha8Rng,
    rand_core::{
        RngCore,
        SeedableRng,
    },
};

use crate::gens::Gen;

/// Default number of attempts a `filter` makes before discarding.
pub const DEFAULT_FILTER_MAX_ATTEMPTS: usize = 100;

/// The seeded random source a generator draws from. See the
/// [module docs](self).
pub struct Source {
    rng: ChaCha8Rng,
    size: usize,
    filter_max_attempts: usize,
    discarded: Option<Cow<'static, str>>,
    draws: u64,
}

impl fmt::Debug for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Source")
            .field("size", &self.size)
            .field("draws", &self.draws)
            .field("discarded", &self.discarded)
            .finish()
    }
}

impl Source {
    /// A source seeded with `seed`, reporting `size` to generators.
    pub fn new(seed: u64, size: usize) -> Self {
        Source {
            rng: ChaCha8Rng::seed_from_u64(seed),
            size,
            filter_max_attempts: DEFAULT_FILTER_MAX_ATTEMPTS,
            discarded: None,
            draws: 0,
        }
    }

    /// Override the number of attempts a `filter` makes before discarding.
    pub fn with_filter_max_attempts(mut self, attempts: usize) -> Self {
        self.filter_max_attempts = attempts.max(1);
        self
    }

    /// Number of attempts a `filter` makes before discarding.
    pub fn filter_max_attempts(&self) -> usize {
        self.filter_max_attempts
    }

    /// The size parameter for this input. Collection generators use it as
    /// their default maximum length; recursive generators can use it to
    /// bound depth. See [`Config::max_size`](crate::Config::max_size) for
    /// how the runner chooses it.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Number of random draws made so far.
    pub fn draws(&self) -> u64 {
        self.draws
    }

    /// A new, independent source seeded from this one, with the same size.
    pub fn derive(&mut self) -> Source {
        Source::new(self.next_u64(), self.size).with_filter_max_attempts(self.filter_max_attempts)
    }

    /// Run a sub-generator.
    pub fn draw<G: Gen + ?Sized>(&mut self, generator: &G) -> G::Value {
        generator.generate(self)
    }

    /// Mark this input as discarded. The runner will not run the property
    /// on it; `reason` feeds the discard histogram. Only the first reason is
    /// kept.
    pub fn discard(&mut self, reason: impl Into<Cow<'static, str>>) {
        if self.discarded.is_none() {
            self.discarded = Some(reason.into());
        }
    }

    /// The discard reason, if [`discard`](Self::discard) was called.
    pub fn discarded(&self) -> Option<&str> {
        self.discarded.as_deref()
    }

    /// An integer drawn uniformly from `range` (any bound style; unbounded
    /// ends use the type's `MIN`/`MAX`). For edge-case bias compose with
    /// [`gens::edges`](crate::gens::edges).
    ///
    /// # Panics
    /// If the range is empty.
    pub fn int<T: Int>(&mut self, range: impl RangeBounds<T>) -> T {
        let (lo, hi) = resolve_range::<T>(&range);
        let span = (hi - lo) as u64;
        self.draws += 1;
        T::from_i128(lo + i128::from(uniform(&mut self.rng, span)))
    }

    /// An index in `0..len`, uniform.
    ///
    /// # Panics
    /// If `len` is 0.
    pub fn index(&mut self, len: usize) -> usize {
        assert!(len > 0, "crabcheck: Source::index called with len 0");
        self.int(0..len)
    }

    /// A fair boolean.
    pub fn bool(&mut self) -> bool {
        self.draws += 1;
        self.rng.next_u32() & 1 == 1
    }

    /// `true` with probability `p`.
    pub fn bool_with(&mut self, p: f64) -> bool {
        self.draws += 1;
        u01(&mut self.rng) < p
    }

    /// One of `items`, chosen uniformly.
    ///
    /// # Panics
    /// If `items` is empty.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        assert!(!items.is_empty(), "crabcheck: Source::pick called with no items");
        &items[self.index(items.len())]
    }

    /// One of `items`, chosen with probability proportional to its weight.
    ///
    /// # Panics
    /// If `items` is empty or all weights are zero.
    pub fn weighted<'a, T>(&mut self, items: &'a [(u32, T)]) -> &'a T {
        let total: u64 = items.iter().map(|(w, _)| u64::from(*w)).sum();
        assert!(total > 0, "crabcheck: Source::weighted needs at least one non-zero weight");
        let mut roll = self.int(0..total);
        for (weight, item) in items {
            let weight = u64::from(*weight);
            if roll < weight {
                return item;
            }
            roll -= weight;
        }
        unreachable!("roll exceeded total weight")
    }

    /// An `f64` uniform in `[0, 1)` with 53 bits of precision. For the whole
    /// `f64` domain use [`f64_bits`](Self::f64_bits) or the visible mixes in
    /// [`gens`](crate::gens).
    pub fn f64(&mut self) -> f64 {
        self.draws += 1;
        u01(&mut self.rng)
    }

    /// An `f64` with a uniformly random bit pattern: any finite value, either
    /// zero, subnormals, infinities and NaN, each with the probability their
    /// encodings occupy.
    pub fn f64_bits(&mut self) -> f64 {
        self.draws += 1;
        f64::from_bits(self.rng.next_u64())
    }

    /// An `f32` uniform in `[0, 1)`.
    pub fn f32(&mut self) -> f32 {
        self.draws += 1;
        u01(&mut self.rng) as f32
    }

    /// An `f32` with a uniformly random bit pattern.
    pub fn f32_bits(&mut self) -> f32 {
        self.draws += 1;
        f32::from_bits(self.rng.next_u32())
    }
}

impl RngCore for Source {
    fn next_u32(&mut self) -> u32 {
        self.draws += 1;
        self.rng.next_u32()
    }

    fn next_u64(&mut self) -> u64 {
        self.draws += 1;
        self.rng.next_u64()
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        self.draws += 1;
        self.rng.fill_bytes(dest)
    }
}

// --------------------------------------------------------------------------
// Integer support
// --------------------------------------------------------------------------

/// Integer types that [`Source::int`] can draw. Implemented for all primitive
/// integers up to 64 bits (plus `isize`/`usize`).
pub trait Int: Copy + Ord + fmt::Debug + Send + Sync + 'static {
    /// Smallest value of the type.
    const MIN: Self;
    /// Largest value of the type.
    const MAX: Self;
    /// Widen to `i128`.
    fn to_i128(self) -> i128;
    /// Narrow from `i128`; the value is guaranteed to be in range.
    fn from_i128(value: i128) -> Self;
}

macro_rules! impl_int {
    ($($t:ty),*) => {$(
        impl Int for $t {
            const MIN: Self = <$t>::MIN;
            const MAX: Self = <$t>::MAX;
            fn to_i128(self) -> i128 { self as i128 }
            fn from_i128(value: i128) -> Self { value as $t }
        }
    )*};
}
impl_int!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

/// Resolve `range` to inclusive `(lo, hi)` in `i128`, panicking if empty.
pub(crate) fn resolve_range<T: Int>(range: &impl RangeBounds<T>) -> (i128, i128) {
    let lo = match range.start_bound() {
        Bound::Included(x) => x.to_i128(),
        Bound::Excluded(x) => x.to_i128() + 1,
        Bound::Unbounded => T::MIN.to_i128(),
    };
    let hi = match range.end_bound() {
        Bound::Included(x) => x.to_i128(),
        Bound::Excluded(x) => x.to_i128() - 1,
        Bound::Unbounded => T::MAX.to_i128(),
    };
    assert!(lo <= hi, "crabcheck: empty integer range {lo}..={hi}");
    (lo, hi)
}

/// Uniform in `0..=span`.
pub(crate) fn uniform(rng: &mut impl RngCore, span: u64) -> u64 {
    if span == u64::MAX {
        return rng.next_u64();
    }
    let n = u128::from(span) + 1;
    ((u128::from(rng.next_u64()) * n) >> 64) as u64
}

/// Uniform in `[0, 1)` with 53 bits of precision.
pub(crate) fn u01(rng: &mut impl RngCore) -> f64 {
    (rng.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
}

/// A non-negative magnitude that is usually tiny: `P(m >= 2^k)` falls off
/// geometrically in `k`. Used by mutators for "small step" perturbations.
pub(crate) fn small_magnitude(rng: &mut impl RngCore) -> u64 {
    let bits = u64::from(rng.next_u64().trailing_zeros().min(20));
    uniform(rng, 1u64 << bits) + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_draws() {
        let mut a = Source::new(42, 10);
        let mut b = Source::new(42, 10);
        let draws =
            |g: &mut Source| (g.int(-5i32..=5), g.bool(), g.f64(), g.index(7), g.next_u64());
        assert_eq!(draws(&mut a), draws(&mut b));
        let mut c = Source::new(43, 10);
        assert_ne!(draws(&mut a), draws(&mut c));
        assert_eq!(a.draws(), 5 * 2);
    }

    #[test]
    fn int_draws_are_uniform_and_in_range() {
        let mut g = Source::new(123, 10);
        let n = 20_000;
        let mut hist = [0u32; 10];
        let mut sum = 0i64;
        for _ in 0..n {
            let v = g.int(0i32..10);
            hist[v as usize] += 1;
            sum += i64::from(g.int(-1000i32..=1000));
            let u: u64 = g.int(..);
            let _ = u;
            assert!((10..20).contains(&g.int(10u8..20)));
        }
        for count in hist {
            assert!((1700..=2300).contains(&count), "{hist:?}");
        }
        assert!((sum / n).abs() < 20, "mean {}", sum / n);
    }

    #[test]
    fn float_draws_are_unit_or_bit_patterns() {
        let mut g = Source::new(77, 10);
        let (mut nan, mut huge) = (0, 0);
        for _ in 0..20_000 {
            let u = g.f64();
            assert!((0.0..1.0).contains(&u), "{u}");
            let v = g.f64_bits();
            if v.is_nan() {
                nan += 1;
            } else if v.abs() > 1e100 {
                huge += 1;
            }
            let w = g.f32();
            assert!((0.0..1.0).contains(&w), "{w}");
            let _ = g.f32_bits();
        }
        assert!(nan > 0 && huge > 100, "nan={nan} huge={huge}");
    }

    #[test]
    fn weighted_pick_and_bool_with() {
        let mut g = Source::new(5, 10);
        let items = [(0u32, "never"), (3, "often"), (1, "sometimes")];
        let mut counts = [0usize; 3];
        for _ in 0..1000 {
            let picked = *g.weighted(&items);
            counts[items.iter().position(|(_, s)| *s == picked).unwrap()] += 1;
        }
        assert_eq!(counts[0], 0);
        assert!(counts[1] > counts[2]);
        let trues = (0..1000).filter(|_| g.bool_with(0.9)).count();
        assert!(trues > 800, "{trues}");
        let letters = ['x', 'y'];
        assert!(letters.contains(g.pick(&letters)));
    }

    #[test]
    fn discard_keeps_first_reason_and_derive_is_independent() {
        let mut g = Source::new(1, 3);
        g.discard("first");
        g.discard(String::from("second"));
        assert_eq!(g.discarded(), Some("first"));
        let mut d = g.derive();
        assert_eq!(d.size(), 3);
        assert_ne!(d.next_u64(), g.next_u64());
    }
}
