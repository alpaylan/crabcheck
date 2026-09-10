//! [`Arbitrary`]: the default generator of a type, and [`any`].

use std::collections::{
    BTreeMap,
    BTreeSet,
    HashMap,
    HashSet,
};

use crate::{
    gens::{
        BoxGen,
        GenExt,
    },
    primitives as gens,
};

/// Types with a default generator. Use [`any`] to obtain it.
///
/// The built-in defaults are documented mixes of visible generators, so the
/// baseline distribution is explicit and can be rebuilt with the same
/// combinators:
///
/// - integers: `frequency([(1, edges(..)), (7, int(..))])`, so one draw in
///   eight is a range end, `0`, `±1` or a neighbour of an end;
/// - floats: `frequency([(1, specials), (2, small round numbers), (2, unit
///   interval), (3, random bit patterns)])`, where the specials are `0`,
///   `-0`, `±1`, `±0.5`, NaN, `±inf`, `MIN`, `MAX`, `EPSILON`,
///   `MIN_POSITIVE` and the smallest subnormal;
/// - collections: length uniform in `0..=size`, elements from their
///   defaults.
///
/// Every default carries the shrinker and mutator of the generators it is
/// built from.
///
/// Implement this for your own types when one generator is the obvious
/// default; otherwise pass generators explicitly.
pub trait Arbitrary: Sized + 'static {
    /// The default generator for this type.
    fn arbitrary() -> BoxGen<Self>;
}

/// The default generator for `T`.
pub fn any<T: Arbitrary>() -> BoxGen<T> {
    T::arbitrary()
}

macro_rules! impl_arbitrary_int {
    ($($t:ty),*) => {$(
        impl Arbitrary for $t {
            fn arbitrary() -> BoxGen<Self> {
                gens::frequency(vec![(1, gens::edges::<$t>(..).boxed()), (7, gens::int::<$t>(..).boxed())]).boxed()
            }
        }
    )*};
}
impl_arbitrary_int!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

impl Arbitrary for bool {
    fn arbitrary() -> BoxGen<Self> {
        gens::bool().boxed()
    }
}

const F64_SPECIALS: [f64; 14] = [
    0.0,
    -0.0,
    1.0,
    -1.0,
    0.5,
    -0.5,
    f64::NAN,
    f64::INFINITY,
    f64::NEG_INFINITY,
    f64::MAX,
    f64::MIN,
    f64::EPSILON,
    f64::MIN_POSITIVE,
    5e-324,
];

const F32_SPECIALS: [f32; 14] = [
    0.0,
    -0.0,
    1.0,
    -1.0,
    0.5,
    -0.5,
    f32::NAN,
    f32::INFINITY,
    f32::NEG_INFINITY,
    f32::MAX,
    f32::MIN,
    f32::EPSILON,
    f32::MIN_POSITIVE,
    1e-45,
];

const FRACTIONS: [f64; 4] = [0.0, 0.5, 0.25, 0.1];

impl Arbitrary for f64 {
    fn arbitrary() -> BoxGen<Self> {
        gens::frequency(vec![
            (1, gens::pick_from(F64_SPECIALS.to_vec()).boxed()),
            (
                2,
                (gens::int(-100i32..=100), gens::int(0usize..4))
                    .map(|(i, f)| f64::from(i) + FRACTIONS[f])
                    .boxed(),
            ),
            (2, gens::f64().boxed()),
            (3, gens::f64_bits().boxed()),
        ])
        .boxed()
    }
}

impl Arbitrary for f32 {
    fn arbitrary() -> BoxGen<Self> {
        gens::frequency(vec![
            (1, gens::pick_from(F32_SPECIALS.to_vec()).boxed()),
            (
                2,
                (gens::int(-100i32..=100), gens::int(0usize..4))
                    .map(|(i, f)| i as f32 + FRACTIONS[f] as f32)
                    .boxed(),
            ),
            (2, gens::f32().boxed()),
            (3, gens::f32_bits().boxed()),
        ])
        .boxed()
    }
}

impl Arbitrary for char {
    fn arbitrary() -> BoxGen<Self> {
        gens::char().boxed()
    }
}

impl Arbitrary for String {
    fn arbitrary() -> BoxGen<Self> {
        gens::string().boxed()
    }
}

impl Arbitrary for () {
    fn arbitrary() -> BoxGen<Self> {
        gens::constant(()).boxed()
    }
}

impl<T: Arbitrary + Clone> Arbitrary for Vec<T> {
    fn arbitrary() -> BoxGen<Self> {
        gens::vec(any::<T>()).boxed()
    }
}

impl<T: Arbitrary> Arbitrary for Option<T> {
    fn arbitrary() -> BoxGen<Self> {
        gens::option(any::<T>()).boxed()
    }
}

impl<T: Arbitrary, E: Arbitrary> Arbitrary for Result<T, E> {
    fn arbitrary() -> BoxGen<Self> {
        gens::result(any::<T>(), any::<E>()).boxed()
    }
}

impl<T: Arbitrary + Clone> Arbitrary for Box<T> {
    fn arbitrary() -> BoxGen<Self> {
        any::<T>().iso(Box::new, |b| (**b).clone()).boxed()
    }
}

impl<T: Arbitrary + Clone, const N: usize> Arbitrary for [T; N] {
    fn arbitrary() -> BoxGen<Self> {
        std::array::from_fn::<BoxGen<T>, N, _>(|_| any::<T>()).boxed()
    }
}

impl<K: Arbitrary + std::hash::Hash + Eq + Clone + Send + Sync, V: Arbitrary + Clone + Send + Sync>
    Arbitrary for HashMap<K, V>
{
    fn arbitrary() -> BoxGen<Self> {
        gens::hash_map(any::<K>(), any::<V>()).boxed()
    }
}

impl<K: Arbitrary + Ord + Clone + Send + Sync, V: Arbitrary + Clone + Send + Sync> Arbitrary
    for BTreeMap<K, V>
{
    fn arbitrary() -> BoxGen<Self> {
        gens::btree_map(any::<K>(), any::<V>()).boxed()
    }
}

impl<T: Arbitrary + std::hash::Hash + Eq + Clone + Send + Sync> Arbitrary for HashSet<T> {
    fn arbitrary() -> BoxGen<Self> {
        gens::hash_set(any::<T>()).boxed()
    }
}

impl<T: Arbitrary + Ord + Clone + Send + Sync> Arbitrary for BTreeSet<T> {
    fn arbitrary() -> BoxGen<Self> {
        gens::btree_set(any::<T>()).boxed()
    }
}

macro_rules! impl_arbitrary_tuple {
    ($($T:ident),+) => {
        impl<$($T: Arbitrary + Clone),+> Arbitrary for ($($T,)+) {
            fn arbitrary() -> BoxGen<Self> {
                ($(any::<$T>(),)+).boxed()
            }
        }
    };
}
impl_arbitrary_tuple!(A);
impl_arbitrary_tuple!(A, B);
impl_arbitrary_tuple!(A, B, C);
impl_arbitrary_tuple!(A, B, C, D);
impl_arbitrary_tuple!(A, B, C, D, E);
impl_arbitrary_tuple!(A, B, C, D, E, F);
impl_arbitrary_tuple!(A, B, C, D, E, F, G);
impl_arbitrary_tuple!(A, B, C, D, E, F, G, H);

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            Gen,
            Source,
        },
    };

    #[test]
    fn std_types_have_default_generators() {
        let mut g = Source::new(1, 8);
        let _: (i32, u64, bool, f64, char, String) = any().generate(&mut g);
        let v: Vec<Option<u8>> = any().generate(&mut g);
        assert!(v.len() <= 8);
        let _: Result<Box<i8>, ()> = any().generate(&mut g);
        let _: [bool; 4] = any().generate(&mut g);
        let _: HashMap<u8, i16> = any().generate(&mut g);
        let _: BTreeSet<char> = any().generate(&mut g);
        let _: (i8, i16, i32, i64, isize, u8, u16, u32) = any().generate(&mut g);
    }

    #[test]
    fn default_mixes_hit_edges_and_specials() {
        let mut g = Source::new(5, 10);
        let (mut min, mut max, mut nan) = (0, 0, 0);
        for _ in 0..4000 {
            match any::<i32>().generate(&mut g) {
                i32::MIN => min += 1,
                i32::MAX => max += 1,
                _ => {},
            }
            if any::<f64>().generate(&mut g).is_nan() {
                nan += 1;
            }
        }
        assert!(min > 20 && max > 20 && nan > 5, "min={min} max={max} nan={nan}");
    }
}
