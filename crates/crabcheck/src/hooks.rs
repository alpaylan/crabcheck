//! Type-level shrinking and mutation: [`Shrink`] and [`Mutate`], with the
//! algorithms the built-in generators share.
//!
//! Generators carry their own hooks (see [`Gen::shrink`](crate::Gen::shrink)
//! and [`Gen::mutate`](crate::Gen::mutate)), which know the generator's
//! range. These traits are the fallback for a *type*: derived by
//! `#[derive(Arbitrary)]`, implemented here for the std types, and attached
//! to a generator with [`GenExt::typed`](crate::GenExt::typed).
//!
//! Both are ordinary code an author can write and the library can verify:
//! see [`check_shrinker`](crate::check_shrinker) and
//! [`check_mutator`](crate::check_mutator).

use std::collections::{
    BTreeMap,
    BTreeSet,
    HashMap,
    HashSet,
};

use rand_core::RngCore;

use crate::{
    Source,
    source::{
        Int,
        small_magnitude,
        uniform,
    },
};

/// Simpler candidates for a value, most promising first. Candidates must
/// stay inside the generator's support and be strictly smaller under some
/// measure; [`check_shrinker`](crate::check_shrinker) verifies both.
pub trait Shrink: Sized {
    /// Simpler candidates for `self`, most promising first.
    fn shrink(&self) -> Vec<Self>;
}

/// A nearby variant of a value for targeted search, drawing randomness from
/// a [`Source`]. Return `None` when no mutation applies.
pub trait Mutate: Sized {
    /// A nearby variant of `self`.
    fn mutate(&self, g: &mut Source) -> Option<Self>;
}

// --------------------------------------------------------------------------
// Shared algorithms
// --------------------------------------------------------------------------

/// Candidates for an integer in `lo..=hi`: the value closest to zero, then
/// binary steps from `value` toward it.
pub(crate) fn shrink_int<T: Int>(value: T, lo: i128, hi: i128) -> Vec<T> {
    let v = value.to_i128();
    let target = 0i128.clamp(lo, hi);
    if v == target {
        return Vec::new();
    }
    let mut out = vec![target];
    let mut step = (v - target) / 2;
    while step != 0 {
        let candidate = v - step;
        if candidate != target {
            out.push(candidate);
        }
        step /= 2;
    }
    let last = if v > target { v - 1 } else { v + 1 };
    if last != target && !out.contains(&last) {
        out.push(last);
    }
    out.into_iter().map(T::from_i128).collect()
}

/// A nearby integer in `lo..=hi`: a small step three times in four,
/// otherwise a fresh uniform draw.
pub(crate) fn mutate_int<T: Int>(value: T, lo: i128, hi: i128, g: &mut Source) -> T {
    let v = value.to_i128();
    let next = if g.next_u32().is_multiple_of(4) {
        lo + i128::from(uniform(g, (hi - lo) as u64))
    } else {
        let step = i128::from(small_magnitude(g));
        if g.next_u32() & 1 == 0 { v.saturating_add(step) } else { v.saturating_sub(step) }
    };
    T::from_i128(next.clamp(lo, hi))
}

/// Candidates for a float: zero, the positive value, its integer part, half.
pub(crate) fn shrink_f64(value: f64) -> Vec<f64> {
    if value == 0.0 {
        return Vec::new();
    }
    let mut out = vec![0.0];
    if value.is_nan() {
        out.push(1.0);
    } else if value.is_infinite() {
        out.push(if value > 0.0 { f64::MAX } else { f64::MIN });
    } else {
        if value < 0.0 {
            out.push(-value);
        }
        let truncated = value.trunc();
        if truncated != value {
            out.push(truncated);
        }
        out.push(value / 2.0);
    }
    out.dedup_by(|a, b| a.to_bits() == b.to_bits());
    out.into_iter().filter(|c| c.to_bits() != value.to_bits()).collect()
}

/// A nearby float: negated, doubled, halved, nudged, or a fresh bit pattern.
pub(crate) fn mutate_f64(value: f64, g: &mut Source) -> f64 {
    match g.next_u32() % 5 {
        0 => -value,
        1 => value * 2.0,
        2 => value / 2.0,
        3 => value + (small_magnitude(g) as f64) * if g.bool() { 1.0 } else { -1.0 },
        _ => g.f64_bits(),
    }
}

/// Candidates for a sequence: drop blocks of 8, 4, 2, 1 elements (down to
/// `min` elements), then shrink each element in place.
pub(crate) fn shrink_seq<T: Clone>(
    items: &[T],
    min: usize,
    shrink_elem: impl Fn(&T) -> Vec<T>,
) -> Vec<Vec<T>> {
    let mut out = Vec::new();
    let len = items.len();
    for block in [8usize, 4, 2, 1] {
        if len < block || len - block < min {
            continue;
        }
        for start in 0..=len - block {
            let mut candidate = Vec::with_capacity(len - block);
            candidate.extend_from_slice(&items[..start]);
            candidate.extend_from_slice(&items[start + block..]);
            out.push(candidate);
        }
    }
    for (i, item) in items.iter().enumerate() {
        for smaller in shrink_elem(item) {
            let mut candidate = items.to_vec();
            candidate[i] = smaller;
            out.push(candidate);
        }
    }
    out
}

/// A nearby sequence: one element mutated, an element inserted (when a
/// generator is available), removed, duplicated, or two swapped, within
/// `min..=max` elements.
pub(crate) fn mutate_seq<T: Clone>(
    items: &[T],
    min: usize,
    max: usize,
    g: &mut Source,
    mut fresh: Option<&mut dyn FnMut(&mut Source) -> T>,
    mutate_elem: impl Fn(&T, &mut Source) -> Option<T>,
) -> Option<Vec<T>> {
    let len = items.len();
    for _ in 0..8 {
        let mut out = items.to_vec();
        match g.next_u32() % 5 {
            0 if len > 0 => {
                let i = g.index(len);
                if let Some(m) = mutate_elem(&items[i], g) {
                    out[i] = m;
                    return Some(out);
                }
            },
            1 if len < max => {
                if let Some(fresh) = fresh.as_mut() {
                    let at = g.int(0..=len);
                    let value = fresh(g);
                    out.insert(at, value);
                    return Some(out);
                }
            },
            2 if len > min && len > 0 => {
                out.remove(g.index(len));
                return Some(out);
            },
            3 if len > 0 && len < max => {
                let i = g.index(len);
                let copy = out[i].clone();
                out.insert(i, copy);
                return Some(out);
            },
            4 if len >= 2 => {
                let a = g.index(len);
                let b = g.index(len);
                if a != b {
                    out.swap(a, b);
                    return Some(out);
                }
            },
            _ => {},
        }
    }
    None
}

/// Characters shrink toward `'a'`, in binary steps along the code points.
pub(crate) fn shrink_char(value: char) -> Vec<char> {
    let target = i64::from('a' as u32);
    let code = i64::from(value as u32);
    if code == target {
        return Vec::new();
    }
    let mut out = vec!['a'];
    let mut step = (code - target) / 2;
    while step != 0 {
        if let Some(c) = char::from_u32((code - step) as u32)
            && c != 'a'
            && !out.contains(&c)
        {
            out.push(c);
        }
        step /= 2;
    }
    out
}

// --------------------------------------------------------------------------
// std impls
// --------------------------------------------------------------------------

macro_rules! impl_int_hooks {
    ($($t:ty),*) => {$(
        impl Shrink for $t {
            fn shrink(&self) -> Vec<Self> {
                shrink_int(*self, <$t>::MIN as i128, <$t>::MAX as i128)
            }
        }
        impl Mutate for $t {
            fn mutate(&self, g: &mut Source) -> Option<Self> {
                Some(mutate_int(*self, <$t>::MIN as i128, <$t>::MAX as i128, g))
            }
        }
    )*};
}
impl_int_hooks!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

impl Shrink for bool {
    fn shrink(&self) -> Vec<Self> {
        if *self { vec![false] } else { Vec::new() }
    }
}

impl Mutate for bool {
    fn mutate(&self, _g: &mut Source) -> Option<Self> {
        Some(!*self)
    }
}

impl Shrink for f64 {
    fn shrink(&self) -> Vec<Self> {
        shrink_f64(*self)
    }
}

impl Mutate for f64 {
    fn mutate(&self, g: &mut Source) -> Option<Self> {
        Some(mutate_f64(*self, g))
    }
}

impl Shrink for f32 {
    fn shrink(&self) -> Vec<Self> {
        shrink_f64(f64::from(*self)).into_iter().map(|v| v as f32).collect()
    }
}

impl Mutate for f32 {
    fn mutate(&self, g: &mut Source) -> Option<Self> {
        Some(mutate_f64(f64::from(*self), g) as f32)
    }
}

impl Shrink for char {
    fn shrink(&self) -> Vec<Self> {
        shrink_char(*self)
    }
}

impl Mutate for char {
    fn mutate(&self, g: &mut Source) -> Option<Self> {
        let code = i64::from(*self as u32);
        let next = if g.bool() {
            let step = small_magnitude(g) as i64 * if g.bool() { 1 } else { -1 };
            (code + step).clamp(0, 0x10FFFF) as u32
        } else {
            let v = g.int(0u32..=0x10FFFF - 0x800);
            if v < 0xD800 { v } else { v + 0x800 }
        };
        char::from_u32(next).or_else(|| char::from_u32(next + 0x800))
    }
}

impl Shrink for String {
    fn shrink(&self) -> Vec<Self> {
        let chars: Vec<char> = self.chars().collect();
        shrink_seq(&chars, 0, |c| c.shrink())
            .into_iter()
            .map(|cs| cs.into_iter().collect())
            .collect()
    }
}

impl Mutate for String {
    fn mutate(&self, g: &mut Source) -> Option<Self> {
        let chars: Vec<char> = self.chars().collect();
        mutate_seq(&chars, 0, usize::MAX, g, None, |c, g| c.mutate(g))
            .map(|cs| cs.into_iter().collect())
    }
}

impl Shrink for () {
    fn shrink(&self) -> Vec<Self> {
        Vec::new()
    }
}

impl Mutate for () {
    fn mutate(&self, _g: &mut Source) -> Option<Self> {
        None
    }
}

impl<T: Shrink + Clone> Shrink for Vec<T> {
    fn shrink(&self) -> Vec<Self> {
        shrink_seq(self, 0, T::shrink)
    }
}

impl<T: Mutate + Clone> Mutate for Vec<T> {
    fn mutate(&self, g: &mut Source) -> Option<Self> {
        mutate_seq(self, 0, usize::MAX, g, None, |x, g| x.mutate(g))
    }
}

impl<T: Shrink + Clone> Shrink for Option<T> {
    fn shrink(&self) -> Vec<Self> {
        match self {
            None => Vec::new(),
            Some(x) => std::iter::once(None).chain(x.shrink().into_iter().map(Some)).collect(),
        }
    }
}

impl<T: Mutate + Clone> Mutate for Option<T> {
    fn mutate(&self, g: &mut Source) -> Option<Self> {
        match self {
            None => None,
            Some(x) => {
                if g.next_u32().is_multiple_of(8) {
                    Some(None)
                } else {
                    x.mutate(g).map(Some)
                }
            },
        }
    }
}

impl<T: Shrink + Clone, E: Shrink + Clone> Shrink for Result<T, E> {
    fn shrink(&self) -> Vec<Self> {
        match self {
            Ok(x) => x.shrink().into_iter().map(Ok).collect(),
            Err(e) => e.shrink().into_iter().map(Err).collect(),
        }
    }
}

impl<T: Mutate + Clone, E: Mutate + Clone> Mutate for Result<T, E> {
    fn mutate(&self, g: &mut Source) -> Option<Self> {
        match self {
            Ok(x) => x.mutate(g).map(Ok),
            Err(e) => e.mutate(g).map(Err),
        }
    }
}

impl<T: Shrink> Shrink for Box<T> {
    fn shrink(&self) -> Vec<Self> {
        (**self).shrink().into_iter().map(Box::new).collect()
    }
}

impl<T: Mutate> Mutate for Box<T> {
    fn mutate(&self, g: &mut Source) -> Option<Self> {
        (**self).mutate(g).map(Box::new)
    }
}

impl<T: Shrink + Clone, const N: usize> Shrink for [T; N] {
    fn shrink(&self) -> Vec<Self> {
        let mut out = Vec::new();
        for i in 0..N {
            for smaller in self[i].shrink() {
                let mut candidate = self.clone();
                candidate[i] = smaller;
                out.push(candidate);
            }
        }
        out
    }
}

impl<T: Mutate + Clone, const N: usize> Mutate for [T; N] {
    fn mutate(&self, g: &mut Source) -> Option<Self> {
        if N == 0 {
            return None;
        }
        let i = g.index(N);
        let mutated = self[i].mutate(g)?;
        let mut out = self.clone();
        out[i] = mutated;
        Some(out)
    }
}

macro_rules! impl_tuple_hooks {
    ($(($T:ident, $i:tt)),+) => {
        impl<$($T: Shrink + Clone),+> Shrink for ($($T,)+) {
            fn shrink(&self) -> Vec<Self> {
                let mut out = Vec::new();
                $(
                    for smaller in self.$i.shrink() {
                        let mut candidate = self.clone();
                        candidate.$i = smaller;
                        out.push(candidate);
                    }
                )+
                out
            }
        }
        impl<$($T: Mutate + Clone),+> Mutate for ($($T,)+) {
            fn mutate(&self, g: &mut Source) -> Option<Self> {
                let arity = [$($i,)+].len();
                let start = g.index(arity);
                for offset in 0..arity {
                    let which = (start + offset) % arity;
                    $(
                        if which == $i && let Some(m) = self.$i.mutate(g) {
                            let mut out = self.clone();
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
impl_tuple_hooks!((A, 0));
impl_tuple_hooks!((A, 0), (B, 1));
impl_tuple_hooks!((A, 0), (B, 1), (C, 2));
impl_tuple_hooks!((A, 0), (B, 1), (C, 2), (D, 3));
impl_tuple_hooks!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4));
impl_tuple_hooks!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5));
impl_tuple_hooks!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6));
impl_tuple_hooks!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7));

/// Maps shrink by dropping entries and shrinking values; keys are left
/// alone so the result is still a map.
macro_rules! impl_map_hooks {
    ($map:ident, $($bound:tt)+) => {
        impl<K: Clone + $($bound)+, V: Shrink + Clone> Shrink for $map<K, V> {
            fn shrink(&self) -> Vec<Self> {
                let entries: Vec<(K, V)> = self.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                shrink_seq(&entries, 0, |(k, v)| v.shrink().into_iter().map(|v| (k.clone(), v)).collect())
                    .into_iter()
                    .map(|e| e.into_iter().collect())
                    .collect()
            }
        }
        impl<K: Clone + $($bound)+, V: Mutate + Clone> Mutate for $map<K, V> {
            fn mutate(&self, g: &mut Source) -> Option<Self> {
                let entries: Vec<(K, V)> = self.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                mutate_seq(&entries, 0, usize::MAX, g, None, |(k, v), g| v.mutate(g).map(|v| (k.clone(), v)))
                    .map(|e| e.into_iter().collect())
            }
        }
    };
}
impl_map_hooks!(HashMap, std::hash::Hash + Eq);
impl_map_hooks!(BTreeMap, Ord);

macro_rules! impl_set_hooks {
    ($set:ident, $($bound:tt)+) => {
        impl<T: Shrink + Clone + $($bound)+> Shrink for $set<T> {
            fn shrink(&self) -> Vec<Self> {
                let items: Vec<T> = self.iter().cloned().collect();
                shrink_seq(&items, 0, T::shrink).into_iter().map(|e| e.into_iter().collect()).collect()
            }
        }
        impl<T: Mutate + Clone + $($bound)+> Mutate for $set<T> {
            fn mutate(&self, g: &mut Source) -> Option<Self> {
                let items: Vec<T> = self.iter().cloned().collect();
                mutate_seq(&items, 0, usize::MAX, g, None, |x, g| x.mutate(g)).map(|e| e.into_iter().collect())
            }
        }
    };
}
impl_set_hooks!(HashSet, std::hash::Hash + Eq);
impl_set_hooks!(BTreeSet, Ord);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ints_shrink_toward_zero_or_the_nearest_bound() {
        assert_eq!(100i32.shrink(), vec![0, 50, 75, 88, 94, 97, 99]);
        assert_eq!((-3i32).shrink(), vec![0, -2]);
        assert_eq!(0u8.shrink(), Vec::<u8>::new());
        assert_eq!(shrink_int(20i32, 10, 30), vec![10, 15, 18, 19]);
        assert_eq!(shrink_int(-20i32, -30, -10), vec![-10, -15, -18, -19]);
        assert_eq!(shrink_int(10i32, 10, 30), Vec::<i32>::new());
        assert_eq!(1i32.shrink(), vec![0]);
    }

    #[test]
    fn floats_bools_chars_shrink_sensibly() {
        assert_eq!((-7.5f64).shrink(), vec![0.0, 7.5, -7.0, -3.75]);
        assert_eq!(f64::NAN.shrink(), vec![0.0, 1.0]);
        assert_eq!(0.0f64.shrink(), Vec::<f64>::new());
        assert_eq!(true.shrink(), vec![false]);
        assert_eq!('a'.shrink(), Vec::<char>::new());
        assert_eq!('e'.shrink(), vec!['a', 'c', 'd']);
        assert!('A'.shrink().contains(&'a'));
    }

    #[test]
    fn sequences_drop_blocks_then_shrink_elements() {
        let v = vec![5i32, 6, 7];
        let candidates = v.shrink();
        assert_eq!(candidates[0], vec![7], "largest block first");
        assert_eq!(candidates[1], vec![5]);
        assert!(
            candidates.contains(&vec![6, 7])
                && candidates.contains(&vec![5, 7])
                && candidates.contains(&vec![5, 6])
        );
        assert!(candidates.contains(&vec![0, 6, 7]));
        assert!(candidates.iter().all(|c| c.len() <= 3));
        let respects_min: Vec<Vec<i32>> = shrink_seq(&v, 3, |x| x.shrink());
        assert!(respects_min.iter().all(|c| c.len() == 3));
        assert!(String::from("ba").shrink().contains(&"a".to_string()));
        assert_eq!(Some(2i32).shrink(), vec![None, Some(0), Some(1)]);
        assert_eq!((3i32, true).shrink(), vec![(0, true), (2, true), (3, false)]);
    }

    #[test]
    fn mutations_stay_in_range_and_change_something() {
        let mut g = Source::new(9, 10);
        let mut same = 0;
        for _ in 0..500 {
            let m = mutate_int(20i32, 10, 30, &mut g);
            assert!((10..=30).contains(&m));
            if (1u8, 'x').mutate(&mut g).unwrap() == (1, 'x') {
                same += 1;
            }
            let _ = vec![1u8, 2, 3].mutate(&mut g).unwrap();
        }
        assert!(same < 10, "{same} mutations left the tuple unchanged");
        let changed = (0..200).filter(|_| 5i32.mutate(&mut g) != Some(5)).count();
        assert!(changed > 150, "{changed}");
        let m: HashMap<u8, i32> = [(1, 10), (2, 20)].into_iter().collect();
        assert!(m.shrink().iter().all(|c| c.len() <= 2));
        assert!(m.mutate(&mut g).is_some());
    }
}
