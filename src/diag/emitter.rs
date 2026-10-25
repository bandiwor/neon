//! Plain-ASCII, rustc-style diagnostic renderer.
//!
//! ```text
//! error[E0001]: mismatched types
//!  --> main.neon:2:13
//!   |
//! 2 | let x: i32 = "a";
//!   |        ---   ^^^ expected `i32`, found `string`
//!   |        |
//!   |        declared here
//!   |
//!   = help: convert with `to_i32`
//! ```
//!
//! The emitter writes into any [`fmt::Write`], never to a terminal, so the
//! CLI, the REPL and a browser host can all route the text where they like.
//! Tabs are expanded to four columns; every other character counts as one
//! column. A span that crosses lines is shown on its first and last line only.

use std::collections::BTreeMap;
use std::fmt::{self, Write};

use super::diagnostic::{DiagBag, Diagnostic, Label};
use super::source_map::{LineCol, SourceFile, SourceMap};
use super::span::FileId;

const TAB_WIDTH: usize = 4;

/// Renders [`Diagnostic`]s against the sources in a [`SourceMap`].
#[derive(Debug, Clone, Copy)]
pub struct Emitter<'a> {
    map: &'a SourceMap,
}

/// One underlined range on one source line, in display columns.
#[derive(Debug)]
struct Ann {
    start: usize,
    end: usize,
    primary: bool,
    message: String,
}

/// All annotations of one file, keyed by 0-based line.
#[derive(Debug)]
struct Group {
    file: FileId,
    /// 0-based line and 1-based char column of the location header.
    anchor: (u32, u32),
    lines: BTreeMap<u32, Vec<Ann>>,
}

fn display_width(s: &str) -> usize {
    s.chars().map(|c| if c == '\t' { TAB_WIDTH } else { 1 }).sum()
}

fn digits(mut n: u32) -> usize {
    let mut d = 1;
    while n >= 10 {
        n /= 10;
        d += 1;
    }
    d
}

fn display_col(file: &SourceFile, lc: LineCol) -> usize {
    display_width(file.line_prefix(lc))
}

impl<'a> Emitter<'a> {
    pub fn new(map: &'a SourceMap) -> Emitter<'a> {
        Emitter { map }
    }

    /// Renders one diagnostic to a string.
    pub fn render(&self, d: &Diagnostic) -> String {
        let mut s = String::new();
        self.emit(d, &mut s).expect("writing to a String cannot fail");
        s
    }

    /// Renders every diagnostic in the bag, separated by blank lines, followed
    /// by a one-line summary.
    pub fn render_all(&self, bag: &DiagBag) -> String {
        let mut s = String::new();
        self.emit_all(bag, &mut s).expect("writing to a String cannot fail");
        s
    }

    pub fn emit_all(&self, bag: &DiagBag, out: &mut dyn Write) -> fmt::Result {
        for d in bag {
            self.emit(d, out)?;
            out.write_char('\n')?;
        }
        let (e, w) = (bag.error_count(), bag.warning_count());
        if e > 0 {
            let plural = if e == 1 { "" } else { "s" };
            write!(out, "error: aborting due to {e} previous error{plural}")?;
            if w > 0 {
                let plural = if w == 1 { "" } else { "s" };
                write!(out, "; {w} warning{plural} emitted")?;
            }
            out.write_char('\n')?;
        } else if w > 0 {
            let plural = if w == 1 { "" } else { "s" };
            writeln!(out, "warning: {w} warning{plural} emitted")?;
        }
        Ok(())
    }

    pub fn emit(&self, d: &Diagnostic, out: &mut dyn Write) -> fmt::Result {
        write!(out, "{}", d.severity)?;
        if let Some(code) = d.code {
            write!(out, "[{code}]")?;
        }
        writeln!(out, ": {}", d.message)?;

        let groups = self.build_groups(d);
        let max_line = groups.iter().flat_map(|g| g.lines.keys()).max().map_or(1, |l| l + 1);
        let w = digits(max_line);
        let pad = " ".repeat(w);

        for (i, g) in groups.iter().enumerate() {
            let file = self.map.file(g.file);
            let arrow = if i == 0 { "-->" } else { ":::" };
            writeln!(out, "{pad}{arrow} {}:{}:{}", file.name(), g.anchor.0 + 1, g.anchor.1)?;
            writeln!(out, "{pad} |")?;
            let mut prev: Option<u32> = None;
            for (&line, anns) in &g.lines {
                if let Some(p) = prev {
                    match line - p {
                        1 => {}
                        2 => write_source_line(out, w, p + 1, file)?,
                        _ => writeln!(out, "...")?,
                    }
                }
                write_source_line(out, w, line, file)?;
                for row in annotation_rows(anns) {
                    if row.is_empty() {
                        writeln!(out, "{pad} |")?;
                    } else {
                        writeln!(out, "{pad} | {row}")?;
                    }
                }
                prev = Some(line);
            }
        }

        if !groups.is_empty() && !(d.notes.is_empty() && d.helps.is_empty()) {
            writeln!(out, "{pad} |")?;
        }
        for n in &d.notes {
            writeln!(out, "{pad} = note: {n}")?;
        }
        for h in &d.helps {
            writeln!(out, "{pad} = help: {h}")?;
        }
        Ok(())
    }

    /// Groups labels by file (primary file first) and splits them into
    /// per-line annotations. Labels with dummy or unknown spans are skipped.
    fn build_groups(&self, d: &Diagnostic) -> Vec<Group> {
        let mut ordered: Vec<&Label> = d.labels.iter().collect();
        ordered.sort_by_key(|l| !l.primary); // stable: primaries first
        let mut groups: Vec<Group> = Vec::new();

        for label in ordered {
            let Some(file) = self.map.get(label.span.file) else {
                continue;
            };
            let lo = label.span.lo.min(file.len());
            let mut hi = label.span.hi.min(file.len()).max(lo);
            // A span that swallows its line terminator stops at the end of the line.
            let bytes = file.text().as_bytes();
            if hi > lo && bytes[hi as usize - 1] == b'\n' {
                hi -= 1;
                if hi > lo && bytes[hi as usize - 1] == b'\r' {
                    hi -= 1;
                }
            }
            let start = file.line_col(lo);
            let end = file.line_col(hi);

            let gi = match groups.iter().position(|g| g.file == label.span.file) {
                Some(i) => i,
                None => {
                    groups.push(Group {
                        file: label.span.file,
                        anchor: (start.line, file.char_col(start) + 1),
                        lines: BTreeMap::new(),
                    });
                    groups.len() - 1
                }
            };
            let lines = &mut groups[gi].lines;

            let s = display_col(file, start);
            if start.line == end.line {
                let e = display_col(file, end).max(s + 1);
                lines.entry(start.line).or_default().push(Ann {
                    start: s,
                    end: e,
                    primary: label.primary,
                    message: label.message.clone(),
                });
            } else {
                let first_end = display_width(file.line_text(start.line).unwrap_or("")).max(s + 1);
                lines.entry(start.line).or_default().push(Ann {
                    start: s,
                    end: first_end,
                    primary: label.primary,
                    message: String::new(),
                });
                let last_text = file.line_text(end.line).unwrap_or("");
                let indent = display_width(&last_text[..last_text.len() - last_text.trim_start().len()]);
                let e = display_col(file, end).max(1);
                lines.entry(end.line).or_default().push(Ann {
                    start: if indent < e { indent } else { 0 },
                    end: e,
                    primary: label.primary,
                    message: label.message.clone(),
                });
            }
        }
        groups
    }
}

fn write_source_line(out: &mut dyn Write, w: usize, line: u32, file: &SourceFile) -> fmt::Result {
    let text = file.line_text(line).unwrap_or("").replace('\t', &" ".repeat(TAB_WIDTH));
    if text.is_empty() {
        writeln!(out, "{:>w$} |", line + 1)
    } else {
        writeln!(out, "{:>w$} | {text}", line + 1)
    }
}

/// The marker row(s) under one source line: `^^^`/`---` underlines with the
/// rightmost message inline and earlier messages hung below on `|` stems.
fn annotation_rows(anns: &[Ann]) -> Vec<String> {
    let mut sorted: Vec<&Ann> = anns.iter().collect();
    sorted.sort_by_key(|a| (a.start, a.end));

    let width = sorted.iter().map(|a| a.end).max().unwrap_or(0);
    let mut marks = vec![' '; width];
    for primary_pass in [false, true] {
        for a in sorted.iter().filter(|a| a.primary == primary_pass) {
            marks[a.start..a.end].fill(if a.primary { '^' } else { '-' });
        }
    }
    let mut first: String = marks.into_iter().collect();

    let mut with_msg: Vec<&Ann> = sorted.into_iter().filter(|a| !a.message.is_empty()).collect();
    let mut rows = Vec::new();
    if let Some(last) = with_msg.pop() {
        first.push(' ');
        first.push_str(&last.message);
    }
    rows.push(first.trim_end().to_string());

    if !with_msg.is_empty() {
        let stem_row = |upto: usize| -> Vec<char> {
            let mut row = Vec::new();
            for a in &with_msg[..upto] {
                row.resize(a.start + 1, ' ');
                row[a.start] = '|';
            }
            row
        };
        rows.push(stem_row(with_msg.len()).into_iter().collect());
        for i in (0..with_msg.len()).rev() {
            let mut row = stem_row(i);
            row.resize(with_msg[i].start, ' ');
            row.extend(with_msg[i].message.chars());
            rows.push(row.into_iter().collect());
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::span::Span;

    fn setup(name: &str, text: &str) -> (SourceMap, FileId) {
        let mut map = SourceMap::new();
        let id = map.add_file(name, text).unwrap();
        (map, id)
    }

    #[test]
    fn single_label() {
        let (map, id) = setup("main.neon", "let x = ;\n");
        let d =
            Diagnostic::error("expected expression").with_code("E0001").with_label(Span::new(id, 8, 9), "found `;`");
        let out = Emitter::new(&map).render(&d);
        assert_eq!(
            out,
            "error[E0001]: expected expression\n\
             \x20--> main.neon:1:9\n\
             \x20 |\n\
             1 | let x = ;\n\
             \x20 |         ^ found `;`\n"
        );
    }

    #[test]
    fn primary_and_secondary_on_one_line() {
        let (map, id) = setup("m.neon", "let x: i32 = \"a\";\n");
        let d = Diagnostic::error("mismatched types")
            .with_secondary(Span::new(id, 7, 10), "declared here")
            .with_label(Span::new(id, 13, 16), "expected `i32`, found `string`")
            .with_help("convert it");
        let out = Emitter::new(&map).render(&d);
        assert_eq!(
            out,
            "error: mismatched types\n\
             \x20--> m.neon:1:14\n\
             \x20 |\n\
             1 | let x: i32 = \"a\";\n\
             \x20 |        ---   ^^^ expected `i32`, found `string`\n\
             \x20 |        |\n\
             \x20 |        declared here\n\
             \x20 |\n\
             \x20 = help: convert it\n"
        );
    }

    #[test]
    fn three_labels_on_one_line() {
        let (map, id) = setup("m.neon", "f(aa, bb, cc)\n");
        let d = Diagnostic::error("x")
            .with_secondary(Span::new(id, 2, 4), "a")
            .with_secondary(Span::new(id, 6, 8), "b")
            .with_label(Span::new(id, 10, 12), "c");
        let out = Emitter::new(&map).render(&d);
        assert_eq!(
            out,
            "error: x\n\
             \x20--> m.neon:1:11\n\
             \x20 |\n\
             1 | f(aa, bb, cc)\n\
             \x20 |   --  --  ^^ c\n\
             \x20 |   |   |\n\
             \x20 |   |   b\n\
             \x20 |   a\n"
        );
    }

    #[test]
    fn distant_lines_get_ellipsis_and_near_lines_fill_gap() {
        let text = "a1\nb2\nc3\nd4\ne5\nf6\n";
        let (map, id) = setup("m.neon", text);
        // lines 0, 2 (one-line gap, shown) and 5 (gap of 3, elided)
        let d = Diagnostic::warning("w")
            .with_label(Span::new(id, 0, 2), "p")
            .with_secondary(Span::new(id, 6, 8), "q")
            .with_secondary(Span::new(id, 15, 17), "r");
        let out = Emitter::new(&map).render(&d);
        assert_eq!(
            out,
            "warning: w\n\
             \x20--> m.neon:1:1\n\
             \x20 |\n\
             1 | a1\n\
             \x20 | ^^ p\n\
             2 | b2\n\
             3 | c3\n\
             \x20 | -- q\n\
             ...\n\
             6 | f6\n\
             \x20 | -- r\n"
        );
    }

    #[test]
    fn gutter_widens_for_two_digit_lines() {
        let mut text = String::new();
        for i in 1..=12 {
            text.push_str(&format!("l{i}\n"));
        }
        let (map, id) = setup("m.neon", &text);
        let off = text.find("l12").unwrap() as u32;
        let d = Diagnostic::error("e").with_label(Span::new(id, off, off + 3), "here");
        let out = Emitter::new(&map).render(&d);
        assert_eq!(
            out,
            "error: e\n\
             \x20 --> m.neon:12:1\n\
             \x20  |\n\
             12 | l12\n\
             \x20  | ^^^ here\n"
        );
    }

    #[test]
    fn tabs_are_expanded() {
        let (map, id) = setup("m.neon", "\tlet x = ;\n");
        let d = Diagnostic::error("e").with_label(Span::new(id, 9, 10), "here");
        let out = Emitter::new(&map).render(&d);
        assert!(out.contains("1 |     let x = ;\n"), "{out}");
        assert!(out.contains("  |             ^ here\n"), "{out}");
        // header column counts characters, not expanded tabs
        assert!(out.contains("m.neon:1:10\n"), "{out}");
    }

    #[test]
    fn empty_span_at_eof_gets_one_caret() {
        let (map, id) = setup("m.neon", "let x = 1");
        let d = Diagnostic::error("expected `;`").with_label(Span::new(id, 9, 9), "here");
        let out = Emitter::new(&map).render(&d);
        assert!(out.contains("1 | let x = 1\n  |          ^ here\n"), "{out}");
    }

    #[test]
    fn multiline_span_shows_first_and_last_line() {
        let (map, id) = setup("m.neon", "fn f() {\n    1;\n    2;\n}\n");
        let d = Diagnostic::error("unclosed").with_label(Span::new(id, 5, 24), "this body");
        let out = Emitter::new(&map).render(&d);
        assert_eq!(
            out,
            "error: unclosed\n\
             \x20--> m.neon:1:6\n\
             \x20 |\n\
             1 | fn f() {\n\
             \x20 |      ^^^\n\
             ...\n\
             4 | }\n\
             \x20 | ^ this body\n"
        );
    }

    #[test]
    fn span_ending_after_newline_stays_on_its_line() {
        let (map, id) = setup("m.neon", "ab\ncd\n");
        let d = Diagnostic::error("e").with_label(Span::new(id, 0, 3), "line");
        let out = Emitter::new(&map).render(&d);
        assert!(out.contains("1 | ab\n  | ^^ line\n"), "{out}");
        assert!(!out.contains("2 |"), "{out}");
    }

    #[test]
    fn multiple_files_use_triple_colon() {
        let mut map = SourceMap::new();
        let a = map.add_file("a.neon", "let a = 1;\n").unwrap();
        let b = map.add_file("b.neon", "let a = 2;\n").unwrap();
        let d = Diagnostic::error("duplicate")
            .with_label(Span::new(b, 4, 5), "redefined here")
            .with_secondary(Span::new(a, 4, 5), "first defined here");
        let out = Emitter::new(&map).render(&d);
        assert_eq!(
            out,
            "error: duplicate\n\
             \x20--> b.neon:1:5\n\
             \x20 |\n\
             1 | let a = 2;\n\
             \x20 |     ^ redefined here\n\
             \x20::: a.neon:1:5\n\
             \x20 |\n\
             1 | let a = 1;\n\
             \x20 |     - first defined here\n"
        );
    }

    #[test]
    fn dummy_spans_and_notes_without_snippets() {
        let map = SourceMap::new();
        let d = Diagnostic::error("cannot read `x.neon`").with_label(Span::DUMMY, "ignored").with_note("no such file");
        assert_eq!(Emitter::new(&map).render(&d), "error: cannot read `x.neon`\n  = note: no such file\n");
    }

    #[test]
    fn unicode_columns_count_chars() {
        let (map, id) = setup("m.neon", "let é = ;\n");
        // `;` is at byte 9 but char column 8 (0-based)
        let d = Diagnostic::error("e").with_label(Span::new(id, 9, 10), "here");
        let out = Emitter::new(&map).render(&d);
        assert!(out.contains("m.neon:1:9\n"), "{out}");
        assert!(out.contains("1 | let é = ;\n  |         ^ here\n"), "{out}");
    }

    #[test]
    fn render_all_adds_summary() {
        let (map, id) = setup("m.neon", "ab\n");
        let mut bag = DiagBag::new();
        bag.error(Span::new(id, 0, 1), "first");
        bag.warning(Span::new(id, 1, 2), "second");
        bag.error(Span::new(id, 0, 2), "third");
        let out = Emitter::new(&map).render_all(&bag);
        assert!(out.starts_with("error: first\n"), "{out}");
        assert!(out.contains("\n\nwarning: second\n"), "{out}");
        assert!(out.ends_with("\nerror: aborting due to 2 previous errors; 1 warning emitted\n"));

        let mut one = DiagBag::new();
        one.error(Span::new(id, 0, 1), "only");
        assert!(Emitter::new(&map).render_all(&one).ends_with("error: aborting due to 1 previous error\n"));

        let mut warn = DiagBag::new();
        warn.warning(Span::new(id, 0, 1), "careful");
        assert!(Emitter::new(&map).render_all(&warn).ends_with("warning: 1 warning emitted\n"));

        assert_eq!(Emitter::new(&map).render_all(&DiagBag::new()), "");
    }

    #[test]
    fn out_of_range_span_does_not_panic() {
        let (map, id) = setup("m.neon", "ab");
        let d = Diagnostic::error("e").with_label(Span::new(id, 50, 90), "far");
        let out = Emitter::new(&map).render(&d);
        assert!(out.contains("1 | ab\n  |   ^ far\n"), "{out}");
    }
}
