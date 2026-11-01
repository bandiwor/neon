//! Tab completion for the prompt, answered by the same analysis the language server uses: the
//! session's cells followed by the half-typed input are analyzed as one document, so members of a
//! value are looked up by its type and names are the ones visible at the cursor.

use crate::lsp::analysis::complete::{closing_delimiters, completion_context, ident_start, CompletionContext, Item2};
use crate::lsp::analysis::Analysis;
use crate::vfs::Vfs;

/// One thing that can be typed at the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub label: String,
    pub detail: String,
}

/// What can be typed at the cursor and what part of the input it replaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    /// Index (in characters) in the buffer where the word being completed starts.
    pub start: usize,
    pub items: Vec<Candidate>,
}

/// Completions for `buffer` with the cursor after `cursor` characters, after the session's `cells`.
pub fn complete(cells: &[String], buffer: &str, cursor: usize, vfs: &dyn Vfs) -> Option<Completion> {
    let mut text = String::new();
    for c in cells {
        text.push_str(c);
        text.push('\n');
    }
    let base = text.len();
    text.push_str(buffer);
    let cursor_byte = buffer.char_indices().nth(cursor).map_or(buffer.len(), |(i, _)| i);
    let offset = base + cursor_byte;

    let start = ident_start(&text, offset);
    let prefix = text[start..offset].to_string();
    let mut found: Vec<(char, Item2)> = match completion_context(&text, offset) {
        CompletionContext::Nothing => return None,
        CompletionContext::Members { dot } => {
            // `x.pre` is not valid code: analyze `x` alone to learn its type
            let patched = format!("{}{}{}", &text[..dot], closing_delimiters(&text, dot), &text[offset..]);
            let mut a = Analysis::run("<repl>", &patched, vfs);
            a.complete_members(dot as u32).into_iter().map(|i| ('0', i)).collect()
        }
        CompletionContext::Path { qualifier } => {
            let mut a = Analysis::run("<repl>", &text, vfs);
            a.complete_path(&qualifier).into_iter().map(|i| ('0', i)).collect()
        }
        CompletionContext::Scope => {
            let mut a = Analysis::run("<repl>", &text, vfs);
            a.complete_scope(offset as u32)
        }
    };

    let matches = |label: &str, case: bool| {
        if case {
            label.starts_with(&prefix)
        } else {
            label.to_lowercase().starts_with(&prefix.to_lowercase())
        }
    };
    let exact = found.iter().any(|(_, i)| matches(&i.label, true));
    found.retain(|(_, i)| matches(&i.label, exact) && !i.label.is_empty());
    found.sort_by(|(ga, a), (gb, b)| (ga, &a.label).cmp(&(gb, &b.label)));
    let mut items: Vec<Candidate> = Vec::new();
    for (_, i) in found {
        if !items.iter().any(|c| c.label == i.label) {
            items.push(Candidate { label: i.label, detail: i.detail });
        }
    }
    if items.is_empty() {
        return None;
    }
    let start_char = buffer[..start - base].chars().count();
    Some(Completion { start: start_char, items })
}

/// The longest prefix all the labels share.
pub fn common_prefix(items: &[Candidate]) -> String {
    let mut it = items.iter().map(|c| c.label.as_str());
    let Some(first) = it.next() else { return String::new() };
    let mut len = first.len();
    for s in it {
        len = first.bytes().zip(s.bytes()).take(len).take_while(|(a, b)| a == b).count();
    }
    while !first.is_char_boundary(len) {
        len -= 1;
    }
    first[..len].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::MemVfs;

    fn labels(cells: &[&str], buffer: &str) -> Vec<String> {
        let cells: Vec<String> = cells.iter().map(|s| s.to_string()).collect();
        complete(&cells, buffer, buffer.chars().count(), &MemVfs::new())
            .map(|c| c.items.into_iter().map(|i| i.label).collect())
            .unwrap_or_default()
    }

    #[test]
    fn names_of_the_session_and_the_prelude() {
        let cells = ["let numbers = [1, 2, 3];", "fn double(n: i64) -> i64 { n * 2 }"];
        assert_eq!(labels(&cells, "dou"), ["double"]);
        assert!(labels(&cells, "num").contains(&"numbers".to_string()));
        let l = labels(&cells, "pri");
        assert!(l.contains(&"print".to_string()) && l.contains(&"println".to_string()), "{l:?}");
        let l = labels(&cells, "mat");
        assert_eq!(l, ["match"], "keywords too");
    }

    #[test]
    fn members_depend_on_the_type_of_the_receiver() {
        let cells = ["let numbers = [1, 2, 3];", "let name = \"neon\";"];
        let l = labels(&cells, "numbers.pu");
        assert_eq!(l, ["push"], "{l:?}");
        let l = labels(&cells, "name.to_");
        assert!(l.contains(&"to_upper".to_string()) && !l.contains(&"push".to_string()), "{l:?}");
        let l = labels(&cells, "numbers.");
        assert!(l.contains(&"len".to_string()) && l.contains(&"map".to_string()), "{l:?}");
    }

    #[test]
    fn paths_and_user_types() {
        let cells = [
            "enum Color { Red, Green(u8) }",
            "struct P { x: i64, label: string }",
            "let p = P { x: 1, label: \"a\" };",
        ];
        assert_eq!(labels(&cells, "Color::"), ["Green", "Red"]);
        assert_eq!(labels(&cells, "Color::G"), ["Green"]);
        let l = labels(&cells, "p.");
        assert!(l.contains(&"x".to_string()) && l.contains(&"label".to_string()), "{l:?}");
        assert_eq!(labels(&cells, "Opt"), ["Option"]);
    }

    #[test]
    fn nothing_inside_strings_and_the_word_start_is_reported() {
        assert!(labels(&[], "\"pri").is_empty());
        let c = complete(&[], "let x = pri", 11, &MemVfs::new()).unwrap();
        assert_eq!(c.start, 8);
        // completing in the middle of the text only looks at what precedes the cursor
        let c = complete(&[], "pri + 1", 3, &MemVfs::new()).unwrap();
        assert_eq!(c.start, 0);
    }

    #[test]
    fn the_common_prefix() {
        let items = |v: &[&str]| {
            v.iter().map(|s| Candidate { label: s.to_string(), detail: String::new() }).collect::<Vec<_>>()
        };
        assert_eq!(common_prefix(&items(&["print", "println"])), "print");
        assert_eq!(common_prefix(&items(&["a", "b"])), "");
        assert_eq!(common_prefix(&items(&["só", "sóm"])), "só");
    }
}
