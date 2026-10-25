//! Front end: interner, literal pools, lexer, flat AST and resilient parser.
//!
//! Identifiers are interned into [`Symbol`]s; numeric, character and string
//! literals are decoded by the lexer straight into a [`LitPool`], so tokens
//! stay small and `Copy`. [`parse_source`] turns text into an [`Ast`] stored
//! as one flat node arena; syntax errors are collected in a `DiagBag` and the
//! offending statements become `Error` nodes, so a tree always comes back.

mod ast;
mod interner;
mod lexer;
mod literal;
mod parser;
mod print;
mod token;

pub use ast::{Ast, AstNode, BinOp, NodeId, NodeKind, NodeList, OptionNodeId, UnOp};
pub use interner::{sym, Interner, Symbol};
pub use lexer::{tokenize, Lexer};
pub use literal::{FloatId, FloatLit, FloatSuffix, IntId, IntLit, IntSuffix, LitPool, StrId};
pub use parser::parse_source;
pub use print::{sexpr, sexpr_module};
pub use token::{Keyword, Token, TokenKind};
