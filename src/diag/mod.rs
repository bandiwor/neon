//! Source positions and error reporting.
//!
//! * [`Span`] / [`FileId`] — compact source locations carried by every token,
//!   AST node and symbol.
//! * [`SourceMap`] — the single authority that turns byte offsets into
//!   0-based lines and columns (what an LSP server needs).
//! * [`Diagnostic`] / [`DiagBag`] — errors are collected, never thrown, so one
//!   run can report many problems.
//! * [`Emitter`] — renders diagnostics as ASCII with source snippets.

mod diagnostic;
mod emitter;
mod source_map;
mod span;

pub use diagnostic::{DiagBag, Diagnostic, Label, Severity};
pub use emitter::Emitter;
pub use source_map::{LineCol, ResolvedSpan, SourceFile, SourceMap, SourceMapError};
pub use span::{FileId, Span};
