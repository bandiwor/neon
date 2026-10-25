//! A small, fast, deterministic hasher for the compiler's internal tables.
//!
//! `std`'s default `SipHash` is built to resist hash flooding, which tables keyed by node ids,
//! symbols and registers do not need; it costs several times more per lookup than this
//! multiply-and-rotate scheme (the one rustc uses). Being deterministic, it also makes iteration
//! order the same on every run.
//!
//! Do not use it for tables keyed by untrusted data that an attacker could choose.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

#[derive(Debug, Default, Clone, Copy)]
pub struct FxHasher {
    hash: u64,
}

impl FxHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            self.add(u64::from_le_bytes(c.try_into().expect("8 bytes")));
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut buf = [0u8; 8];
            buf[..rest.len()].copy_from_slice(rest);
            // the length in the top byte keeps "a" and "a\0" apart
            self.add(u64::from_le_bytes(buf) ^ ((rest.len() as u64) << 56));
        }
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(i as u64);
    }

    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add(i as u64);
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(i as u64);
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }

    /// The multiplication leaves the best bits at the top; tables index with the low ones.
    #[inline]
    fn finish(&self) -> u64 {
        self.hash.rotate_left(26)
    }
}

pub type FxBuildHasher = BuildHasherDefault<FxHasher>;
pub type FxHashMap<K, V> = HashMap<K, V, FxBuildHasher>;
pub type FxHashSet<T> = HashSet<T, FxBuildHasher>;

/// Hashes a 64-bit word into a well-mixed 64-bit value (for the run-time maps).
#[inline]
pub fn mix64(mut x: u64) -> u64 {
    x ^= x >> 32;
    x = x.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 29;
    x
}

/// Hashes the bytes of a string with the same quality.
#[inline]
pub fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = FxHasher::default();
    h.write(bytes);
    mix64(h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::Hash;

    fn h<T: Hash>(v: T) -> u64 {
        let mut s = FxHasher::default();
        v.hash(&mut s);
        s.finish()
    }

    #[test]
    fn deterministic_and_discriminating() {
        assert_eq!(h("hello"), h("hello"));
        assert_ne!(h("hello"), h("hellp"));
        assert_ne!(h(1u32), h(2u32));
        assert_ne!(h((1u32, 2u32)), h((2u32, 1u32)));
        // trailing zero bytes are not ignored
        let mut a = FxHasher::default();
        a.write(b"a");
        let mut b = FxHasher::default();
        b.write(b"a\0");
        assert_ne!(a.finish(), b.finish());
    }

    #[test]
    fn works_as_a_map_hasher() {
        let mut m: FxHashMap<u32, &str> = FxHashMap::default();
        for i in 0..1000 {
            m.insert(i, "x");
        }
        assert_eq!(m.len(), 1000);
        assert!(m.get(&999).is_some() && m.get(&1000).is_none());
        let mut s: FxHashSet<String> = FxHashSet::default();
        assert!(s.insert("a".into()) && !s.insert("a".into()));
    }

    #[test]
    fn mixed_words_spread_over_low_bits() {
        // sequential keys must not collide in a small table
        let mut seen = std::collections::HashSet::new();
        for i in 0..64u64 {
            seen.insert(mix64(i) & 255);
        }
        assert!(seen.len() > 40, "{}", seen.len());
    }
}
