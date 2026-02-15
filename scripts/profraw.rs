// Cargo.toml (add as needed)
// [dependencies]
// bitflags = "2"
// thiserror = "1"
// serde = { version = "1", features = ["derive"] }
// rustc-demangle = "0.1"          // optional
// cpp_demangle = "0.4"            // optional

use {
    serde::{
        Deserialize,
        Serialize,
    },
    std::{
        borrow::Cow,
        num::NonZeroUsize,
    },
};

/// Raw file endianness signaled by the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Endianness {
    Little,
    Big,
}

/// A conservative representation of the raw header in `.profraw`.
/// Different LLVM versions add fields; keep unknown tail as `extra` to forward-compat.
/// Numeric fields use u64/i64 so we don’t truncate across 32/64-bit producers.
#[derive(Debug, Clone)]
pub struct RawHeader {
    /// Magic identifying raw instrprof file (e.g., 0xFFFFFF... style).
    pub magic: u64,
    /// Raw format version.
    pub version: u64,
    /// Target pointer width in bytes (4 or 8 typically).
    pub pointer_size: u8,
    /// File endianness.
    pub endianness: Endianness,

    /// Size (bytes) of the DATA records section.
    pub data_size: u64,
    /// Size (bytes) of the COUNTERS section (array of u64 counters).
    pub counters_size: u64,
    /// Size (bytes) of the NAMES section (NUL-terminated string table).
    pub names_size: u64,
    /// Size (bytes) of the VALUE profile payloads section.
    pub value_data_size: u64,

    /// Section base relocation deltas (file stores pointer-relative deltas).
    pub counters_delta: i64,
    pub names_delta: i64,
    pub value_data_delta: i64,

    /// The maximum value kind index in this file (IPVK_Last).
    /// Determines how many per-function value-site counts exist.
    pub value_kind_last: u32,

    /// Optional binary IDs size (present in newer LLVM). Zero if absent.
    pub binary_ids_size: u64,

    /// Any remaining bytes of the header we don’t explicitly decode (forward-compat).
    pub extra: Vec<u8>,
}

/// Lightweight view into a raw section (backed by the mmapped file).
#[derive(Debug, Clone, Copy)]
pub struct SectionView<'a> {
    pub bytes: &'a [u8],
}

/// Top-level raw profile file view after basic header/section slicing.
#[derive(Debug, Clone, Copy)]
pub struct RawProfile<'a> {
    pub header: &'a RawHeader,
    pub data: SectionView<'a>,
    pub counters: SectionView<'a>,
    pub names: SectionView<'a>,
    pub value_data: SectionView<'a>,
    /// Optional: a Binary IDs section if your tool wants to read producer IDs.
    pub binary_ids: Option<SectionView<'a>>,
}

/// Value profiling kinds (subset; extend as you need).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum ValueKind {
    IndirectCallTarget = 0, // IPVK_IndirectCallTarget
    MemOPSize = 1,          // IPVK_MemOPSize
    // Add more kinds here as needed. Keep mapping consistent with LLVM's IPVK_*.
    Unknown = 255,
}

/// A single “data record header” as it appears in the DATA section,
/// minimally represented so the reader can hop through records.
/// NB: the on-disk structure has alignment/padding rules; don’t #[repr(C)] here.
/// Parse field-by-field from bytes using the file’s endianness and pointer size.
#[derive(Debug, Clone)]
pub struct RawDataRecordHeader {
    /// Offset into the NAMES section to the NUL-terminated function name.
    pub name_ref: u64,
    /// LLVM’s function hash used to validate mapping with coverage.
    pub func_hash: u64,
    /// Number of counters for this function.
    pub num_counters: u32,
    /// Per-kind number of “value sites”. Length = value_kind_last + 1.
    pub num_value_sites: Vec<u16>,

    /// The byte offset (from start of COUNTERS section) where this function’s
    /// counters start (computed from header deltas + record’s encoded pointer).
    pub counters_offset: u64,

    /// The byte offset into VALUE section where this function’s value profile
    /// payloads begin (if any). Some producers store it implicitly; you may
    /// compute it while scanning.
    pub value_data_offset: Option<u64>,

    /// Total byte size of the raw record including any padding/alignment,
    /// to allow the iterator to advance to the next record.
    pub raw_record_size: NonZeroUsize,
}

// -------------------------------------------------------------------------
// Canonical, parsed model
// -------------------------------------------------------------------------

/// Parsed & ready-to-use counters/value profiles for a single function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionRecord<'a> {
    /// Mangled name as stored in the file (slice into `names` section).
    pub mangled: Cow<'a, str>,
    /// Optional demangled display name.
    pub demangled: Option<String>,
    /// Function hash from the profile.
    pub func_hash: u64,
    /// The execution counters: one per instrumented site in the function.
    pub counters: Cow<'a, [u64]>,
    /// Value profiling payloads grouped by kind.
    pub value_profile: ValueProfile,
}

/// Value profile data, organized per kind.
/// Keep this high-level; the raw encoding packs headers/tuples you’ll decode.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ValueProfile {
    /// For indirect call targets: (target symbol or address, count).
    pub indirect_call_targets: Vec<ProfiledTarget>,
    /// For MemOP sizes: (observed size, count).
    pub memop_sizes: Vec<ProfiledNumber>,

    /// Unknown/unsupported kinds kept losslessly by raw bytes (forward-compat).
    pub raw_unknown_kinds: Vec<UnknownValueKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfiledTarget {
    /// Prefer a demangled name if available; otherwise hex address string.
    pub target: String,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfiledNumber {
    pub value: u64,
    pub count: u64,
}

/// Raw catch-all to retain unrecognized value-kinds without data loss.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnknownValueKind {
    pub kind: u8,
    /// Opaque bytes for this function & kind; decode later if needed.
    pub payload: Vec<u8>,
}

/// The high-level artifact you can serialize to your analysis pipeline,
/// or transform into `llvm-cov export`-compatible JSON later.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedProfile<'a> {
    /// Raw file properties useful for debugging & reproducibility.
    pub file_meta: FileMeta,
    /// All functions found in the DATA section, in file order.
    pub functions: Vec<FunctionRecord<'a>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMeta {
    pub magic: u64,
    pub version: u64,
    pub pointer_size: u8,
    pub endianness: Endianness,
    pub value_kind_last: u32,
    pub producer_notes: Option<String>, // e.g., LLVM version string if you extract it
}

// -------------------------------------------------------------------------
// Merge & coverage join helpers
// -------------------------------------------------------------------------

/// Result of merging multiple `.profraw` files.
/// You can implement `merge_into` elementwise on counters and value profiles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergedProfile {
    pub file_meta: FileMeta,
    pub functions: Vec<MergedFunction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergedFunction {
    pub mangled: String,
    pub demangled: Option<String>,
    pub func_hash: u64,
    pub counters: Vec<u64>,
    pub value_profile: ValueProfile,
    /// Number of contributing files for this function.
    pub sources_merged: u32,
}

// -------------------------------------------------------------------------
// Error types
// -------------------------------------------------------------------------

#[derive(/* thiserror::Error, */ Debug)]
pub enum ProfrawError {
    // #[error("unrecognized magic {0:#x}")]
    BadMagic(u64),
    // #[error("unsupported version {found} (supported: {min}..={max})")]
    UnsupportedVersion { found: u64, min: u64, max: u64 },
    // #[error("unsupported pointer size {0}")]
    UnsupportedPointerSize(u8),
    // #[error("section out of bounds: {section}")]
    SectionBounds { section: &'static str },
    // #[error("misaligned offset in section {section}: offset={offset}, align={align}")]
    Misaligned { section: &'static str, offset: u64, align: u64 },
    // #[error("truncated record at byte {0}")]
    TruncatedRecord(u64),
    // #[error("names string not NUL-terminated at offset {0}")]
    UnterminatedName(u64),
    // #[error("counter slice out of range (offset {offset}, len {len})")]
    CounterSliceOOB { offset: u64, len: u64 },
    // #[error("value profile parse error: {0}")]
    ValueProfile(&'static str),
    // #[error("binary mismatch: counters schema differs (num_counters)")]
    CountersSchemaMismatch,
    // #[error("internal: {0}")]
    Internal(&'static str),
}

// -------------------------------------------------------------------------
// Demangling (optional)
// -------------------------------------------------------------------------

/// Simple demangling dispatcher you can call while building `FunctionRecord`.
pub fn maybe_demangle(mangled: &str) -> Option<String> {
    // Rust
    if mangled.starts_with("_R") || mangled.starts_with("R") {
        #[cfg(feature = "profiling")]
        if let Ok(d) = rustc_demangle::try_demangle(mangled) {
            return Some(d.to_string());
        }
    }

    None
}


fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: profraw-reader <path-to-profraw-file>");
        std::process::exit(1);
    }

    let profraw_path = &args[1];
    // Read and process the .profraw file
}
