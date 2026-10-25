//! Stack maps: which frame slots hold live heap references at a safe-point.

use std::fmt;

/// A set of register numbers (a frame has at most 256 registers).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct BitSet {
    words: [u64; 4],
}

impl BitSet {
    pub fn new() -> BitSet {
        BitSet::default()
    }

    pub fn insert(&mut self, i: u8) {
        self.words[(i >> 6) as usize] |= 1 << (i & 63);
    }

    pub fn remove(&mut self, i: u8) {
        self.words[(i >> 6) as usize] &= !(1 << (i & 63));
    }

    pub fn contains(&self, i: u8) -> bool {
        self.words[(i >> 6) as usize] & (1 << (i & 63)) != 0
    }

    pub fn is_empty(&self) -> bool {
        self.words == [0; 4]
    }

    pub fn count(&self) -> u32 {
        self.words.iter().map(|w| w.count_ones()).sum()
    }

    /// Adds every element of `other`; returns whether anything changed.
    pub fn union_with(&mut self, other: &BitSet) -> bool {
        let mut changed = false;
        for (a, b) in self.words.iter_mut().zip(other.words) {
            let n = *a | b;
            changed |= n != *a;
            *a = n;
        }
        changed
    }

    pub fn intersect_with(&mut self, other: &BitSet) {
        for (a, b) in self.words.iter_mut().zip(other.words) {
            *a &= b;
        }
    }

    pub fn subtract(&mut self, other: &BitSet) {
        for (a, b) in self.words.iter_mut().zip(other.words) {
            *a &= !b;
        }
    }

    /// Removes every element `>= from`.
    pub fn remove_from(&mut self, from: u16) {
        for r in from..256 {
            self.remove(r as u8);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = u8> + '_ {
        (0..=255u8).filter(move |&i| self.contains(i))
    }
}

impl fmt::Debug for BitSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

impl FromIterator<u8> for BitSet {
    fn from_iter<T: IntoIterator<Item = u8>>(iter: T) -> Self {
        let mut s = BitSet::new();
        for i in iter {
            s.insert(i);
        }
        s
    }
}

/// The registers that hold live references when execution is at instruction
/// `pc` (before it runs). Only these are traced by the collector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackMap {
    pub pc: u32,
    pub live_roots: BitSet,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_operations() {
        let mut s = BitSet::new();
        assert!(s.is_empty());
        for i in [0u8, 63, 64, 127, 128, 255] {
            s.insert(i);
        }
        assert_eq!(s.count(), 6);
        assert!(s.contains(255) && s.contains(64) && !s.contains(1));
        s.remove(64);
        assert!(!s.contains(64));
        assert_eq!(s.iter().collect::<Vec<_>>(), [0, 63, 127, 128, 255]);
    }

    #[test]
    fn union_intersect_subtract() {
        let a: BitSet = [1u8, 2, 3, 200].into_iter().collect();
        let b: BitSet = [3u8, 4, 200].into_iter().collect();
        let mut u = a;
        assert!(u.union_with(&b));
        assert!(!u.union_with(&b));
        assert_eq!(u.iter().collect::<Vec<_>>(), [1, 2, 3, 4, 200]);
        let mut i = a;
        i.intersect_with(&b);
        assert_eq!(i.iter().collect::<Vec<_>>(), [3, 200]);
        let mut d = a;
        d.subtract(&b);
        assert_eq!(d.iter().collect::<Vec<_>>(), [1, 2]);
    }

    #[test]
    fn remove_from_clears_a_suffix() {
        let mut s: BitSet = [1u8, 5, 6, 255].into_iter().collect();
        s.remove_from(6);
        assert_eq!(s.iter().collect::<Vec<_>>(), [1, 5]);
        s.remove_from(0);
        assert!(s.is_empty());
    }
}
