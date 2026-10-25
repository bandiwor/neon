use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::io;
use std::ops::Range;
use std::sync::OnceLock;

use super::span::{FileId, Span};
use crate::vfs::Vfs;

/// A 0-based line and a 0-based **byte** column within that line.
///
/// Byte columns are what the lexer works with natively; use
/// [`SourceFile::char_col`] / [`SourceFile::utf16_col`] to convert for
/// display or for LSP clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct LineCol {
    pub line: u32,
    pub col: u32,
}

/// A [`Span`] converted to line/column endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedSpan {
    pub file: FileId,
    pub start: LineCol,
    pub end: LineCol,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceMapError {
    /// Offsets are `u32`, so a single source is limited to 4 GiB.
    FileTooLarge { name: String, len: usize },
}

impl fmt::Display for SourceMapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceMapError::FileTooLarge { name, len } => {
                write!(f, "source `{name}` is too large ({len} bytes; the limit is 4 GiB)")
            }
        }
    }
}

impl Error for SourceMapError {}

/// One registered source. Line starts are computed once, on first query, and
/// every later lookup is a binary search.
#[derive(Debug)]
pub struct SourceFile {
    name: String,
    text: String,
    line_starts: OnceLock<Vec<u32>>,
}

impl SourceFile {
    fn new(name: String, text: String) -> Result<SourceFile, SourceMapError> {
        if text.len() > u32::MAX as usize {
            return Err(SourceMapError::FileTooLarge { name, len: text.len() });
        }
        Ok(SourceFile { name, text, line_starts: OnceLock::new() })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Length in bytes.
    pub fn len(&self) -> u32 {
        self.text.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn line_starts(&self) -> &[u32] {
        self.line_starts.get_or_init(|| {
            let mut starts = vec![0u32];
            for (i, b) in self.text.bytes().enumerate() {
                if b == b'\n' {
                    starts.push(i as u32 + 1);
                }
            }
            starts
        })
    }

    /// Number of lines. A trailing newline starts a final empty line, which
    /// matches how editors and LSP count.
    pub fn line_count(&self) -> u32 {
        self.line_starts().len() as u32
    }

    /// The 0-based line containing `offset` (clamped to the end of file).
    pub fn line_index(&self, offset: u32) -> u32 {
        let offset = offset.min(self.len());
        (self.line_starts().partition_point(|&s| s <= offset) - 1) as u32
    }

    /// Converts a byte offset (clamped to the end of file) to line/column.
    pub fn line_col(&self, offset: u32) -> LineCol {
        let offset = offset.min(self.len());
        let line = self.line_index(offset);
        LineCol { line, col: offset - self.line_starts()[line as usize] }
    }

    /// Byte range of `line`, excluding its `\n` or `\r\n` terminator.
    pub fn line_range(&self, line: u32) -> Option<Range<u32>> {
        let starts = self.line_starts();
        let start = *starts.get(line as usize)?;
        let mut end = starts.get(line as usize + 1).copied().unwrap_or(self.len());
        let bytes = self.text.as_bytes();
        if end > start && bytes[end as usize - 1] == b'\n' {
            end -= 1;
            if end > start && bytes[end as usize - 1] == b'\r' {
                end -= 1;
            }
        }
        Some(start..end)
    }

    /// Text of `line` without its terminator.
    pub fn line_text(&self, line: u32) -> Option<&str> {
        let r = self.line_range(line)?;
        Some(&self.text[r.start as usize..r.end as usize])
    }

    /// Converts a line/column back to a byte offset. A column past the end of
    /// the line is clamped to the end of the line (the LSP convention).
    pub fn offset(&self, lc: LineCol) -> Option<u32> {
        let r = self.line_range(lc.line)?;
        Some((r.start + lc.col).min(r.end))
    }

    /// The column of `lc` counted in Unicode scalar values.
    pub fn char_col(&self, lc: LineCol) -> u32 {
        self.line_prefix(lc).chars().count() as u32
    }

    /// The column of `lc` counted in UTF-16 code units (LSP's default).
    pub fn utf16_col(&self, lc: LineCol) -> u32 {
        self.line_prefix(lc).chars().map(|c| c.len_utf16() as u32).sum()
    }

    pub(crate) fn line_prefix(&self, lc: LineCol) -> &str {
        let Some(line) = self.line_text(lc.line) else {
            return "";
        };
        let mut end = (lc.col as usize).min(line.len());
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        &line[..end]
    }

    /// The source text in `[lo, hi)`, or `None` if out of range or not on
    /// character boundaries.
    pub fn slice(&self, lo: u32, hi: u32) -> Option<&str> {
        self.text.get(lo as usize..hi as usize)
    }
}

/// Registry of every source known to a compilation. Hands out [`FileId`]s and
/// answers all position questions, so no other component stores line or
/// column numbers.
#[derive(Debug, Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
    by_name: HashMap<String, FileId>,
}

impl SourceMap {
    pub fn new() -> SourceMap {
        SourceMap::default()
    }

    /// Registers a source. If `name` was registered before, the name now
    /// refers to this newer source; the old id stays valid.
    pub fn add_file(&mut self, name: impl Into<String>, text: impl Into<String>) -> Result<FileId, SourceMapError> {
        let file = SourceFile::new(name.into(), text.into())?;
        let id = FileId(self.files.len() as u32);
        self.by_name.insert(file.name.clone(), id);
        self.files.push(file);
        Ok(id)
    }

    /// Reads `path` through `vfs` and registers it, or returns the existing
    /// id if a source of that name is already registered.
    pub fn load(&mut self, vfs: &dyn Vfs, path: &str) -> io::Result<FileId> {
        if let Some(&id) = self.by_name.get(path) {
            return Ok(id);
        }
        let text = vfs.read_to_string(path)?;
        self.add_file(path, text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// Replaces a source's text (an editor edit), invalidating only its line
    /// index. Existing spans into the file are the caller's to refresh.
    pub fn replace_text(&mut self, id: FileId, text: impl Into<String>) -> Result<(), SourceMapError> {
        let old = self.files.get(id.0 as usize).expect("replace_text: unknown FileId");
        let fresh = SourceFile::new(old.name.clone(), text.into())?;
        self.files[id.0 as usize] = fresh;
        Ok(())
    }

    pub fn get(&self, id: FileId) -> Option<&SourceFile> {
        self.files.get(id.0 as usize)
    }

    /// Like [`get`](Self::get) but panics on an id that did not come from
    /// this map.
    pub fn file(&self, id: FileId) -> &SourceFile {
        self.get(id).expect("unknown FileId")
    }

    pub fn id_by_name(&self, name: &str) -> Option<FileId> {
        self.by_name.get(name).copied()
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (FileId, &SourceFile)> {
        self.files.iter().enumerate().map(|(i, f)| (FileId(i as u32), f))
    }

    /// Line/column of `offset` in `id`.
    pub fn line_col(&self, id: FileId, offset: u32) -> Option<LineCol> {
        Some(self.get(id)?.line_col(offset))
    }

    /// Line/column endpoints of `span`; `None` for dummy or unknown spans.
    pub fn resolve(&self, span: Span) -> Option<ResolvedSpan> {
        let file = self.get(span.file)?;
        Some(ResolvedSpan { file: span.file, start: file.line_col(span.lo), end: file.line_col(span.hi) })
    }

    /// The text a span covers.
    pub fn snippet(&self, span: Span) -> Option<&str> {
        self.get(span.file)?.slice(span.lo, span.hi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::MemVfs;

    fn one(text: &str) -> (SourceMap, FileId) {
        let mut map = SourceMap::new();
        let id = map.add_file("t.neon", text).unwrap();
        (map, id)
    }

    #[test]
    fn offsets_to_line_col() {
        let (map, id) = one("ab\ncde\n\nf");
        let f = map.file(id);
        assert_eq!(f.line_count(), 4);
        let at = |o| f.line_col(o);
        assert_eq!(at(0), LineCol { line: 0, col: 0 });
        assert_eq!(at(2), LineCol { line: 0, col: 2 }); // the '\n' itself
        assert_eq!(at(3), LineCol { line: 1, col: 0 });
        assert_eq!(at(5), LineCol { line: 1, col: 2 });
        assert_eq!(at(7), LineCol { line: 2, col: 0 }); // empty line
        assert_eq!(at(8), LineCol { line: 3, col: 0 });
        assert_eq!(at(9), LineCol { line: 3, col: 1 }); // EOF
        assert_eq!(at(999), LineCol { line: 3, col: 1 }); // clamped
    }

    #[test]
    fn trailing_newline_starts_empty_line() {
        let (map, id) = one("a\n");
        let f = map.file(id);
        assert_eq!(f.line_count(), 2);
        assert_eq!(f.line_text(1), Some(""));
        assert_eq!(f.line_text(2), None);
        assert_eq!(f.line_col(2), LineCol { line: 1, col: 0 });
    }

    #[test]
    fn empty_source() {
        let (map, id) = one("");
        let f = map.file(id);
        assert!(f.is_empty());
        assert_eq!(f.line_count(), 1);
        assert_eq!(f.line_col(0), LineCol { line: 0, col: 0 });
        assert_eq!(f.line_text(0), Some(""));
    }

    #[test]
    fn line_text_strips_terminators() {
        let (map, id) = one("one\r\ntwo\nthree");
        let f = map.file(id);
        assert_eq!(f.line_text(0), Some("one"));
        assert_eq!(f.line_text(1), Some("two"));
        assert_eq!(f.line_text(2), Some("three"));
        assert_eq!(f.line_range(0), Some(0..3));
    }

    #[test]
    fn offset_is_inverse_of_line_col() {
        let (map, id) = one("ab\ncde\n\nf");
        let f = map.file(id);
        for o in 0..=f.len() {
            let lc = f.line_col(o);
            // The offset of a '\n' maps back to itself; all offsets round-trip.
            assert_eq!(f.offset(lc), Some(o), "offset {o} -> {lc:?}");
        }
        // Column past end of line clamps to the end of that line.
        assert_eq!(f.offset(LineCol { line: 0, col: 50 }), Some(2));
        assert_eq!(f.offset(LineCol { line: 9, col: 0 }), None);
    }

    #[test]
    fn multibyte_columns() {
        // 'é' is 2 bytes / 1 char / 1 UTF-16 unit; '😀' is 4 bytes / 1 char / 2 units.
        let (map, id) = one("é😀x");
        let f = map.file(id);
        let x = f.line_col(6);
        assert_eq!(x, LineCol { line: 0, col: 6 });
        assert_eq!(f.char_col(x), 2); // é, 😀 precede x
        assert_eq!(f.utf16_col(x), 3);
        // A byte column in the middle of a character floors to its start.
        assert_eq!(f.char_col(LineCol { line: 0, col: 1 }), 0);
        assert_eq!(f.char_col(LineCol { line: 0, col: 4 }), 1);
    }

    #[test]
    fn spans_resolve_and_slice() {
        let (map, id) = one("let x = 1;\nlet y = 2;");
        let span = Span::new(id, 15, 16);
        let r = map.resolve(span).unwrap();
        assert_eq!(r.start, LineCol { line: 1, col: 4 });
        assert_eq!(r.end, LineCol { line: 1, col: 5 });
        assert_eq!(map.snippet(span), Some("y"));
        assert_eq!(map.snippet(Span::DUMMY), None);
        assert!(map.resolve(Span::DUMMY).is_none());
        assert_eq!(map.snippet(Span::new(id, 0, 999)), None);
    }

    #[test]
    fn ids_names_and_iteration() {
        let mut map = SourceMap::new();
        assert!(map.is_empty());
        let a = map.add_file("a.neon", "1").unwrap();
        let b = map.add_file("b.neon", "2").unwrap();
        assert_eq!((a, b), (FileId(0), FileId(1)));
        assert_eq!(map.len(), 2);
        assert_eq!(map.id_by_name("b.neon"), Some(b));
        assert_eq!(map.id_by_name("c.neon"), None);
        assert_eq!(map.file(a).name(), "a.neon");
        assert!(map.get(FileId(7)).is_none());
        let names: Vec<_> = map.iter().map(|(_, f)| f.name().to_string()).collect();
        assert_eq!(names, ["a.neon", "b.neon"]);
    }

    #[test]
    fn load_goes_through_vfs_and_dedups() {
        let vfs = MemVfs::new().with_file("m.neon", "let a = 1;");
        let mut map = SourceMap::new();
        let id = map.load(&vfs, "m.neon").unwrap();
        assert_eq!(map.file(id).text(), "let a = 1;");
        assert_eq!(map.load(&vfs, "m.neon").unwrap(), id);
        assert_eq!(map.len(), 1);
        let err = map.load(&vfs, "missing.neon").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn replace_text_refreshes_line_index() {
        let (mut map, id) = one("a\nb");
        assert_eq!(map.file(id).line_count(), 2);
        map.replace_text(id, "a\nb\nc\nd").unwrap();
        assert_eq!(map.file(id).line_count(), 4);
        assert_eq!(map.file(id).name(), "t.neon");
        assert_eq!(map.file(id).line_text(3), Some("d"));
    }

    #[test]
    fn error_display() {
        let e = SourceMapError::FileTooLarge { name: "x".into(), len: 5 };
        assert!(e.to_string().contains("too large"));
    }
}
