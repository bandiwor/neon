//! `textDocument/semanticTokens/full`: classification of identifiers by what
//! they refer to, plus keywords, literals and comments.

use super::Analysis;
use crate::diag::{DiagBag, FileId};
use crate::sema::{DefKind, FnOwner};
use crate::syntax::{tokenize, Interner, LitPool, TokenKind};

pub const TOKEN_TYPES: &[&str] = &[
    "namespace",     // 0
    "type",          // 1
    "struct",        // 2
    "enum",          // 3
    "interface",     // 4
    "typeParameter", // 5
    "parameter",     // 6
    "variable",      // 7
    "property",      // 8
    "enumMember",    // 9
    "function",      // 10
    "method",        // 11
    "keyword",       // 12
    "comment",       // 13
    "string",        // 14
    "number",        // 15
];

pub const TOKEN_MODIFIERS: &[&str] = &["declaration", "readonly", "defaultLibrary"];

const T_NAMESPACE: u32 = 0;
const T_TYPE: u32 = 1;
const T_STRUCT: u32 = 2;
const T_ENUM: u32 = 3;
const T_INTERFACE: u32 = 4;
const T_TYPE_PARAM: u32 = 5;
const T_PARAMETER: u32 = 6;
const T_VARIABLE: u32 = 7;
const T_PROPERTY: u32 = 8;
const T_ENUM_MEMBER: u32 = 9;
const T_FUNCTION: u32 = 10;
const T_METHOD: u32 = 11;
const T_KEYWORD: u32 = 12;
const T_COMMENT: u32 = 13;
const T_STRING: u32 = 14;
const T_NUMBER: u32 = 15;

const M_DECLARATION: u32 = 1;
const M_READONLY: u32 = 2;
const M_DEFAULT_LIBRARY: u32 = 4;

/// One classified stretch of a single line (byte offsets).
struct Piece {
    lo: usize,
    hi: usize,
    ty: u32,
    modifiers: u32,
}

impl Analysis {
    /// The encoded `data` array of the semantic tokens response.
    pub fn semantic_tokens(&self) -> Vec<u32> {
        let text = self.text().to_string();
        let mut pieces: Vec<Piece> = Vec::new();

        // identifiers, keywords and literals from the token stream
        let mut interner = Interner::new();
        let mut pool = LitPool::new();
        let mut diags = DiagBag::new();
        let tokens = tokenize(FileId(0), &text, &mut interner, &mut pool, &mut diags);
        let mut covered: Vec<(usize, usize)> = Vec::new();
        for t in &tokens {
            let (lo, hi) = (t.span.lo as usize, t.span.hi as usize);
            covered.push((lo, hi));
            match t.kind {
                TokenKind::Kw(k) => {
                    let ty = if k.is_primitive_type() { T_TYPE } else { T_KEYWORD };
                    let modifiers = if k.is_primitive_type() { M_DEFAULT_LIBRARY } else { 0 };
                    pieces.push(Piece { lo, hi, ty, modifiers });
                }
                TokenKind::Int(_) | TokenKind::Float(_) => pieces.push(Piece { lo, hi, ty: T_NUMBER, modifiers: 0 }),
                TokenKind::Str(_) | TokenKind::Char(_) => pieces.push(Piece { lo, hi, ty: T_STRING, modifiers: 0 }),
                TokenKind::Ident(_) => {
                    if let Some((ty, modifiers)) = self.classify_identifier(lo as u32) {
                        pieces.push(Piece { lo, hi, ty, modifiers });
                    }
                }
                _ => {}
            }
        }

        // comments live in the gaps between tokens
        covered.sort();
        let mut at = 0usize;
        for (lo, hi) in covered.iter().copied().chain(std::iter::once((text.len(), text.len()))) {
            if lo > at {
                scan_comments(&text, at, lo, &mut pieces);
            }
            at = at.max(hi);
        }

        pieces.sort_by_key(|p| p.lo);
        self.encode(&text, pieces)
    }

    fn classify_identifier(&self, offset: u32) -> Option<(u32, u32)> {
        let def_id = self.def_at(offset)?;
        let d = self.prog.symbols.def(def_id);
        let is_decl = d.span.file == self.file && d.span.contains(offset);
        let mut modifiers = if is_decl { M_DECLARATION } else { 0 };
        let in_prelude = self.map.get(d.span.file).is_some_and(|f| f.name().starts_with('<'));
        if in_prelude {
            modifiers |= M_DEFAULT_LIBRARY;
        }
        let ty = match d.kind {
            DefKind::Module(_) => T_NAMESPACE,
            DefKind::Fn(f) => match self.prog.func(f).owner {
                FnOwner::Impl(_) | FnOwner::Trait(_) => T_METHOD,
                _ => T_FUNCTION,
            },
            DefKind::Adt(a) => {
                if self.prog.adt(a).is_enum() {
                    T_ENUM
                } else {
                    T_STRUCT
                }
            }
            DefKind::Variant(..) => T_ENUM_MEMBER,
            DefKind::Field(..) => T_PROPERTY,
            DefKind::Trait(_) => T_INTERFACE,
            DefKind::Global(g) => {
                let gd = self.prog.global(g);
                if gd.is_const || !gd.mutable {
                    modifiers |= M_READONLY;
                }
                T_VARIABLE
            }
            DefKind::Local(l) => {
                let ld = self.prog.local(l);
                if !ld.mutable {
                    modifiers |= M_READONLY;
                }
                // parameters are the first locals of their function
                let is_param = self.prog.fns.iter().any(|f| f.param_locals.contains(&l))
                    || self.prog.modules[ld.module.index()].info.closures.values().any(|c| c.params.contains(&l));
                if is_param {
                    T_PARAMETER
                } else {
                    T_VARIABLE
                }
            }
            DefKind::GenericParam => T_TYPE_PARAM,
            DefKind::ImportName => T_VARIABLE,
        };
        Some((ty, modifiers))
    }

    fn encode(&self, text: &str, pieces: Vec<Piece>) -> Vec<u32> {
        let index = crate::lsp::position::LineIndex::new(text);
        let mut data = Vec::with_capacity(pieces.len() * 5);
        let (mut prev_line, mut prev_col) = (0u32, 0u32);
        for p in pieces {
            // tokens may not span lines: split at newlines
            let mut lo = p.lo;
            while lo < p.hi {
                let line_end = text[lo..p.hi].find('\n').map_or(p.hi, |i| lo + i);
                let end = if text[lo..line_end].ends_with('\r') { line_end - 1 } else { line_end };
                if end > lo {
                    let (line, col) = index.position(lo);
                    let (_, end_col) = index.position(end);
                    let len = end_col - col;
                    let delta_line = line - prev_line;
                    let delta_col = if delta_line == 0 { col - prev_col } else { col };
                    data.extend_from_slice(&[delta_line, delta_col, len, p.ty, p.modifiers]);
                    prev_line = line;
                    prev_col = col;
                }
                lo = line_end + 1;
            }
        }
        data
    }
}

/// Finds `// ...` and `/* ... */` comments in `text[from..to]` (a gap between tokens).
fn scan_comments(text: &str, from: usize, to: usize, out: &mut Vec<Piece>) {
    let bytes = text.as_bytes();
    let mut i = from;
    while i + 1 < to {
        if bytes[i] == b'/' && bytes[i + 1] == b'/' {
            let end = text[i..to].find('\n').map_or(to, |n| i + n);
            out.push(Piece { lo: i, hi: end, ty: T_COMMENT, modifiers: 0 });
            i = end;
        } else if bytes[i] == b'/' && bytes[i + 1] == b'*' {
            // block comments may nest
            let mut depth = 1;
            let mut j = i + 2;
            while j < to && depth > 0 {
                if text[j..to].starts_with("/*") {
                    depth += 1;
                    j += 2;
                } else if text[j..to].starts_with("*/") {
                    depth -= 1;
                    j += 2;
                } else {
                    j += text[j..].chars().next().map_or(1, char::len_utf8);
                }
            }
            out.push(Piece { lo: i, hi: j.min(to), ty: T_COMMENT, modifiers: 0 });
            i = j;
        } else {
            i += 1;
        }
    }
}
