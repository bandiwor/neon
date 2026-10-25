//! Pure, lexical path helpers shared by every [`Vfs`](super::Vfs).
//!
//! Neon module paths are always treated as `/`-separated; backslashes are
//! converted so that sources behave identically on every host (and in the
//! browser, where there is no host path syntax at all). Nothing here touches
//! a file system.

fn is_drive(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

fn has_drive_prefix(s: &str) -> bool {
    s.len() >= 2 && is_drive(&s[..2])
}

/// Whether `path` is absolute (`/x`, `C:/x`, `C:\x`).
pub fn is_absolute(path: &str) -> bool {
    let rest = if has_drive_prefix(path) { &path[2..] } else { path };
    rest.starts_with('/') || rest.starts_with('\\')
}

/// Lexically normalizes a path: unifies separators, collapses `//`, and
/// resolves `.` and `..`. Leading `..` of a relative path are preserved;
/// `..` above an absolute root is dropped. An empty result becomes `.`.
pub fn normalize(path: &str) -> String {
    let unified = path.replace('\\', "/");
    let mut rest: &str = &unified;
    let mut out = String::new();
    if has_drive_prefix(rest) {
        out.push_str(&rest[..2]);
        rest = &rest[2..];
    }
    let absolute = rest.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for comp in rest.split('/') {
        match comp {
            "" | "." => {}
            ".." => match parts.last() {
                Some(&last) if last != ".." => {
                    parts.pop();
                }
                _ if absolute => {}
                _ => parts.push(".."),
            },
            c => parts.push(c),
        }
    }
    if absolute {
        out.push('/');
    }
    out.push_str(&parts.join("/"));
    if out.is_empty() {
        out.push('.');
    }
    out
}

/// The directory part of `path` (`"a/b.neon"` → `"a"`, `"b.neon"` → `"."`).
pub fn dir_of(path: &str) -> &str {
    match path.rfind(['/', '\\']) {
        None if has_drive_prefix(path) => &path[..2],
        None => ".",
        Some(i) => {
            let dir = &path[..i];
            if dir.is_empty() || is_drive(dir) {
                &path[..=i]
            } else {
                dir
            }
        }
    }
}

/// The final component of `path`.
pub fn file_name(path: &str) -> &str {
    match path.rfind(['/', '\\']) {
        Some(i) => &path[i + 1..],
        None if has_drive_prefix(path) => &path[2..],
        None => path,
    }
}

/// The extension of the final component, without the dot. Dotfiles
/// (`.gitignore`) have none.
pub fn extension(path: &str) -> Option<&str> {
    let name = file_name(path);
    match name.rfind('.') {
        Some(0) | None => None,
        Some(i) => Some(&name[i + 1..]),
    }
}

/// Appends `.ext` unless the final component already has an extension.
pub fn with_default_extension(path: &str, ext: &str) -> String {
    if extension(path).is_some() {
        path.to_string()
    } else {
        format!("{path}.{ext}")
    }
}

/// Joins `rel` onto the directory `base`. An absolute `rel` replaces `base`.
/// The result is normalized.
pub fn join(base: &str, rel: &str) -> String {
    if is_absolute(rel) {
        normalize(rel)
    } else {
        normalize(&format!("{base}/{rel}"))
    }
}

/// Resolves an import specifier relative to the file that contains it.
pub fn resolve_import(importer: &str, spec: &str) -> String {
    join(dir_of(importer), spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_basics() {
        assert_eq!(normalize("a/b/../c"), "a/c");
        assert_eq!(normalize("./a//b/./c/"), "a/b/c");
        assert_eq!(normalize(""), ".");
        assert_eq!(normalize("."), ".");
        assert_eq!(normalize("a/.."), ".");
        assert_eq!(normalize("a\\b\\c.neon"), "a/b/c.neon");
    }

    #[test]
    fn normalize_leading_parents() {
        assert_eq!(normalize("../a"), "../a");
        assert_eq!(normalize("../../a/../b"), "../../b");
        assert_eq!(normalize("a/../../b"), "../b");
    }

    #[test]
    fn normalize_absolute() {
        assert_eq!(normalize("/a/../.."), "/");
        assert_eq!(normalize("/../a"), "/a");
        assert_eq!(normalize("/"), "/");
        assert_eq!(normalize("C:\\x\\..\\y"), "C:/y");
        assert_eq!(normalize("C:/.."), "C:/");
    }

    #[test]
    fn absolute_detection() {
        assert!(is_absolute("/x"));
        assert!(is_absolute("C:/x"));
        assert!(is_absolute("C:\\x"));
        assert!(!is_absolute("x/y"));
        assert!(!is_absolute("C:x"));
    }

    #[test]
    fn dir_and_name() {
        assert_eq!(dir_of("a/b/c.neon"), "a/b");
        assert_eq!(dir_of("c.neon"), ".");
        assert_eq!(dir_of("/c.neon"), "/");
        assert_eq!(dir_of("C:/c.neon"), "C:/");
        assert_eq!(file_name("a/b/c.neon"), "c.neon");
        assert_eq!(file_name("c.neon"), "c.neon");
        assert_eq!(file_name("a/b/"), "");
    }

    #[test]
    fn extensions() {
        assert_eq!(extension("a/b.neon"), Some("neon"));
        assert_eq!(extension("a.b/c"), None);
        assert_eq!(extension(".hidden"), None);
        assert_eq!(extension("x.tar.gz"), Some("gz"));
        assert_eq!(with_default_extension("lib/util", "neon"), "lib/util.neon");
        assert_eq!(with_default_extension("lib/util.neon", "neon"), "lib/util.neon");
    }

    #[test]
    fn joins_and_imports() {
        assert_eq!(join("a/b", "../c.neon"), "a/c.neon");
        assert_eq!(join("a/b", "/abs/x"), "/abs/x");
        assert_eq!(resolve_import("src/main.neon", "./util"), "src/util");
        assert_eq!(resolve_import("main.neon", "lib/x"), "lib/x");
        assert_eq!(resolve_import("/p/q/main.neon", "../r"), "/p/r");
    }
}
