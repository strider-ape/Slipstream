//! Deterministic file content. Every byte is computed from its offset and a seed,
//! so the server can serve files of any size (1 GiB, 5 GiB...) without touching the
//! disk, any range is cheap to produce, and tests can rebuild the expected bytes.

/// SplitMix64: a tiny, well-known mixing function. Good enough to make the bytes
/// look random, so a bug that writes a range at the wrong offset can't go unnoticed.
pub fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// FNV-1a hash, used to turn names and request bodies into seeds.
pub fn fnv1a(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Seed of the file `name`: an explicit `?seed=` wins, otherwise derived from the name.
/// `version` goes up when a scenario "changes the file on the server".
pub fn file_seed(name: &str, seed: Option<u64>, version: u64) -> u64 {
    seed.unwrap_or_else(|| fnv1a(name.as_bytes()))
        .wrapping_add(version)
}

/// Fill `buf` with the file's bytes starting at `offset`.
pub fn fill(seed: u64, offset: u64, buf: &mut [u8]) {
    let key = splitmix64(seed);
    let mut pos = offset;
    let mut i = 0;
    while i < buf.len() {
        // Bytes come in 8-byte blocks; we may start in the middle of one.
        let block = splitmix64(key ^ (pos / 8)).to_le_bytes();
        let skip = (pos % 8) as usize;
        let n = (8 - skip).min(buf.len() - i);
        buf[i..i + n].copy_from_slice(&block[skip..skip + n]);
        i += n;
        pos += n as u64;
    }
}

/// `len` bytes of the file starting at `offset`.
pub fn bytes(seed: u64, offset: u64, len: usize) -> Vec<u8> {
    let mut buf = vec![0; len];
    fill(seed, offset, &mut buf);
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_slice_matches_the_whole_file() {
        let whole = bytes(42, 0, 1000);
        for (start, len) in [(0, 1), (3, 5), (7, 9), (8, 8), (123, 456), (999, 1)] {
            assert_eq!(bytes(42, start as u64, len), whole[start..start + len]);
        }
    }

    #[test]
    fn seeds_and_versions_give_different_files() {
        assert_ne!(bytes(1, 0, 64), bytes(2, 0, 64));
        assert_ne!(file_seed("a.bin", None, 0), file_seed("b.bin", None, 0));
        assert_ne!(file_seed("a.bin", None, 0), file_seed("a.bin", None, 1));
        assert_eq!(file_seed("a.bin", Some(7), 0), 7);
    }
}
