//! Reader for LLVM's coverage mapping: which source regions each profile
//! counter (or expression over counters) stands for.
//!
//! The compiler writes two sections into the binary: `__llvm_covmap` holds
//! one filenames table per compilation unit and `__llvm_covfun` one record
//! per function with its file ids, counter expressions and regions. They are
//! read from the running executable with the `object` crate because they are
//! not loaded into memory on ELF. Format versions 4 to 7 are understood.

use std::{
    collections::HashMap,
    sync::OnceLock,
};

use object::{
    Object,
    ObjectSection,
};

/// A counter reference as stored in a region or an expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Counter {
    /// Always zero.
    Zero,
    /// The function's counter with this index.
    Ref(u32),
    /// The sum of the expression with this index.
    Add(u32),
    /// The difference of the expression with this index.
    Sub(u32),
}

/// What a region stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Code with a count.
    Code,
    /// A branch with a true count and a false count (nightly
    /// `-Z coverage-options=branch`).
    Branch,
    /// A gap between statements; carries a count but is not code.
    Gap,
    /// Code the compiler skipped.
    Skipped,
    /// A macro expansion into another file.
    Expansion,
    /// An MC/DC decision.
    Decision,
}

/// One source region of a function. Columns are decoded but not used yet;
/// suspects are reported by line.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct Region {
    pub kind: Kind,
    /// Index into [`Mapping::files`].
    pub file: usize,
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
    /// The region's count (the true count of a branch).
    pub counter: Counter,
    /// The false count of a branch, else `Zero`.
    pub false_counter: Counter,
}

/// A function's mapping, keyed in [`Mapping`] by the profile name hash and
/// the structural hash shared with the profile data record.
#[derive(Clone, Debug)]
pub(crate) struct MappedFunction {
    /// Expression operands; whether an expression adds or subtracts is
    /// carried by the [`Counter`] that refers to it.
    pub expressions: Vec<(Counter, Counter)>,
    pub regions: Vec<Region>,
}

impl MappedFunction {
    /// Evaluate a counter over the function's counter values.
    pub fn eval(&self, counter: Counter, values: &[u64]) -> u64 {
        self.eval_at(counter, values, 0)
    }

    fn eval_at(&self, counter: Counter, values: &[u64], depth: usize) -> u64 {
        if depth > 4096 {
            return 0;
        }
        match counter {
            Counter::Zero => 0,
            Counter::Ref(i) => values.get(i as usize).copied().unwrap_or(0),
            Counter::Add(i) => {
                match self.expressions.get(i as usize) {
                    Some(&(l, r)) => {
                        self.eval_at(l, values, depth + 1).saturating_add(self.eval_at(
                            r,
                            values,
                            depth + 1,
                        ))
                    },
                    None => 0,
                }
            },
            Counter::Sub(i) => {
                match self.expressions.get(i as usize) {
                    Some(&(l, r)) => {
                        self.eval_at(l, values, depth + 1).saturating_sub(self.eval_at(
                            r,
                            values,
                            depth + 1,
                        ))
                    },
                    None => 0,
                }
            },
        }
    }
}

/// The binary's coverage mapping.
#[derive(Debug, Default)]
pub(crate) struct Mapping {
    /// Every file name mentioned by any compilation unit.
    pub files: Vec<String>,
    functions: Vec<MappedFunction>,
    index: HashMap<(u64, u64), usize>,
}

impl Mapping {
    /// The running executable's mapping, read once; `None` when the binary
    /// carries no mapping sections or they cannot be parsed.
    pub fn get() -> Option<&'static Mapping> {
        static MAPPING: OnceLock<Option<Mapping>> = OnceLock::new();
        MAPPING.get_or_init(|| read_current_exe().ok()).as_ref()
    }

    /// The mapping of the function with these profile hashes.
    pub fn function(&self, name_ref: u64, func_hash: u64) -> Option<&MappedFunction> {
        self.index.get(&(name_ref, func_hash)).map(|&i| &self.functions[i])
    }
}

fn read_current_exe() -> Result<Mapping, String> {
    let path = std::env::current_exe().map_err(|e| e.to_string())?;
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let file = object::File::parse(&*bytes).map_err(|e| e.to_string())?;
    let mut covmap = None;
    let mut covfun = None;
    for section in file.sections() {
        let name = section.name().unwrap_or("");
        let is_map = name.ends_with("llvm_covmap") || name.starts_with(".lcovmap");
        let is_fun = name.ends_with("llvm_covfun") || name.starts_with(".lcovfun");
        if is_map || is_fun {
            let data = section.data().map_err(|e| e.to_string())?.to_vec();
            if is_map {
                covmap = Some(data);
            } else {
                covfun = Some(data);
            }
        }
    }
    match (covmap, covfun) {
        (Some(map), Some(fun)) => parse(&map, &fun),
        _ => Err("no coverage mapping sections".to_string()),
    }
}

/// Parse the two sections.
pub(crate) fn parse(covmap: &[u8], covfun: &[u8]) -> Result<Mapping, String> {
    let mut mapping = Mapping::default();
    // filenames_ref -> indices into mapping.files
    let mut tables: HashMap<u64, Vec<usize>> = HashMap::new();
    let mut pos = 0;
    while pos + 16 <= covmap.len() {
        let filenames_size = u32_at(covmap, pos + 4)? as usize;
        let version = u32_at(covmap, pos + 12)?;
        if version < 3 {
            return Err(format!("coverage mapping version {} is not supported", version + 1));
        }
        let blob =
            covmap.get(pos + 16..pos + 16 + filenames_size).ok_or("truncated filenames table")?;
        let digest = super::names::md5(blob);
        let filenames_ref = u64::from_le_bytes(digest[0..8].try_into().expect("8 bytes"));
        let names = decode_filenames(blob, version)?;
        let base = mapping.files.len();
        mapping.files.extend(names);
        tables.insert(filenames_ref, (base..mapping.files.len()).collect());
        pos = align8(pos + 16 + filenames_size);
    }
    let mut pos = 0;
    while pos + 28 <= covfun.len() {
        let name_ref = u64_at(covfun, pos)?;
        let data_len = u32_at(covfun, pos + 8)? as usize;
        let func_hash = u64_at(covfun, pos + 12)?;
        let filenames_ref = u64_at(covfun, pos + 20)?;
        let data = covfun.get(pos + 28..pos + 28 + data_len).ok_or("truncated function record")?;
        pos = align8(pos + 28 + data_len);
        let Some(table) = tables.get(&filenames_ref) else {
            continue;
        };
        let function = decode_function(data, table)?;
        mapping.index.entry((name_ref, func_hash)).or_insert(mapping.functions.len());
        mapping.functions.push(function);
    }
    Ok(mapping)
}

fn decode_filenames(blob: &[u8], version: u32) -> Result<Vec<String>, String> {
    let (count, pos) = uleb(blob, 0)?;
    let (uncompressed_len, pos) = uleb(blob, pos)?;
    let (compressed_len, pos) = uleb(blob, pos)?;
    let raw: Vec<u8> = if compressed_len > 0 {
        let packed = blob.get(pos..pos + compressed_len as usize).ok_or("truncated filenames")?;
        miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(packed, uncompressed_len as usize)
            .map_err(|e| format!("filenames inflate: {e:?}"))?
    } else {
        blob.get(pos..).ok_or("truncated filenames")?.to_vec()
    };
    let mut names = Vec::with_capacity(count as usize);
    let mut at = 0;
    for _ in 0..count {
        let (len, next) = uleb(&raw, at)?;
        let bytes = raw.get(next..next + len as usize).ok_or("truncated file name")?;
        names.push(String::from_utf8_lossy(bytes).into_owned());
        at = next + len as usize;
    }
    // Version 6 and later put the compilation directory first and store
    // the other names relative to it when they are not absolute.
    if version >= 5 && !names.is_empty() {
        let dir = names[0].clone();
        for name in names.iter_mut().skip(1) {
            let absolute = name.starts_with('/')
                || name.starts_with('\\')
                || name.as_bytes().get(1) == Some(&b':');
            if !absolute && !dir.is_empty() {
                *name = format!("{}/{}", dir.trim_end_matches(['/', '\\']), name);
            }
        }
    }
    Ok(names)
}

fn decode_counter(encoded: u64) -> Result<Counter, String> {
    let id = u32::try_from(encoded >> 2).map_err(|_| "counter id out of range")?;
    Ok(match encoded & 3 {
        0 => Counter::Zero,
        1 => Counter::Ref(id),
        2 => Counter::Sub(id),
        _ => Counter::Add(id),
    })
}

fn decode_function(data: &[u8], table: &[usize]) -> Result<MappedFunction, String> {
    let (file_count, mut pos) = uleb(data, 0)?;
    let mut files = Vec::with_capacity(file_count as usize);
    for _ in 0..file_count {
        let (index, next) = uleb(data, pos)?;
        pos = next;
        files.push(*table.get(index as usize).ok_or("file id out of range")?);
    }
    let (expression_count, next) = uleb(data, pos)?;
    pos = next;
    let mut expressions = Vec::with_capacity(expression_count as usize);
    for _ in 0..expression_count {
        let (lhs, next) = uleb(data, pos)?;
        let (rhs, next) = uleb(data, next)?;
        pos = next;
        expressions.push((decode_counter(lhs)?, decode_counter(rhs)?));
    }
    let mut regions = Vec::new();
    for &file in &files {
        let (region_count, next) = uleb(data, pos)?;
        pos = next;
        let mut line = 0u64;
        for _ in 0..region_count {
            let (encoded, next) = uleb(data, pos)?;
            pos = next;
            let (mut kind, counter, false_counter) = if encoded & 3 != 0 {
                (Kind::Code, decode_counter(encoded)?, Counter::Zero)
            } else if encoded & 4 != 0 {
                (Kind::Expansion, Counter::Zero, Counter::Zero)
            } else {
                match encoded >> 3 {
                    0 => (Kind::Code, Counter::Zero, Counter::Zero),
                    2 => (Kind::Skipped, Counter::Zero, Counter::Zero),
                    4 | 6 => {
                        let (c, next) = uleb(data, pos)?;
                        let (c2, next) = uleb(data, next)?;
                        pos = next;
                        if encoded >> 3 == 6 {
                            for _ in 0..3 {
                                let (_, next) = uleb(data, pos)?;
                                pos = next;
                            }
                        }
                        (Kind::Branch, decode_counter(c)?, decode_counter(c2)?)
                    },
                    5 => {
                        for _ in 0..2 {
                            let (_, next) = uleb(data, pos)?;
                            pos = next;
                        }
                        (Kind::Decision, Counter::Zero, Counter::Zero)
                    },
                    other => return Err(format!("unknown region kind {other}")),
                }
            };
            let (line_delta, next) = uleb(data, pos)?;
            let (start_col, next) = uleb(data, next)?;
            let (num_lines, next) = uleb(data, next)?;
            let (end_col, next) = uleb(data, next)?;
            pos = next;
            line += line_delta;
            let mut end_col = end_col as u32;
            if end_col & (1 << 31) != 0 {
                kind = Kind::Gap;
                end_col &= !(1 << 31);
            }
            let (start_col, end_col) = if start_col == 0 && end_col == 0 {
                (1, u32::MAX)
            } else {
                (start_col as u32, end_col)
            };
            regions.push(Region {
                kind,
                file,
                start_line: line as u32,
                start_col,
                end_line: (line + num_lines) as u32,
                end_col,
                counter,
                false_counter,
            });
        }
    }
    Ok(MappedFunction { expressions, regions })
}

fn uleb(bytes: &[u8], mut pos: usize) -> Result<(u64, usize), String> {
    let mut value = 0u64;
    let mut shift = 0;
    loop {
        let byte = *bytes.get(pos).ok_or("truncated integer")?;
        pos += 1;
        if shift < 64 {
            value |= u64::from(byte & 0x7f) << shift;
        }
        if byte & 0x80 == 0 {
            return Ok((value, pos));
        }
        shift += 7;
    }
}

fn u32_at(bytes: &[u8], pos: usize) -> Result<u32, String> {
    let slice = bytes.get(pos..pos + 4).ok_or("truncated header")?;
    Ok(u32::from_le_bytes(slice.try_into().expect("4 bytes")))
}

fn u64_at(bytes: &[u8], pos: usize) -> Result<u64, String> {
    let slice = bytes.get(pos..pos + 8).ok_or("truncated header")?;
    Ok(u64::from_le_bytes(slice.try_into().expect("8 bytes")))
}

fn align8(pos: usize) -> usize {
    (pos + 7) & !7
}

#[cfg(test)]
#[allow(clippy::identity_op)] // `(id << 2) | tag` spells out the encoding
mod tests {
    use super::*;

    fn put_uleb(out: &mut Vec<u8>, mut v: u64) {
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(byte);
                return;
            }
            out.push(byte | 0x80);
        }
    }

    fn filenames_blob(names: &[&str]) -> Vec<u8> {
        let mut raw = Vec::new();
        for name in names {
            put_uleb(&mut raw, name.len() as u64);
            raw.extend_from_slice(name.as_bytes());
        }
        let mut blob = Vec::new();
        put_uleb(&mut blob, names.len() as u64);
        put_uleb(&mut blob, raw.len() as u64);
        put_uleb(&mut blob, 0);
        blob.extend(raw);
        blob
    }

    /// One compilation unit (version 7, working dir + one file) and one
    /// function with counters c0, c1, the expression c0 - c1, a code region
    /// on c0, a code region on the expression and a branch (c1, expr).
    fn sample() -> (Vec<u8>, Vec<u8>) {
        let blob = filenames_blob(&["/work", "src/lib.rs"]);
        let mut covmap = Vec::new();
        covmap.extend_from_slice(&0u32.to_le_bytes());
        covmap.extend_from_slice(&(blob.len() as u32).to_le_bytes());
        covmap.extend_from_slice(&0u32.to_le_bytes());
        covmap.extend_from_slice(&6u32.to_le_bytes());
        covmap.extend(&blob);
        let digest = super::super::names::md5(&blob);
        let filenames_ref = u64::from_le_bytes(digest[0..8].try_into().unwrap());

        let mut data = Vec::new();
        put_uleb(&mut data, 1); // one file
        put_uleb(&mut data, 1); // file id 1 = src/lib.rs
        put_uleb(&mut data, 1); // one expression
        put_uleb(&mut data, (0 << 2) | 1); // c0
        put_uleb(&mut data, (1 << 2) | 1); // c1
        put_uleb(&mut data, 3); // three regions in the file
        // code region on c0: lines 10..12, cols 1..2
        put_uleb(&mut data, (0 << 2) | 1);
        put_uleb(&mut data, 10);
        put_uleb(&mut data, 1);
        put_uleb(&mut data, 2);
        put_uleb(&mut data, 2);
        // code region on expr 0 (subtract): line 11 (delta 1), whole line
        put_uleb(&mut data, (0 << 2) | 2);
        put_uleb(&mut data, 1);
        put_uleb(&mut data, 0);
        put_uleb(&mut data, 0);
        put_uleb(&mut data, 0);
        // branch region (true c1, false expr0) at line 11 (delta 0), gap bit set on end col
        put_uleb(&mut data, 4 << 3);
        put_uleb(&mut data, (1 << 2) | 1);
        put_uleb(&mut data, (0 << 2) | 2);
        put_uleb(&mut data, 0);
        put_uleb(&mut data, 5);
        put_uleb(&mut data, 0);
        put_uleb(&mut data, 9 | (1 << 31));

        let mut covfun = Vec::new();
        covfun.extend_from_slice(&0x1111u64.to_le_bytes());
        covfun.extend_from_slice(&(data.len() as u32).to_le_bytes());
        covfun.extend_from_slice(&0x2222u64.to_le_bytes());
        covfun.extend_from_slice(&filenames_ref.to_le_bytes());
        covfun.extend(&data);
        while covfun.len() % 8 != 0 {
            covfun.push(0);
        }
        (covmap, covfun)
    }

    #[test]
    fn parses_files_functions_and_regions() {
        let (covmap, covfun) = sample();
        let mapping = parse(&covmap, &covfun).unwrap();
        assert_eq!(mapping.files, ["/work", "/work/src/lib.rs"]);
        assert_eq!(mapping.functions.len(), 1);
        let f = mapping.function(0x1111, 0x2222).expect("joined by hashes");
        assert!(mapping.function(0x1111, 0x2223).is_none());
        assert_eq!(f.expressions, [(Counter::Ref(0), Counter::Ref(1))]);
        assert_eq!(f.regions.len(), 3);
        let r = &f.regions[0];
        assert_eq!(
            (r.kind, r.file, r.start_line, r.start_col, r.end_line, r.end_col),
            (Kind::Code, 1, 10, 1, 12, 2)
        );
        assert_eq!(r.counter, Counter::Ref(0));
        let r = &f.regions[1];
        assert_eq!(
            (r.kind, r.start_line, r.start_col, r.end_line, r.end_col),
            (Kind::Code, 11, 1, 11, u32::MAX)
        );
        assert_eq!(r.counter, Counter::Sub(0));
        let r = &f.regions[2];
        assert_eq!((r.kind, r.start_line, r.start_col, r.end_col), (Kind::Gap, 11, 5, 9));
        assert_eq!((r.counter, r.false_counter), (Counter::Ref(1), Counter::Sub(0)));

        let values = [5, 2];
        assert_eq!(f.eval(f.regions[0].counter, &values), 5);
        assert_eq!(f.eval(f.regions[1].counter, &values), 3);
        assert_eq!(f.eval(f.regions[2].counter, &values), 2);
        assert_eq!(f.eval(Counter::Add(0), &values), 7);
        assert_eq!(f.eval(Counter::Sub(0), &[1, 4]), 0, "differences saturate");
    }

    #[test]
    fn rejects_old_versions_and_truncation() {
        let (mut covmap, covfun) = sample();
        covmap[12..16].copy_from_slice(&1u32.to_le_bytes());
        assert!(parse(&covmap, &covfun).unwrap_err().contains("version 2"));
        let (covmap, covfun) = sample();
        assert!(parse(&covmap, &covfun[..covfun.len() - 9]).is_err());
    }

    #[cfg(crabcheck_instrumented)]
    #[test]
    fn the_test_binary_maps_its_instrumented_functions() {
        let mapping = Mapping::get().expect("instrumented binaries carry a mapping");
        let functions = super::super::Functions::get();
        let joined = functions
            .all()
            .iter()
            .filter(|f| mapping.function(f.name_ref, f.func_hash).is_some())
            .count();
        assert!(
            joined * 10 >= functions.all().len() * 9,
            "{joined} of {} joined",
            functions.all().len()
        );
        let this = functions
            .all()
            .iter()
            .find(|f| f.name.contains("mapping::parse"))
            .expect("this module is instrumented");
        let mapped = mapping.function(this.name_ref, this.func_hash).unwrap();
        assert!(
            mapped.regions.iter().any(|r| mapping.files[r.file].ends_with("coverage/mapping.rs"))
        );
    }
}
