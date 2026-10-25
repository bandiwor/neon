use std::ops::Range;

/// Identifies a source registered in a [`SourceMap`](super::SourceMap).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileId(pub u32);

impl FileId {
    /// Placeholder for synthesized nodes with no source location.
    pub const DUMMY: FileId = FileId(u32::MAX);
}

/// A half-open byte range `[lo, hi)` within one source file.
///
/// Spans are 12 bytes and `Copy`, so AST nodes, tokens and symbols can all
/// carry one; line/column data is derived on demand via the `SourceMap`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub file: FileId,
    pub lo: u32,
    pub hi: u32,
}

impl Span {
    /// A span with no location.
    pub const DUMMY: Span = Span { file: FileId::DUMMY, lo: 0, hi: 0 };

    /// Creates a span; swapped endpoints are reordered.
    pub fn new(file: FileId, lo: u32, hi: u32) -> Span {
        Span { file, lo: lo.min(hi), hi: lo.max(hi) }
    }

    pub fn is_dummy(&self) -> bool {
        self.file == FileId::DUMMY
    }

    pub fn len(&self) -> u32 {
        self.hi - self.lo
    }

    pub fn is_empty(&self) -> bool {
        self.lo == self.hi
    }

    /// Whether the byte at `offset` lies inside the span.
    pub fn contains(&self, offset: u32) -> bool {
        self.lo <= offset && offset < self.hi
    }

    /// The smallest span covering both. If the spans come from different
    /// files (or one is a dummy) `self` is returned unchanged.
    pub fn to(self, other: Span) -> Span {
        if other.is_dummy() {
            return self;
        }
        if self.is_dummy() {
            return other;
        }
        if self.file != other.file {
            return self;
        }
        Span { file: self.file, lo: self.lo.min(other.lo), hi: self.hi.max(other.hi) }
    }

    /// Empty span at the start.
    pub fn shrink_to_lo(self) -> Span {
        Span { hi: self.lo, ..self }
    }

    /// Empty span at the end.
    pub fn shrink_to_hi(self) -> Span {
        Span { lo: self.hi, ..self }
    }

    pub fn range(&self) -> Range<usize> {
        self.lo as usize..self.hi as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const F: FileId = FileId(0);

    #[test]
    fn construction_orders_endpoints() {
        let s = Span::new(F, 9, 4);
        assert_eq!((s.lo, s.hi, s.len()), (4, 9, 5));
        assert!(!s.is_empty());
        assert!(Span::new(F, 3, 3).is_empty());
    }

    #[test]
    fn contains_is_half_open() {
        let s = Span::new(F, 2, 5);
        assert!(!s.contains(1) && s.contains(2) && s.contains(4) && !s.contains(5));
    }

    #[test]
    fn merging() {
        let a = Span::new(F, 2, 5);
        let b = Span::new(F, 8, 10);
        assert_eq!(a.to(b), Span::new(F, 2, 10));
        assert_eq!(b.to(a), Span::new(F, 2, 10));
        let other_file = Span::new(FileId(1), 0, 1);
        assert_eq!(a.to(other_file), a);
        assert_eq!(a.to(Span::DUMMY), a);
        assert_eq!(Span::DUMMY.to(a), a);
    }

    #[test]
    fn shrinking_and_range() {
        let s = Span::new(F, 2, 5);
        assert_eq!(s.shrink_to_lo(), Span::new(F, 2, 2));
        assert_eq!(s.shrink_to_hi(), Span::new(F, 5, 5));
        assert_eq!(s.range(), 2..5);
        assert!(Span::DUMMY.is_dummy() && !s.is_dummy());
    }
}
