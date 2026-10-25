//! Conversion between byte offsets and LSP positions (line, UTF-16 column).

/// Line starts of one text, for repeated conversions.
#[derive(Debug, Clone)]
pub struct LineIndex<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    pub fn new(text: &'a str) -> LineIndex<'a> {
        let mut starts = vec![0];
        starts.extend(text.bytes().enumerate().filter(|&(_, b)| b == b'\n').map(|(i, _)| i + 1));
        LineIndex { text, starts }
    }

    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    /// `(line, utf16 column)` of a byte offset (clamped to the text).
    pub fn position(&self, offset: usize) -> (u32, u32) {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        let line = self.starts.partition_point(|&s| s <= offset) - 1;
        let col: usize = self.text[self.starts[line]..offset].chars().map(char::len_utf16).sum();
        (line as u32, col as u32)
    }

    /// The byte offset of `(line, utf16 column)`; positions past the end of a
    /// line or of the text are clamped to it, as the protocol requires.
    pub fn offset(&self, line: u32, col: u32) -> usize {
        let Some(&start) = self.starts.get(line as usize) else { return self.text.len() };
        let end = self.starts.get(line as usize + 1).copied().unwrap_or(self.text.len());
        let line_text = &self.text[start..end];
        let mut units = 0u32;
        for (i, c) in line_text.char_indices() {
            let width = c.len_utf16() as u32;
            // a column inside a surrogate pair rounds down to the start of the character
            if units + width > col || c == '\n' || (c == '\r' && line_text[i..].starts_with("\r\n")) {
                return start + i;
            }
            units += width;
        }
        end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_positions() {
        let t = "ab\ncde\n\nf";
        let ix = LineIndex::new(t);
        assert_eq!(ix.line_count(), 4);
        assert_eq!(ix.position(0), (0, 0));
        assert_eq!(ix.position(4), (1, 1));
        assert_eq!(ix.position(7), (2, 0));
        assert_eq!(ix.position(100), (3, 1));
        assert_eq!(ix.offset(1, 2), 5);
        assert_eq!(ix.offset(1, 99), 6, "clamped to the end of the line");
        assert_eq!(ix.offset(2, 0), 7);
        assert_eq!(ix.offset(99, 0), t.len());
    }

    #[test]
    fn utf16_columns_count_surrogates() {
        let t = "é😀x\nñ";
        let ix = LineIndex::new(t);
        // é = 1 unit (2 bytes), 😀 = 2 units (4 bytes), x
        assert_eq!(ix.position(2), (0, 1));
        assert_eq!(ix.position(6), (0, 3));
        assert_eq!(ix.position(7), (0, 4));
        assert_eq!(ix.offset(0, 1), 2);
        assert_eq!(ix.offset(0, 3), 6);
        assert_eq!(ix.offset(0, 2), 2, "inside a surrogate pair rounds down");
        assert_eq!(ix.position(3), (0, 1), "a mid-character byte offset rounds down");
        assert_eq!(ix.offset(1, 1), 10, "after the two-byte ñ that starts line 1 at byte 8");
    }

    #[test]
    fn crlf_lines_stop_before_the_terminator() {
        let t = "ab\r\ncd";
        let ix = LineIndex::new(t);
        assert_eq!(ix.offset(0, 99), 2);
        assert_eq!(ix.offset(1, 1), 5);
        assert_eq!(ix.position(4), (1, 0));
    }
}
