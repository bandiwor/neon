//! Front end: string interner, literal pools, tokens and the lexer.
//!
//! Identifiers are interned into [`Symbol`]s; numeric, character and string
//! literals are decoded by the lexer straight into a [`LitPool`], so tokens
//! stay small and `Copy`.

mod interner;
mod lexer;
mod literal;
mod token;

pub use interner::{sym, Interner, Symbol};
pub use lexer::{tokenize, Lexer};
pub use literal::{FloatId, FloatLit, FloatSuffix, IntId, IntLit, IntSuffix, LitPool, StrId};
pub use token::{Keyword, Token, TokenKind};
