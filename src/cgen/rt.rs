//! The C runtime, split into small pieces. A piece is a chunk of the output file: either a
//! declaration (types, macros, state) or one or more functions. The generator asks for the pieces
//! the program needs; their dependencies are added transitively, so nothing unused is emitted.

use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Types, macros and state: goes to the declarations as is.
    Decl,
    /// Functions: prototypes go to the declarations, bodies to the implementations at the end.
    Funcs,
}

#[derive(Debug)]
pub struct Piece {
    pub name: &'static str,
    pub deps: &'static [&'static str],
    /// Standard headers the piece needs.
    pub hdrs: &'static [&'static str],
    pub kind: Kind,
    pub src: &'static str,
}

/// Every piece, in an order where declarations come after the declarations they use.
pub fn all() -> Vec<&'static Piece> {
    super::rt_core::PIECES.iter().chain(super::rt_data::PIECES).chain(super::rt_fmt::PIECES).collect()
}

/// The names in `names` and everything they depend on.
pub fn closure(names: &BTreeSet<String>) -> BTreeSet<&'static str> {
    let pieces = all();
    let mut out: BTreeSet<&'static str> = BTreeSet::new();
    let mut work: Vec<String> = names.iter().cloned().collect();
    while let Some(n) = work.pop() {
        let Some(p) = pieces.iter().find(|p| p.name == n) else { panic!("unknown runtime piece `{n}`") };
        if out.insert(p.name) {
            work.extend(p.deps.iter().map(|d| d.to_string()));
        }
    }
    out
}

/// `static T f(args) {` -> `static T f(args);` for every function defined at the top level of `src`.
pub fn prototypes(src: &str) -> Vec<String> {
    src.lines()
        .filter(|l| l.starts_with("static ") && l.ends_with(") {"))
        .map(|l| format!("{};", &l[..l.len() - 2]))
        .collect()
}
