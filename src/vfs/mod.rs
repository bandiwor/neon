//! Virtual file system.
//!
//! Every byte of source the compiler reads goes through the [`Vfs`] trait.
//! The CLI uses [`OsVfs`]; tests, the REPL and browser/WASM hosts use
//! [`MemVfs`], an in-memory tree of named files.

use std::fmt;
use std::io;

mod mem;
#[cfg(not(target_family = "wasm"))]
mod os;
pub mod path;

pub use mem::MemVfs;
#[cfg(not(target_family = "wasm"))]
pub use os::OsVfs;

/// Read-only view of a file system, as seen by the module loader.
///
/// Paths are `/`-separated strings (see [`path`]); implementations decide
/// what they mean. Keeping the interface string-based means it works the same
/// on a desktop, in a test, and inside a browser.
pub trait Vfs: fmt::Debug {
    /// Reads the whole file as UTF-8 text.
    fn read_to_string(&self, path: &str) -> io::Result<String>;

    /// Whether `path` names a file or directory.
    fn exists(&self, path: &str) -> bool;

    /// Whether `path` names a regular file.
    fn is_file(&self, path: &str) -> bool;

    /// Returns the canonical spelling of `path`, used as the identity of a
    /// module (two imports of the same file must canonicalize equally).
    /// The default is lexical normalization.
    fn canonicalize(&self, path: &str) -> io::Result<String> {
        Ok(path::normalize(path))
    }

    /// Resolves the import specifier `spec` written inside `importer`.
    fn resolve(&self, importer: &str, spec: &str) -> String {
        path::resolve_import(importer, spec)
    }
}

pub(crate) fn not_found(path: &str) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("no such file: {path}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The trait must stay object-safe: the loader holds `&dyn Vfs`.
    #[test]
    fn vfs_is_object_safe() {
        let mem = MemVfs::new().with_file("a.neon", "1");
        let dynamic: &dyn Vfs = &mem;
        assert_eq!(dynamic.read_to_string("a.neon").unwrap(), "1");
    }

    #[test]
    fn default_resolve_is_relative_to_importer() {
        let mem = MemVfs::new();
        assert_eq!(mem.resolve("src/main.neon", "../lib/x.neon"), "lib/x.neon");
    }
}
