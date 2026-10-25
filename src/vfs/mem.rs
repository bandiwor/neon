use std::collections::BTreeMap;
use std::io;

use super::{not_found, path, Vfs};

/// An in-memory file system: a flat map from normalized path to contents.
/// Directories are implicit (any prefix of a file path).
#[derive(Debug, Clone, Default)]
pub struct MemVfs {
    files: BTreeMap<String, String>,
}

impl MemVfs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder-style [`insert`](Self::insert).
    pub fn with_file(mut self, path: &str, contents: &str) -> Self {
        self.insert(path, contents);
        self
    }

    /// Adds or replaces a file, returning the previous contents.
    pub fn insert(&mut self, path: &str, contents: &str) -> Option<String> {
        self.files.insert(path::normalize(path), contents.to_string())
    }

    pub fn remove(&mut self, path: &str) -> Option<String> {
        self.files.remove(&path::normalize(path))
    }

    /// All file paths in sorted order.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    fn is_dir(&self, norm: &str) -> bool {
        if norm == "." || norm == "/" {
            return true;
        }
        let prefix = format!("{norm}/");
        self.files.range(prefix.clone()..).next().is_some_and(|(k, _)| k.starts_with(&prefix))
    }
}

impl Vfs for MemVfs {
    fn read_to_string(&self, p: &str) -> io::Result<String> {
        self.files.get(&path::normalize(p)).cloned().ok_or_else(|| not_found(p))
    }

    fn exists(&self, p: &str) -> bool {
        let norm = path::normalize(p);
        self.files.contains_key(&norm) || self.is_dir(&norm)
    }

    fn is_file(&self, p: &str) -> bool {
        self.files.contains_key(&path::normalize(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_back_and_normalize() {
        let mut v = MemVfs::new();
        assert!(v.is_empty());
        assert_eq!(v.insert("src/main.neon", "let x = 1;"), None);
        assert_eq!(v.read_to_string("src/main.neon").unwrap(), "let x = 1;");
        assert_eq!(v.read_to_string("./src//x/../main.neon").unwrap(), "let x = 1;");
        assert_eq!(v.read_to_string("src\\main.neon").unwrap(), "let x = 1;");
        assert_eq!(v.insert("src/main.neon", "v2").as_deref(), Some("let x = 1;"));
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn missing_file_is_not_found() {
        let v = MemVfs::new();
        let err = v.read_to_string("nope.neon").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(err.to_string().contains("nope.neon"));
    }

    #[test]
    fn exists_and_is_file() {
        let v = MemVfs::new().with_file("a/b/c.neon", "").with_file("a/z.neon", "");
        assert!(v.exists("a/b/c.neon") && v.is_file("a/b/c.neon"));
        assert!(v.exists("a") && !v.is_file("a"));
        assert!(v.exists("a/b") && !v.is_file("a/b"));
        assert!(v.exists(".") && v.exists("/"));
        assert!(!v.exists("a/b/c") && !v.exists("b"));
        // "a/b" must not be confused with a sibling sharing its prefix.
        let v2 = MemVfs::new().with_file("ab/c.neon", "");
        assert!(!v2.exists("a"));
    }

    #[test]
    fn remove_and_list() {
        let mut v = MemVfs::new().with_file("b.neon", "").with_file("a.neon", "");
        assert_eq!(v.paths().collect::<Vec<_>>(), ["a.neon", "b.neon"]);
        assert_eq!(v.remove("a.neon").as_deref(), Some(""));
        assert!(!v.exists("a.neon"));
    }

    #[test]
    fn canonicalize_is_lexical() {
        let v = MemVfs::new();
        assert_eq!(v.canonicalize("a/./b/../c").unwrap(), "a/c");
    }
}
