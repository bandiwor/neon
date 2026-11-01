//! The host services behind the standard modules (`sys`, `fs`, `term`), as the operating system gives
//! them. The terminal and command-line programs use these; the browser and the tests bring their own
//! (see [`crate::vm::VmIo`]), so the engine itself never touches the OS.

use std::io::ErrorKind;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// What a file operation reports instead of the operating system's own wording, so a program prints the
/// same on every platform (and the C translation, which maps `errno` the same way, agrees).
fn reason(e: &std::io::Error) -> &'static str {
    match e.kind() {
        ErrorKind::NotFound => "not found",
        ErrorKind::PermissionDenied => "permission denied",
        ErrorKind::AlreadyExists => "already exists",
        ErrorKind::NotADirectory => "not a directory",
        ErrorKind::IsADirectory => "is a directory",
        ErrorKind::DirectoryNotEmpty => "directory not empty",
        ErrorKind::InvalidData => "not valid UTF-8 text",
        _ => "i/o error",
    }
}

fn failed(path: &str, e: &std::io::Error) -> String {
    format!("{path}: {}", reason(e))
}

pub fn fs_read(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| failed(path, &e))
}

pub fn fs_write(path: &str, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| failed(path, &e))
}

pub fn fs_append(path: &str, text: &str) -> Result<(), String> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().append(true).create(true).open(path).map_err(|e| failed(path, &e))?;
    f.write_all(text.as_bytes()).map_err(|e| failed(path, &e))
}

pub fn fs_exists(path: &str) -> bool {
    Path::new(path).exists()
}

pub fn fs_is_file(path: &str) -> bool {
    Path::new(path).is_file()
}

pub fn fs_is_dir(path: &str) -> bool {
    Path::new(path).is_dir()
}

/// The names in a directory, sorted, without `.` and `..`.
pub fn fs_list(path: &str) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(path).map_err(|e| failed(path, &e))? {
        let entry = entry.map_err(|e| failed(path, &e))?;
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    Ok(names)
}

pub fn fs_create_dir(path: &str) -> Result<(), String> {
    std::fs::create_dir(path).map_err(|e| failed(path, &e))
}

/// Removes a file or an empty directory.
pub fn fs_remove(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    let r = if p.is_dir() { std::fs::remove_dir(p) } else { std::fs::remove_file(p) };
    r.map_err(|e| failed(path, &e))
}

pub fn fs_rename(from: &str, to: &str) -> Result<(), String> {
    std::fs::rename(from, to).map_err(|e| failed(from, &e))
}

pub fn env(name: &str) -> Option<String> {
    std::env::var_os(name).map(|v| v.to_string_lossy().into_owned())
}

/// `"linux"`, `"macos"`, `"windows"`, ...
pub fn platform() -> &'static str {
    std::env::consts::OS
}

/// Milliseconds since 1970-01-01 UTC.
pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// Milliseconds since the program started: only differences mean something.
pub fn ticks_ms() -> i64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as i64
}

pub fn sleep_ms(ms: u64) {
    std::thread::sleep(std::time::Duration::from_millis(ms));
}

/// A file system in memory, for the places that have no disk (the browser, the tests). Paths are
/// `/`-separated and relative to the root; a directory exists when it was created or has files in it.
#[derive(Debug, Clone, Default)]
pub struct MemFiles {
    files: std::collections::BTreeMap<String, String>,
    dirs: std::collections::BTreeSet<String>,
}

fn clean(path: &str) -> String {
    let parts: Vec<&str> = path.split(['/', '\\']).filter(|p| !p.is_empty() && *p != ".").collect();
    parts.join("/")
}

impl MemFiles {
    pub fn new() -> MemFiles {
        MemFiles::default()
    }

    pub fn with_file(mut self, path: &str, text: &str) -> MemFiles {
        self.files.insert(clean(path), text.to_string());
        self
    }

    pub fn insert(&mut self, path: &str, text: &str) {
        self.files.insert(clean(path), text.to_string());
    }

    /// The files, for a host that wants to see what the program wrote.
    pub fn files(&self) -> &std::collections::BTreeMap<String, String> {
        &self.files
    }

    fn dir_exists(&self, p: &str) -> bool {
        p.is_empty() || self.dirs.contains(p) || self.files.keys().any(|f| f.starts_with(&format!("{p}/")))
    }

    fn parent_ok(&self, p: &str) -> bool {
        match p.rfind('/') {
            Some(i) => self.dir_exists(&p[..i]),
            None => true,
        }
    }

    pub fn read(&self, path: &str) -> Result<String, String> {
        let p = clean(path);
        match self.files.get(&p) {
            Some(t) => Ok(t.clone()),
            None if self.dir_exists(&p) => Err(format!("{path}: is a directory")),
            None => Err(format!("{path}: not found")),
        }
    }

    pub fn write(&mut self, path: &str, text: &str, append: bool) -> Result<(), String> {
        let p = clean(path);
        if self.dir_exists(&p) {
            return Err(format!("{path}: is a directory"));
        }
        if !self.parent_ok(&p) {
            return Err(format!("{path}: not found"));
        }
        let entry = self.files.entry(p).or_default();
        if !append {
            entry.clear();
        }
        entry.push_str(text);
        Ok(())
    }

    pub fn exists(&self, path: &str) -> bool {
        let p = clean(path);
        self.files.contains_key(&p) || self.dir_exists(&p)
    }

    pub fn is_file(&self, path: &str) -> bool {
        self.files.contains_key(&clean(path))
    }

    pub fn is_dir(&self, path: &str) -> bool {
        let p = clean(path);
        !self.files.contains_key(&p) && self.dir_exists(&p)
    }

    pub fn list(&self, path: &str) -> Result<Vec<String>, String> {
        let p = clean(path);
        if self.files.contains_key(&p) {
            return Err(format!("{path}: not a directory"));
        }
        if !self.dir_exists(&p) {
            return Err(format!("{path}: not found"));
        }
        let prefix = if p.is_empty() { String::new() } else { format!("{p}/") };
        let mut names = std::collections::BTreeSet::new();
        for f in self.files.keys().chain(self.dirs.iter()) {
            if let Some(rest) = f.strip_prefix(&prefix) {
                if let Some(name) = rest.split('/').next().filter(|n| !n.is_empty()) {
                    names.insert(name.to_string());
                }
            }
        }
        Ok(names.into_iter().collect())
    }

    pub fn create_dir(&mut self, path: &str) -> Result<(), String> {
        let p = clean(path);
        if self.exists(&p) {
            return Err(format!("{path}: already exists"));
        }
        if !self.parent_ok(&p) {
            return Err(format!("{path}: not found"));
        }
        self.dirs.insert(p);
        Ok(())
    }

    pub fn remove(&mut self, path: &str) -> Result<(), String> {
        let p = clean(path);
        if self.files.remove(&p).is_some() {
            return Ok(());
        }
        if !self.dir_exists(&p) || p.is_empty() {
            return Err(format!("{path}: not found"));
        }
        if self.files.keys().chain(self.dirs.iter()).any(|f| f.starts_with(&format!("{p}/"))) {
            return Err(format!("{path}: directory not empty"));
        }
        self.dirs.remove(&p);
        Ok(())
    }

    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        let (a, b) = (clean(from), clean(to));
        if !self.exists(&a) {
            return Err(format!("{from}: not found"));
        }
        if !self.parent_ok(&b) {
            return Err(format!("{to}: not found"));
        }
        if let Some(t) = self.files.remove(&a) {
            self.files.insert(b, t);
            return Ok(());
        }
        // a directory: everything under it moves
        let moved: Vec<(String, String)> = self
            .files
            .iter()
            .filter_map(|(k, v)| k.strip_prefix(&format!("{a}/")).map(|rest| (format!("{b}/{rest}"), v.clone())))
            .collect();
        self.files.retain(|k, _| !k.starts_with(&format!("{a}/")));
        self.files.extend(moved);
        self.dirs.remove(&a);
        self.dirs.insert(b);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_round_trip_and_errors_are_portable() {
        let dir = std::env::temp_dir().join(format!("neon-host-{}", std::process::id()));
        let d = dir.to_str().unwrap().to_string();
        fs_create_dir(&d).unwrap();
        assert_eq!(fs_create_dir(&d).unwrap_err(), format!("{d}: already exists"));
        let f = format!("{d}/a.txt");
        fs_write(&f, "one").unwrap();
        fs_append(&f, "\ntwo").unwrap();
        assert_eq!(fs_read(&f).unwrap(), "one\ntwo");
        assert!(fs_exists(&f) && fs_is_file(&f) && !fs_is_dir(&f) && fs_is_dir(&d));
        fs_write(&format!("{d}/b.txt"), "").unwrap();
        assert_eq!(fs_list(&d).unwrap(), ["a.txt", "b.txt"]);
        fs_rename(&f, &format!("{d}/c.txt")).unwrap();
        assert_eq!(fs_list(&d).unwrap(), ["b.txt", "c.txt"]);
        assert_eq!(fs_read(&f).unwrap_err(), format!("{f}: not found"));
        assert!(fs_remove(&d).unwrap_err().ends_with("directory not empty"));
        fs_remove(&format!("{d}/b.txt")).unwrap();
        fs_remove(&format!("{d}/c.txt")).unwrap();
        fs_remove(&d).unwrap();
        assert!(!fs_exists(&d));
    }

    #[test]
    fn memory_files_behave_like_a_disk() {
        let mut m = MemFiles::new().with_file("a/b.txt", "x");
        assert!(m.is_dir("a") && m.is_file("a/b.txt") && !m.exists("c"));
        assert_eq!(m.read("a").unwrap_err(), "a: is a directory");
        assert_eq!(m.read("nope").unwrap_err(), "nope: not found");
        m.write("a/c.txt", "1", false).unwrap();
        m.write("a/c.txt", "2", true).unwrap();
        assert_eq!(m.read("a/c.txt").unwrap(), "12");
        assert_eq!(m.write("zzz/q.txt", "", false).unwrap_err(), "zzz/q.txt: not found");
        m.create_dir("d").unwrap();
        assert_eq!(m.list("").unwrap(), ["a", "d"]);
        assert_eq!(m.list("a").unwrap(), ["b.txt", "c.txt"]);
        assert_eq!(m.remove("a").unwrap_err(), "a: directory not empty");
        m.rename("a", "e").unwrap();
        assert_eq!(m.list("e").unwrap(), ["b.txt", "c.txt"]);
        m.remove("d").unwrap();
        assert!(!m.exists("d") && !m.exists("a"));
    }

    #[test]
    fn the_clock_moves_forward() {
        let (a, b) = (ticks_ms(), ticks_ms());
        assert!(b >= a);
        assert!(now_ms() > 1_600_000_000_000);
        assert!(!platform().is_empty());
    }
}
