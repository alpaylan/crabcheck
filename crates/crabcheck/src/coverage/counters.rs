//! Counter snapshots and function records read from the profile runtime.

use std::{
    collections::HashMap,
    ops::Range,
    sync::{
        Mutex,
        MutexGuard,
        OnceLock,
    },
};

/// Whether this binary was built with `-C instrument-coverage` and the
/// profile runtime is linked in.
pub fn available() -> bool {
    cfg!(crabcheck_instrumented)
}

static LOCK: Mutex<()> = Mutex::new(());

/// The process-wide lock coverage-using phases hold so that crabcheck's own
/// runs do not pollute each other's deltas. Other test threads are not
/// stopped by it; run with `RUST_TEST_THREADS=1` for exact attribution.
pub fn lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The counter section (or a window of it) at one moment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    offset: usize,
    counters: Vec<u64>,
}

impl Snapshot {
    /// Number of counters in the binary.
    pub fn len(&self) -> usize {
        self.counters.len()
    }

    /// Whether the binary has no counters (always true when uninstrumented).
    pub fn is_empty(&self) -> bool {
        self.counters.is_empty()
    }

    /// The counter values.
    pub fn counters(&self) -> &[u64] {
        &self.counters
    }

    /// Counters that increased between `self` and `later` (both must cover
    /// the same window). Indices are absolute counter indices.
    pub fn delta(&self, later: &Snapshot) -> Delta {
        let hits = self
            .counters
            .iter()
            .zip(&later.counters)
            .enumerate()
            .filter(|(_, (before, after))| after > before)
            .map(|(index, (before, after))| (self.offset + index, after - before))
            .collect();
        Delta { hits }
    }
}

/// Counters that increased between two snapshots, as `(index, increment)`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Delta {
    hits: Vec<(usize, u64)>,
}

impl Delta {
    /// The counters hit, with how many times each was incremented.
    pub fn hits(&self) -> &[(usize, u64)] {
        &self.hits
    }

    /// Whether any counter was hit.
    pub fn is_empty(&self) -> bool {
        self.hits.is_empty()
    }

    /// Whether the counter at `index` was hit.
    pub fn touches(&self, index: usize) -> bool {
        self.hits.binary_search_by_key(&index, |(i, _)| *i).is_ok()
    }

    /// Restrict to counters inside `range`.
    pub fn within(&self, range: &Range<usize>) -> Delta {
        Delta { hits: self.hits.iter().copied().filter(|(i, _)| range.contains(i)).collect() }
    }
}

/// One instrumented function: its demangled name and the counters that
/// belong to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Function {
    /// Demangled name, e.g. `my_crate::module::function`.
    pub name: String,
    /// Indices into a [`Snapshot`] of this function's counters.
    pub counters: Range<usize>,
}

/// Every instrumented function in the binary, from the profile data records
/// and the names section. Built once per process.
#[derive(Debug)]
pub struct Functions {
    functions: Vec<Function>,
}

impl Functions {
    /// The instrumented functions, or an empty list when uninstrumented.
    pub fn get() -> &'static Functions {
        static FUNCTIONS: OnceLock<Functions> = OnceLock::new();
        FUNCTIONS.get_or_init(|| Functions { functions: runtime::functions() })
    }

    /// All functions.
    pub fn all(&self) -> &[Function] {
        &self.functions
    }

    /// Functions whose demangled name starts with `prefix` (a crate name,
    /// or `crate::module`).
    pub fn in_crate<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = &'a Function> + 'a {
        self.functions.iter().filter(move |f| {
            f.name.starts_with(prefix) && f.name[prefix.len()..].starts_with("::")
                || f.name.trim_start_matches('<').starts_with(prefix)
        })
    }

    /// Functions touched by `delta`.
    pub fn reached<'a>(&'a self, delta: &'a Delta) -> impl Iterator<Item = &'a Function> + 'a {
        self.functions.iter().filter(|f| delta.hits.iter().any(|(i, _)| f.counters.contains(i)))
    }
}

/// A snapshot of the whole counter section, or `None` when uninstrumented.
pub fn snapshot() -> Option<Snapshot> {
    runtime::counters(None).map(|counters| Snapshot { offset: 0, counters })
}

/// A snapshot of the counters in `range` only; cheaper than the whole
/// section when only one crate matters.
pub fn snapshot_range(range: &Range<usize>) -> Option<Snapshot> {
    runtime::counters(Some(range.clone()))
        .map(|counters| Snapshot { offset: range.start, counters })
}

/// A profraw image of the process's counters, for `llvm-profdata` and
/// `llvm-cov`. `None` when uninstrumented.
pub fn profraw_image() -> Option<Vec<u8>> {
    runtime::profraw()
}

/// The only unsafe code in the crate: FFI into the LLVM profile runtime and
/// reads of its static sections, whose layout is fixed by compiler-rt.
#[cfg(crabcheck_instrumented)]
#[allow(unsafe_code)]
mod runtime {
    use std::sync::atomic::{
        AtomicU64,
        Ordering,
    };

    use super::Function;

    unsafe extern "C" {
        fn __llvm_profile_begin_counters() -> *mut u8;
        fn __llvm_profile_end_counters() -> *mut u8;
        fn __llvm_profile_begin_data() -> *const u8;
        fn __llvm_profile_end_data() -> *const u8;
        fn __llvm_profile_begin_names() -> *const u8;
        fn __llvm_profile_end_names() -> *const u8;
        fn __llvm_profile_counter_entry_size() -> usize;
        fn __llvm_profile_get_size_for_buffer() -> u64;
        fn __llvm_profile_write_buffer(buffer: *mut u8) -> i32;
    }

    /// Size of one `__llvm_profile_data` record (LLVM 20 to 22).
    const RECORD_SIZE: usize = 64;

    pub(super) fn counters(range: Option<std::ops::Range<usize>>) -> Option<Vec<u64>> {
        // SAFETY: the profile runtime owns a contiguous, process-lifetime
        // counter section; the compiler increments its entries atomically.
        unsafe {
            if __llvm_profile_counter_entry_size() != 8 {
                return None;
            }
            let begin = __llvm_profile_begin_counters();
            let end = __llvm_profile_end_counters();
            let count = end.offset_from(begin).max(0) as usize / 8;
            let slice = std::slice::from_raw_parts(begin as *const AtomicU64, count);
            let range = range.map_or(0..count, |r| r.start.min(count)..r.end.min(count));
            Some(slice[range].iter().map(|c| c.load(Ordering::Relaxed)).collect())
        }
    }

    pub(super) fn profraw() -> Option<Vec<u8>> {
        // SAFETY: write_buffer fills exactly get_size_for_buffer bytes.
        unsafe {
            let size = __llvm_profile_get_size_for_buffer() as usize;
            let mut buffer = vec![0u8; size];
            if __llvm_profile_write_buffer(buffer.as_mut_ptr()) != 0 {
                return None;
            }
            Some(buffer)
        }
    }

    pub(super) fn functions() -> Vec<Function> {
        // SAFETY: the data and names sections are static, contiguous, and
        // laid out as documented in compiler-rt's InstrProfData.inc.
        let (records, names, counters_base) = unsafe {
            let begin = __llvm_profile_begin_data();
            let end = __llvm_profile_end_data();
            let len = end.offset_from(begin).max(0) as usize;
            let records = std::slice::from_raw_parts(begin, len);
            let nb = __llvm_profile_begin_names();
            let ne = __llvm_profile_end_names();
            let names = std::slice::from_raw_parts(nb, ne.offset_from(nb).max(0) as usize);
            (records, names, __llvm_profile_begin_counters() as usize)
        };
        let names = super::super::names::parse(names);
        let by_ref = super::super::names::by_ref(&names);
        let mut functions = Vec::with_capacity(records.len() / RECORD_SIZE);
        for (index, record) in records.chunks_exact(RECORD_SIZE).enumerate() {
            let name_ref = u64::from_le_bytes(record[0..8].try_into().expect("8 bytes"));
            let counter_ptr = i64::from_le_bytes(record[16..24].try_into().expect("8 bytes"));
            let num_counters =
                u32::from_le_bytes(record[48..52].try_into().expect("4 bytes")) as usize;
            let record_addr = unsafe { __llvm_profile_begin_data() as usize } + index * RECORD_SIZE;
            let counter_addr = (record_addr as i64 + counter_ptr) as usize;
            if counter_addr < counters_base || num_counters == 0 {
                continue;
            }
            let start = (counter_addr - counters_base) / 8;
            let name = by_ref.get(&name_ref).map_or_else(
                || format!("<unknown {name_ref:#x}>"),
                |n| super::super::names::pretty(n),
            );
            functions.push(Function { name, counters: start..start + num_counters });
        }
        functions.sort_by_key(|f| f.counters.start);
        functions
    }
}

#[cfg(not(crabcheck_instrumented))]
mod runtime {
    use super::Function;

    pub(super) fn counters(_range: Option<std::ops::Range<usize>>) -> Option<Vec<u64>> {
        None
    }

    pub(super) fn functions() -> Vec<Function> {
        Vec::new()
    }

    pub(super) fn profraw() -> Option<Vec<u8>> {
        None
    }
}

/// Group a delta by function name: how many counter increments landed in
/// each function.
pub fn hits_by_function(functions: &Functions, delta: &Delta) -> HashMap<String, u64> {
    let mut out = HashMap::new();
    for function in functions.reached(delta) {
        let total: u64 =
            delta.hits.iter().filter(|(i, _)| function.counters.contains(i)).map(|(_, n)| n).sum();
        *out.entry(function.name.clone()).or_default() += total;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        super::names,
        *,
    };

    #[test]
    fn delta_and_within() {
        let before = Snapshot { offset: 0, counters: vec![1, 5, 0, 7] };
        let after = Snapshot { offset: 0, counters: vec![1, 9, 3, 7] };
        let delta = before.delta(&after);
        assert_eq!(delta.hits(), &[(1, 4), (2, 3)]);
        assert!(delta.touches(2) && !delta.touches(3));
        assert_eq!(delta.within(&(2..4)).hits(), &[(2, 3)]);
        assert!(
            Snapshot { offset: 0, counters: vec![] }
                .delta(&Snapshot { offset: 0, counters: vec![] })
                .is_empty()
        );
        let windowed = Snapshot { offset: 10, counters: vec![0, 1] }
            .delta(&Snapshot { offset: 10, counters: vec![0, 2] });
        assert_eq!(windowed.hits(), &[(11, 1)]);
    }

    #[test]
    fn availability_matches_the_build() {
        assert_eq!(available(), snapshot().is_some());
        if available() {
            let functions = Functions::get();
            assert!(!functions.all().is_empty());
            assert!(
                functions.all().iter().any(|f| f.name.contains("crabcheck")),
                "{:?}",
                &functions.all()[..5.min(functions.all().len())]
            );
            // Running code increments its counters.
            let before = snapshot().unwrap();
            let _ = std::hint::black_box(names::md5(b"delta"));
            let after = snapshot().unwrap();
            let delta = before.delta(&after);
            assert!(
                functions.reached(&delta).any(|f| f.name.contains("md5")),
                "{:?}",
                hits_by_function(functions, &delta)
            );
        }
    }
}
