//! The profile names section: mangled function names keyed by the MD5-based
//! `NameRef` the data records carry.

use std::collections::HashMap;

/// Parse the `__llvm_prf_names` blob: a sequence of chunks, each
/// `uleb128 uncompressed_size, uleb128 compressed_size, bytes`, where a
/// compressed size of zero means the bytes are plain and otherwise they are
/// zlib-compressed. Names within a chunk are separated by `\x01`.
pub(crate) fn parse(blob: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let mut pos = 0;
    while pos < blob.len() {
        let Some((uncompressed, next)) = uleb128(blob, pos) else { break };
        let Some((compressed, next)) = uleb128(blob, next) else { break };
        let (chunk, after) = if compressed == 0 {
            let end = (next + uncompressed as usize).min(blob.len());
            (blob[next..end].to_vec(), end)
        } else {
            let end = (next + compressed as usize).min(blob.len());
            let inflated =
                miniz_oxide::inflate::decompress_to_vec_zlib(&blob[next..end]).unwrap_or_default();
            (inflated, end)
        };
        for name in chunk.split(|b| *b == 1).filter(|n| !n.is_empty()) {
            names.push(String::from_utf8_lossy(name).into_owned());
        }
        pos = after;
    }
    names
}

fn uleb128(bytes: &[u8], mut pos: usize) -> Option<(u64, usize)> {
    let mut value = 0u64;
    let mut shift = 0;
    loop {
        let b = *bytes.get(pos)?;
        pos += 1;
        value |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Some((value, pos));
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
}

/// Map `NameRef` (the low 64 bits of the MD5 of the name) to the name.
pub(crate) fn by_ref(names: &[String]) -> HashMap<u64, &str> {
    names.iter().map(|n| (name_ref(n), n.as_str())).collect()
}

/// LLVM's `IndexedInstrProf::ComputeHash`: the first eight bytes of the MD5
/// digest as a little-endian integer.
pub(crate) fn name_ref(name: &str) -> u64 {
    let digest = md5(name.as_bytes());
    u64::from_le_bytes(digest[..8].try_into().expect("8 bytes"))
}

/// A demangled, prefix-stripped version of a PGO function name. Local
/// functions carry a `<file>:` prefix before the mangled symbol.
pub(crate) fn pretty(name: &str) -> String {
    let symbol = match name.find(":_") {
        Some(i) if name[i + 1..].starts_with("_R") || name[i + 1..].starts_with("_ZN") => {
            &name[i + 1..]
        },
        _ => name,
    };
    format!("{:#}", rustc_demangle::demangle(symbol))
}

/// RFC 1321 MD5, needed only to recover names from their hashes.
pub(crate) fn md5(input: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> =
        (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4_294_967_296.0) as u32).collect();
    let (mut a0, mut b0, mut c0, mut d0) =
        (0x6745_2301u32, 0xefcd_ab89u32, 0x98ba_dcfeu32, 0x1032_5476u32);
    let mut message = input.to_vec();
    let bit_len = (input.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_le_bytes());
    for chunk in message.chunks(64) {
        let m: Vec<u32> =
            chunk.chunks(4).map(|w| u32::from_le_bytes(w.try_into().expect("4 bytes"))).collect();
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    out[..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..].copy_from_slice(&d0.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(d: [u8; 16]) -> String {
        d.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn md5_matches_reference_vectors() {
        assert_eq!(hex(md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex(md5(b"abc")), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            hex(md5(b"The quick brown fox jumps over the lazy dog")),
            "9e107d9d372bb6826bd81d3542a419d6"
        );
        let long = "a".repeat(200);
        assert_eq!(hex(md5(long.as_bytes())), "887f30b43b2867f4a9accceee7d16e6c");
    }

    #[test]
    fn names_blob_round_trip_uncompressed() {
        let payload = b"main\x01foo::bar";
        let mut blob = vec![payload.len() as u8, 0];
        blob.extend_from_slice(payload);
        let names = parse(&blob);
        assert_eq!(names, vec!["main", "foo::bar"]);
        let map = by_ref(&names);
        assert_eq!(map[&name_ref("main")], "main");
    }

    #[test]
    fn names_blob_round_trip_compressed() {
        let payload = b"_ZN3foo3barE\x01baz";
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(payload, 6);
        let mut blob = vec![payload.len() as u8, compressed.len() as u8];
        blob.extend_from_slice(&compressed);
        assert_eq!(parse(&blob), vec!["_ZN3foo3barE", "baz"]);
        assert_eq!(pretty("src/lib.rs:_ZN3foo3barE"), "foo::bar");
    }
}
