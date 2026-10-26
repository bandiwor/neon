//! Flat, arena-allocated syntax tree.
//!
//! Every node lives in one `Vec<AstNode>` and is addressed by [`NodeId`].
//! There is no `Box<Node>`: a child is a `NodeId`, an optional child is an
//! [`OptionNodeId`] (sentinel `u32::MAX`), and a variable-length child list is
//! a [`NodeList`], a range into one shared `Vec<NodeId>`. Parents are always
//! allocated after their children, so a node's id is greater than all of its
//! descendants' ids.

use crate::fx::FxHashMap as HashMap;
use std::fmt;

use super::interner::Symbol;
use super::literal::{FloatId, IntId, LitPool, StrId};
use super::token::Keyword;
use crate::diag::{FileId, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub u32);

/// A `NodeId` that may be absent; the absent value is `u32::MAX`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct OptionNodeId(u32);

impl OptionNodeId {
    pub const NONE: OptionNodeId = OptionNodeId(u32::MAX);

    pub fn some(id: NodeId) -> OptionNodeId {
        debug_assert!(id.0 != u32::MAX);
        OptionNodeId(id.0)
    }

    pub fn get(self) -> Option<NodeId> {
        (self.0 != u32::MAX).then_some(NodeId(self.0))
    }

    pub fn is_none(self) -> bool {
        self.0 == u32::MAX
    }

    pub fn is_some(self) -> bool {
        !self.is_none()
    }
}

impl From<NodeId> for OptionNodeId {
    fn from(id: NodeId) -> Self {
        OptionNodeId::some(id)
    }
}

impl From<Option<NodeId>> for OptionNodeId {
    fn from(id: Option<NodeId>) -> Self {
        id.map_or(OptionNodeId::NONE, OptionNodeId::some)
    }
}

impl fmt::Debug for OptionNodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.get() {
            Some(id) => write!(f, "Some({})", id.0),
            None => f.write_str("None"),
        }
    }
}

/// A run of child ids stored in [`Ast`]'s shared list buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeList {
    start: u32,
    len: u32,
}

impl NodeList {
    pub const EMPTY: NodeList = NodeList { start: 0, len: 0 };

    pub fn len(self) -> usize {
        self.len as usize
    }

    pub fn is_empty(self) -> bool {
        self.len == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnOp {
    Neg,
    Not,
    BitNot,
}

impl UnOp {
    pub fn as_str(self) -> &'static str {
        match self {
            UnOp::Neg => "-",
            UnOp::Not => "!",
            UnOp::BitNot => "~",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    And,
    Or,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl BinOp {
    pub fn as_str(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::BitAnd => "&",
            BinOp::BitOr => "|",
            BinOp::BitXor => "^",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
            BinOp::And => "&&",
            BinOp::Or => "||",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
        }
    }

    pub fn is_comparison(self) -> bool {
        matches!(self, BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge)
    }

    pub fn is_logical(self) -> bool {
        matches!(self, BinOp::And | BinOp::Or)
    }
}

/// What a node is. Child references are ids into the same [`Ast`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NodeKind {
    // ---- module and declarations -------------------------------------
    /// Top-level items and statements, in source order.
    Module {
        items: NodeList,
    },
    /// `import { a, b as c } from "path";` (`names` are `ImportName`s)
    ImportNamed {
        names: NodeList,
        path: StrId,
    },
    ImportName {
        name: Symbol,
        alias: Option<Symbol>,
    },
    /// `import "path" as alias;`
    ImportAlias {
        path: StrId,
        alias: Symbol,
    },
    /// `export <declaration>`
    Export {
        item: NodeId,
    },
    /// `body` is absent for a signature in a trait.
    FnDecl {
        name: Symbol,
        generics: NodeList,
        params: NodeList,
        ret: OptionNodeId,
        body: OptionNodeId,
    },
    /// `T: Bound + Bound` (`bounds` are type paths)
    GenericParam {
        name: Symbol,
        bounds: NodeList,
    },
    /// A parameter; `name` is the `self` keyword symbol for a receiver.
    Param {
        name: Symbol,
        mutable: bool,
        ty: OptionNodeId,
    },
    StructDecl {
        name: Symbol,
        generics: NodeList,
        fields: NodeList,
    },
    FieldDecl {
        name: Symbol,
        ty: NodeId,
    },
    EnumDecl {
        name: Symbol,
        generics: NodeList,
        variants: NodeList,
    },
    /// `fields` are payload types; empty for a unit variant.
    Variant {
        name: Symbol,
        fields: NodeList,
    },
    TraitDecl {
        name: Symbol,
        generics: NodeList,
        items: NodeList,
    },
    /// `impl<G> Trait for Type { .. }` or `impl Type { .. }`
    ImplDecl {
        generics: NodeList,
        trait_ref: OptionNodeId,
        self_ty: NodeId,
        items: NodeList,
    },
    ConstDecl {
        name: Symbol,
        ty: OptionNodeId,
        init: NodeId,
    },

    // ---- statements ----------------------------------------------------
    Let {
        pat: NodeId,
        ty: OptionNodeId,
        init: OptionNodeId,
    },
    /// `semi` is false for block-like statements and a block's tail position.
    ExprStmt {
        expr: NodeId,
        semi: bool,
    },

    // ---- expressions -----------------------------------------------------
    IntLit(IntId),
    FloatLit(FloatId),
    StrLit(StrId),
    /// `f"text {expr:spec}"`: the parts are `StrLit` text pieces and `FStrArg`s.
    FString {
        parts: NodeList,
    },
    /// One `{expr}` or `{expr:spec}` of a format string.
    FStrArg {
        expr: NodeId,
        /// The format specifier: `StrLit` text and `FStrArg`s (without a specifier of their own); empty when absent.
        spec: NodeList,
    },
    CharLit(char),
    BoolLit(bool),
    UnitLit,
    /// A single-segment path without generic arguments.
    Ident(Symbol),
    /// `expr?`: unwrap an `Option` / `Result`, or return the failure from the function.
    Try {
        expr: NodeId,
    },
    /// Any other path (`Option::Some`, `m::f`, `parse::<i32>`); elements are `PathSegment`s.
    Path {
        segments: NodeList,
    },
    PathSegment {
        name: Symbol,
        args: NodeList,
    },
    Unary {
        op: UnOp,
        operand: NodeId,
    },
    Binary {
        op: BinOp,
        lhs: NodeId,
        rhs: NodeId,
    },
    /// `target = value`, or `target op= value` when `op` is set.
    Assign {
        op: Option<BinOp>,
        target: NodeId,
        value: NodeId,
    },
    Cast {
        expr: NodeId,
        ty: NodeId,
    },
    Call {
        callee: NodeId,
        args: NodeList,
    },
    MethodCall {
        receiver: NodeId,
        name: Symbol,
        generics: NodeList,
        args: NodeList,
    },
    Field {
        base: NodeId,
        name: Symbol,
    },
    Index {
        base: NodeId,
        index: NodeId,
    },
    Range {
        start: OptionNodeId,
        end: OptionNodeId,
        inclusive: bool,
    },
    ArrayLit {
        elems: NodeList,
    },
    MapLit {
        entries: NodeList,
    },
    MapEntry {
        key: NodeId,
        value: NodeId,
    },
    SetLit {
        elems: NodeList,
    },
    /// `path` is an `Ident` or `Path`; shorthand fields are expanded.
    StructLit {
        path: NodeId,
        fields: NodeList,
    },
    FieldInit {
        name: Symbol,
        value: NodeId,
    },
    Closure {
        params: NodeList,
        ret: OptionNodeId,
        body: NodeId,
    },
    Block {
        stmts: NodeList,
        tail: OptionNodeId,
    },
    If {
        cond: NodeId,
        then_branch: NodeId,
        else_branch: OptionNodeId,
    },
    While {
        cond: NodeId,
        body: NodeId,
    },
    Loop {
        body: NodeId,
    },
    For {
        pat: NodeId,
        iter: NodeId,
        body: NodeId,
    },
    Match {
        scrutinee: NodeId,
        arms: NodeList,
    },
    MatchArm {
        pat: NodeId,
        guard: OptionNodeId,
        body: NodeId,
    },
    Return {
        value: OptionNodeId,
    },
    Break {
        value: OptionNodeId,
    },
    Continue,

    // ---- patterns -------------------------------------------------------
    PatWild,
    PatBinding {
        name: Symbol,
        mutable: bool,
    },
    /// A literal expression node (possibly a negated number).
    PatLit {
        expr: NodeId,
    },
    PatRange {
        lo: NodeId,
        hi: NodeId,
        inclusive: bool,
    },
    /// `Option::Some(x)`, `Shape::Unit` (`parens` false), or bare `Some(x)`.
    PatVariant {
        path: NodeId,
        fields: NodeList,
        parens: bool,
    },
    PatOr {
        alts: NodeList,
    },

    // ---- types ----------------------------------------------------------
    TyPrim(Keyword),
    /// Elements are `PathSegment`s (so `Option<T>` is one segment with one arg).
    TyPath {
        segments: NodeList,
    },
    TySelf,
    TyArray {
        elem: NodeId,
    },
    TyMap {
        key: NodeId,
        value: NodeId,
    },
    TySet {
        elem: NodeId,
    },
    TyFn {
        params: NodeList,
        ret: OptionNodeId,
    },
    TyUnit,

    /// Placeholder for source that failed to parse; its span covers the
    /// skipped text. Tooling can keep working around it.
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AstNode {
    pub kind: NodeKind,
    pub span: Span,
}

/// One parsed module: node arena, child lists and decoded literals.
#[derive(Debug)]
pub struct Ast {
    pub file: FileId,
    nodes: Vec<AstNode>,
    lists: Vec<NodeId>,
    pub lits: LitPool,
    root: NodeId,
    name_spans: HashMap<NodeId, Span>,
}

impl Ast {
    pub(crate) fn new(file: FileId, lits: LitPool) -> Ast {
        Ast { file, nodes: Vec::new(), lists: Vec::new(), lits, root: NodeId(u32::MAX), name_spans: HashMap::default() }
    }

    pub(crate) fn push(&mut self, kind: NodeKind, span: Span) -> NodeId {
        let id = NodeId(u32::try_from(self.nodes.len()).expect("more than u32::MAX AST nodes"));
        self.nodes.push(AstNode { kind, span });
        id
    }

    pub(crate) fn push_list(&mut self, ids: &[NodeId]) -> NodeList {
        if ids.is_empty() {
            return NodeList::EMPTY;
        }
        let start = u32::try_from(self.lists.len()).expect("more than u32::MAX AST list entries");
        self.lists.extend_from_slice(ids);
        NodeList { start, len: ids.len() as u32 }
    }

    pub(crate) fn set_name_span(&mut self, id: NodeId, span: Span) {
        self.name_spans.insert(id, span);
    }

    /// Span of the name a declaration introduces (`fn area`, a field, a
    /// parameter, ...); the whole node's span for nodes without one.
    pub fn name_span(&self, id: NodeId) -> Span {
        self.name_spans.get(&id).copied().unwrap_or_else(|| self.span(id))
    }

    pub(crate) fn set_root(&mut self, root: NodeId) {
        self.root = root;
    }

    /// The `Module` node.
    pub fn root(&self) -> NodeId {
        self.root
    }

    pub fn node(&self, id: NodeId) -> &AstNode {
        &self.nodes[id.0 as usize]
    }

    pub fn kind(&self, id: NodeId) -> &NodeKind {
        &self.nodes[id.0 as usize].kind
    }

    pub fn span(&self, id: NodeId) -> Span {
        self.nodes[id.0 as usize].span
    }

    pub fn list(&self, list: NodeList) -> &[NodeId] {
        &self.lists[list.start as usize..(list.start + list.len) as usize]
    }

    /// Number of nodes.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (NodeId, &AstNode)> {
        self.nodes.iter().enumerate().map(|(i, n)| (NodeId(i as u32), n))
    }

    /// The innermost node whose span contains byte `offset` (a position query
    /// for editors). Children are allocated before parents, so among nodes
    /// with equally small spans the lowest id is the deepest.
    pub fn node_at(&self, offset: u32) -> Option<NodeId> {
        let mut best: Option<(u32, NodeId)> = None;
        for (id, n) in self.iter() {
            if matches!(n.kind, NodeKind::Module { .. }) || !n.span.contains(offset) {
                continue;
            }
            let len = n.span.len();
            if best.is_none_or(|(l, _)| len < l) {
                best = Some((len, id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// Whether any node is an `Error` placeholder.
    pub fn has_errors(&self) -> bool {
        self.nodes.iter().any(|n| matches!(n.kind, NodeKind::Error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(lo: u32, hi: u32) -> Span {
        Span::new(FileId(0), lo, hi)
    }

    #[test]
    fn option_node_id_sentinel() {
        assert!(OptionNodeId::NONE.is_none());
        assert_eq!(OptionNodeId::NONE.get(), None);
        let some = OptionNodeId::some(NodeId(7));
        assert!(some.is_some());
        assert_eq!(some.get(), Some(NodeId(7)));
        assert_eq!(OptionNodeId::from(NodeId(0)).get(), Some(NodeId(0)));
        assert_eq!(OptionNodeId::from(None).get(), None);
        assert_eq!(format!("{:?} {:?}", some, OptionNodeId::NONE), "Some(7) None");
        assert_eq!(std::mem::size_of::<OptionNodeId>(), 4);
    }

    #[test]
    fn nodes_stay_compact() {
        // Flat storage only pays off if nodes stay small.
        assert!(std::mem::size_of::<NodeKind>() <= 36, "{}", std::mem::size_of::<NodeKind>());
        assert!(std::mem::size_of::<AstNode>() <= 48, "{}", std::mem::size_of::<AstNode>());
    }

    #[test]
    fn lists_are_ranges_into_one_buffer() {
        let mut ast = Ast::new(FileId(0), LitPool::new());
        let a = ast.push(NodeKind::UnitLit, span(0, 1));
        let b = ast.push(NodeKind::UnitLit, span(1, 2));
        let c = ast.push(NodeKind::UnitLit, span(2, 3));
        let l1 = ast.push_list(&[a, b]);
        let l2 = ast.push_list(&[c]);
        let empty = ast.push_list(&[]);
        assert_eq!(ast.list(l1), &[a, b]);
        assert_eq!(ast.list(l2), &[c]);
        assert!(ast.list(empty).is_empty() && empty.is_empty());
        assert_eq!((l1.len(), l2.len()), (2, 1));
    }

    #[test]
    fn node_at_finds_innermost() {
        let mut ast = Ast::new(FileId(0), LitPool::new());
        let inner = ast.push(NodeKind::UnitLit, span(4, 6));
        let outer = ast.push(NodeKind::Return { value: inner.into() }, span(0, 10));
        let list = ast.push_list(&[outer]);
        let root = ast.push(NodeKind::Module { items: list }, span(0, 10));
        ast.set_root(root);
        assert_eq!(ast.node_at(5), Some(inner));
        assert_eq!(ast.node_at(1), Some(outer));
        assert_eq!(ast.node_at(10), None);
        assert_eq!(ast.root(), root);
        assert!(!ast.has_errors());
    }

    #[test]
    fn same_span_prefers_child() {
        let mut ast = Ast::new(FileId(0), LitPool::new());
        let child = ast.push(NodeKind::UnitLit, span(0, 2));
        let parent = ast.push(NodeKind::Return { value: child.into() }, span(0, 2));
        assert_ne!(child, parent);
        assert_eq!(ast.node_at(0), Some(child));
    }

    #[test]
    fn operator_properties() {
        assert!(BinOp::Le.is_comparison() && !BinOp::Add.is_comparison());
        assert!(BinOp::And.is_logical() && !BinOp::BitAnd.is_logical());
        assert_eq!(BinOp::Shr.as_str(), ">>");
        assert_eq!(UnOp::BitNot.as_str(), "~");
    }
}
