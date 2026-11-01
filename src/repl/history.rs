//! The history file: one entry per line, line breaks and backslashes escaped.

use std::io::Write;
use std::path::PathBuf;

const KEEP: usize = 2000;

/// Where the history lives: `$NEON_HISTORY` (empty: nowhere), else `~/.neon_history`.
pub fn default_path() -> Option<PathBuf> {
    match std::env::var_os("NEON_HISTORY") {
        Some(p) if p.is_empty() => None,
        Some(p) => Some(PathBuf::from(p)),
        None => std::env::var_os("HOME").filter(|h| !h.is_empty()).map(|h| PathBuf::from(h).join(".neon_history")),
    }
}

pub fn encode(entry: &str) -> String {
    entry.replace('\\', "\\\\").replace('\n', "\\n")
}

pub fn decode(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The most recent entries of the file (none if it cannot be read).
pub fn load(path: &PathBuf) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let all: Vec<String> = text.lines().filter(|l| !l.is_empty()).map(decode).collect();
    all[all.len().saturating_sub(KEEP)..].to_vec()
}

/// Appends one entry; errors are ignored (a read-only home must not break the prompt).
pub fn append(path: &PathBuf, entry: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{}", encode(entry));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_round_trip() {
        for e in ["x + 1", "fn f() {\n    1\n}", "let s = \"a\\nb\";", "tail\\", ""] {
            assert_eq!(decode(&encode(e)), e, "{e:?}");
            assert!(!encode(e).contains('\n'));
        }
    }

    #[test]
    fn the_file_is_appended_and_loaded() {
        let path = std::env::temp_dir().join(format!("neon-history-test-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        assert!(load(&path).is_empty());
        append(&path, "let a = 1;");
        append(&path, "fn f() {\n}");
        assert_eq!(load(&path), ["let a = 1;", "fn f() {\n}"]);
        let _ = std::fs::remove_file(&path);
    }
}
