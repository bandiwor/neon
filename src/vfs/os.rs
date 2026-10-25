use std::fs;
use std::io;
use std::path::Path;

use super::Vfs;

/// The host file system. Only compiled on non-WASM targets; the compiler
/// itself never names it, it just receives a `&dyn Vfs`.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsVfs;

impl OsVfs {
    pub fn new() -> Self {
        OsVfs
    }
}

impl Vfs for OsVfs {
    fn read_to_string(&self, path: &str) -> io::Result<String> {
        fs::read_to_string(path)
    }

    fn write(&self, path: &str, contents: &str) -> io::Result<()> {
        fs::write(path, contents)
    }

    fn exists(&self, path: &str) -> bool {
        Path::new(path).exists()
    }

    fn is_file(&self, path: &str) -> bool {
        Path::new(path).is_file()
    }

    fn canonicalize(&self, path: &str) -> io::Result<String> {
        let canon = fs::canonicalize(path)?;
        canon
            .into_os_string()
            .into_string()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, format!("path is not valid UTF-8: {path}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("neon-osvfs-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_real_files() {
        let dir = scratch_dir("read");
        let file = dir.join("a.neon");
        fs::write(&file, "let x = 1;").unwrap();
        let p = file.to_str().unwrap();

        let vfs = OsVfs::new();
        assert_eq!(vfs.read_to_string(p).unwrap(), "let x = 1;");
        assert!(vfs.exists(p) && vfs.is_file(p));
        assert!(vfs.exists(dir.to_str().unwrap()) && !vfs.is_file(dir.to_str().unwrap()));
        assert!(!vfs.exists(dir.join("missing").to_str().unwrap()));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_file_errors_and_canonicalize_resolves_dots() {
        let dir = scratch_dir("canon");
        fs::write(dir.join("m.neon"), "").unwrap();
        let vfs = OsVfs::new();

        let err = vfs.read_to_string(dir.join("nope.neon").to_str().unwrap()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);

        let direct = vfs.canonicalize(dir.join("m.neon").to_str().unwrap()).unwrap();
        let dotted = vfs.canonicalize(dir.join(".").join("m.neon").to_str().unwrap()).unwrap();
        assert_eq!(direct, dotted);
        assert!(vfs.canonicalize(dir.join("nope.neon").to_str().unwrap()).is_err());

        fs::remove_dir_all(&dir).unwrap();
    }
}
