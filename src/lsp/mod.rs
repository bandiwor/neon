//! A Language Server Protocol server for Neon.
//!
//! `neon lsp` speaks JSON-RPC over stdio. Every open document is analyzed as
//! the root of its own import graph (imports see other open documents'
//! unsaved text), and the results back diagnostics, hover, go-to-definition,
//! references, rename, document symbols, completion, signature help and
//! semantic tokens.
//!
//! The server never touches the file system or the process streams directly:
//! it is generic over a reader, a writer and a [`Vfs`](crate::vfs::Vfs), so
//! the protocol tests drive it entirely in memory.

pub mod analysis;
pub mod json;
pub mod position;
pub mod server;
pub mod uri;

pub use analysis::Analysis;
pub use server::{read_message, run, write_message, Server};
