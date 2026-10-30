//! The semantic model of a whole program: definitions, scopes and the typing
//! results the later phases consume.

use crate::fx::FxHashMap as HashMap;
use std::sync::Arc;

use super::ids::*;
use super::types::{Ty, TypeTable};
use crate::builtin::Builtin;
use crate::diag::{FileId, Span};
use crate::syntax::{Ast, Interner, NodeId, Symbol};

// ---- generics and traits ---------------------------------------------------

/// `Trait<args>` applied to some type (the type itself is implicit).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TraitRef {
    pub trait_id: TraitId,
    pub args: Vec<Ty>,
}

#[derive(Debug, Clone)]
pub struct GenericParam {
    pub name: Symbol,
    pub def: Option<DefId>,
    pub bounds: Vec<TraitRef>,
    pub span: Span,
}

/// The flattened generic parameters of an item. A method lists its owner's
/// parameters first: an impl's own, or `[Self, trait params..]` for a trait.
#[derive(Debug, Clone, Default)]
pub struct Generics {
    pub params: Vec<GenericParam>,
}

impl Generics {
    pub fn len(&self) -> usize {
        self.params.len()
    }

    pub fn is_empty(&self) -> bool {
        self.params.is_empty()
    }
}

// ---- definitions --------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FnOwner {
    Free,
    Impl(ImplId),
    Trait(TraitId),
    /// The implicit function holding a module's top-level statements.
    ModuleInit,
}

#[derive(Debug)]
pub struct FnDef {
    pub name: Symbol,
    pub module: ModuleId,
    pub def: DefId,
    /// The `FnDecl` node (or the `Module` node for a module init).
    pub node: NodeId,
    pub name_span: Span,
    pub owner: FnOwner,
    pub generics: Generics,
    /// Parameter types including the receiver.
    pub params: Vec<Ty>,
    pub ret: Ty,
    pub has_self: bool,
    pub body: Option<NodeId>,
    pub exported: bool,
    /// Parameter locals, filled in when the body is checked.
    pub param_locals: Vec<LocalId>,
}

#[derive(Debug)]
pub struct FieldDef {
    pub name: Symbol,
    pub ty: Ty,
    pub span: Span,
    pub def: DefId,
}

#[derive(Debug)]
pub struct VariantDef {
    pub name: Symbol,
    pub fields: Vec<Ty>,
    pub span: Span,
    pub def: DefId,
}

#[derive(Debug)]
pub enum AdtKind {
    Struct { fields: Vec<FieldDef> },
    Enum { variants: Vec<VariantDef> },
}

#[derive(Debug)]
pub struct AdtDef {
    pub name: Symbol,
    pub module: ModuleId,
    pub def: DefId,
    pub node: NodeId,
    pub name_span: Span,
    pub generics: Generics,
    pub kind: AdtKind,
    pub exported: bool,
}

impl AdtDef {
    pub fn is_enum(&self) -> bool {
        matches!(self.kind, AdtKind::Enum { .. })
    }

    pub fn variants(&self) -> &[VariantDef] {
        match &self.kind {
            AdtKind::Enum { variants } => variants,
            AdtKind::Struct { .. } => &[],
        }
    }

    pub fn fields(&self) -> &[FieldDef] {
        match &self.kind {
            AdtKind::Struct { fields } => fields,
            AdtKind::Enum { .. } => &[],
        }
    }
}

#[derive(Debug)]
pub struct TraitDef {
    pub name: Symbol,
    pub module: ModuleId,
    pub def: DefId,
    pub node: NodeId,
    pub name_span: Span,
    /// `[Self, trait params..]`.
    pub generics: Generics,
    pub methods: Vec<FnId>,
    pub exported: bool,
}

#[derive(Debug)]
pub struct ImplDef {
    pub module: ModuleId,
    pub node: NodeId,
    pub span: Span,
    pub generics: Generics,
    pub trait_ref: Option<TraitRef>,
    pub self_ty: Ty,
    pub methods: Vec<FnId>,
}

#[derive(Debug)]
pub struct GlobalDef {
    pub name: Symbol,
    pub module: ModuleId,
    pub def: DefId,
    pub name_span: Span,
    pub ty: Ty,
    pub mutable: bool,
    pub is_const: bool,
    pub exported: bool,
}

#[derive(Debug)]
pub struct LocalDef {
    pub name: Symbol,
    pub module: ModuleId,
    pub def: DefId,
    pub span: Span,
    pub ty: Ty,
    pub mutable: bool,
    /// The function or closure node whose frame holds this variable.
    pub owner: NodeId,
    /// Captured by a closure (an upvalue).
    pub captured: bool,
}

// ---- scopes -------------------------------------------------------------------

/// A named top-level entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Fn(FnId),
    Adt(AdtId),
    Trait(TraitId),
    Global(GlobalId),
}

#[derive(Debug, Default)]
pub struct ModuleScope {
    pub items: HashMap<Symbol, Item>,
    pub exports: HashMap<Symbol, Item>,
    /// `import "path" as alias;`
    pub aliases: HashMap<Symbol, ModuleId>,
    /// The definition record of each alias, for find-references.
    pub alias_defs: HashMap<Symbol, DefId>,
}

// ---- typing results --------------------------------------------------------------

/// What a call, method call or operator invokes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Callee {
    /// A function or inherent method; `args` are its flattened generic arguments.
    Fn(FnId, Vec<Ty>),
    /// A trait method declaration; `args` = `[Self, trait args.., method args..]`.
    /// The trait solver picks the impl once the arguments are concrete.
    Trait(FnId, Vec<Ty>),
    /// An intrinsic; `args` holds the receiver (or first argument) type.
    Builtin(Builtin, Vec<Ty>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConstVal {
    Int(i128),
    Float(f64),
}

/// What an identifier or path expression denotes.
#[derive(Debug, Clone, PartialEq)]
pub enum Res {
    Local(LocalId),
    Global(GlobalId),
    /// A function used as a value or callee.
    Callee(Callee),
    /// An enum variant constructor or unit variant.
    Variant {
        adt: AdtId,
        idx: u32,
        args: Vec<Ty>,
    },
    /// A compile-time constant such as `i32::MAX`.
    Const(ConstVal),
}

#[derive(Debug, Clone)]
pub struct ClosureInfo {
    pub params: Vec<LocalId>,
    /// Variables of enclosing functions this closure uses (deduplicated).
    pub captures: Vec<LocalId>,
    pub ret: Ty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IterKind {
    Range,
    Array,
    Set,
    MapKeys,
}

/// Per-module typing results, indexed by that module's `NodeId`s.
#[derive(Debug, Default)]
pub struct ModuleInfo {
    /// Type of every expression and pattern node.
    pub node_types: Vec<Ty>,
    pub res: HashMap<NodeId, Res>,
    /// `Callee` of method calls and overloaded operators.
    pub callees: HashMap<NodeId, Callee>,
    /// `PatBinding` / `Param` / closure-param node to its variable.
    pub binding_locals: HashMap<NodeId, LocalId>,
    /// `Field`, `FieldInit` node to the field index.
    pub field_indices: HashMap<NodeId, u32>,
    /// `sys.args` expressions (`Field` nodes whose base is the built-in `sys`).
    pub sys_args: crate::fx::FxHashSet<NodeId>,
    /// `PatVariant` node to its variant.
    pub pat_variants: HashMap<NodeId, (AdtId, u32)>,
    pub closures: HashMap<NodeId, ClosureInfo>,
    pub iter_kinds: HashMap<NodeId, IterKind>,
    /// Module-level `let`/`const` node to its global.
    pub globals: HashMap<NodeId, GlobalId>,
}

#[derive(Debug)]
pub struct Module {
    pub id: ModuleId,
    pub path: String,
    pub file: FileId,
    pub ast: Arc<Ast>,
    pub scope: ModuleScope,
    pub info: ModuleInfo,
    pub init: FnId,
    pub imports: Vec<(NodeId, ModuleId)>,
}

// ---- definition/reference table ------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefKind {
    Module(ModuleId),
    Fn(FnId),
    Adt(AdtId),
    Variant(AdtId, u32),
    Field(AdtId, u32),
    Trait(TraitId),
    Global(GlobalId),
    Local(LocalId),
    GenericParam,
    /// A name brought in by `import { name }` (resolves to the imported item).
    ImportName,
}

#[derive(Debug, Clone)]
pub struct Def {
    pub name: Symbol,
    pub kind: DefKind,
    /// Span of the defining name.
    pub span: Span,
    pub module: ModuleId,
}

/// A reference to a definition at a source span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Use {
    pub span: Span,
    pub def: DefId,
}

/// Links source spans to definitions: the data behind "go to definition",
/// "find references" and rename.
#[derive(Debug, Default)]
pub struct SymbolTable {
    pub defs: Vec<Def>,
    pub uses: Vec<Use>,
}

impl SymbolTable {
    pub fn add_def(&mut self, name: Symbol, kind: DefKind, span: Span, module: ModuleId) -> DefId {
        self.defs.push(Def { name, kind, span, module });
        DefId(self.defs.len() as u32 - 1)
    }

    pub fn add_use(&mut self, span: Span, def: DefId) {
        if !span.is_dummy() {
            self.uses.push(Use { span, def });
        }
    }

    pub fn def(&self, id: DefId) -> &Def {
        &self.defs[id.index()]
    }

    /// The definition referenced at `offset` of `file`, or defined there.
    pub fn def_at(&self, file: FileId, offset: u32) -> Option<DefId> {
        if let Some(u) = self.uses.iter().find(|u| u.span.file == file && u.span.contains(offset)) {
            return Some(u.def);
        }
        self.defs.iter().position(|d| d.span.file == file && d.span.contains(offset)).map(|i| DefId(i as u32))
    }

    /// Every use of `def`.
    pub fn uses_of(&self, def: DefId) -> impl Iterator<Item = &Use> {
        self.uses.iter().filter(move |u| u.def == def)
    }
}

// ---- prelude items -----------------------------------------------------------------

/// Which operator (or formatting) trait a `TraitId` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LangTrait {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Neg,
    Not,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Eq,
    Ord,
    Display,
}

impl LangTrait {
    pub const ALL: [(LangTrait, &'static str); 15] = [
        (LangTrait::Add, "Add"),
        (LangTrait::Sub, "Sub"),
        (LangTrait::Mul, "Mul"),
        (LangTrait::Div, "Div"),
        (LangTrait::Rem, "Rem"),
        (LangTrait::Neg, "Neg"),
        (LangTrait::Not, "Not"),
        (LangTrait::BitAnd, "BitAnd"),
        (LangTrait::BitOr, "BitOr"),
        (LangTrait::BitXor, "BitXor"),
        (LangTrait::Shl, "Shl"),
        (LangTrait::Shr, "Shr"),
        (LangTrait::Eq, "Eq"),
        (LangTrait::Ord, "Ord"),
        (LangTrait::Display, "Display"),
    ];
}

/// Traits the checker consults for operator overloading, found by name in
/// the prelude.
#[derive(Debug, Default, Clone)]
pub struct LangItems {
    pub by_kind: HashMap<LangTrait, TraitId>,
    pub kinds: HashMap<TraitId, LangTrait>,
}

impl LangItems {
    pub fn get(&self, k: LangTrait) -> Option<TraitId> {
        self.by_kind.get(&k).copied()
    }

    pub fn kind_of(&self, t: TraitId) -> Option<LangTrait> {
        self.kinds.get(&t).copied()
    }
}

/// The prelude's `Option` and `Result` enums, which fallible builtins return.
#[derive(Debug, Default, Clone, Copy)]
pub struct LangAdts {
    pub option: Option<AdtId>,
    pub result: Option<AdtId>,
}

// ---- the program ----------------------------------------------------------------------

/// Which impls exist, for method and trait lookup.
#[derive(Debug, Default)]
pub struct ImplIndex {
    pub by_trait: HashMap<TraitId, Vec<ImplId>>,
    pub inherent: Vec<ImplId>,
}

#[derive(Debug, Default)]
pub struct Program {
    pub types: TypeTable,
    pub modules: Vec<Module>,
    pub fns: Vec<FnDef>,
    pub adts: Vec<AdtDef>,
    pub traits: Vec<TraitDef>,
    pub impls: Vec<ImplDef>,
    pub globals: Vec<GlobalDef>,
    pub locals: Vec<LocalDef>,
    pub symbols: SymbolTable,
    pub lang: LangItems,
    pub lang_adts: LangAdts,
    pub impl_index: ImplIndex,
}

impl Program {
    pub fn module(&self, id: ModuleId) -> &Module {
        &self.modules[id.index()]
    }

    pub fn func(&self, id: FnId) -> &FnDef {
        &self.fns[id.index()]
    }

    pub fn adt(&self, id: AdtId) -> &AdtDef {
        &self.adts[id.index()]
    }

    pub fn trait_def(&self, id: TraitId) -> &TraitDef {
        &self.traits[id.index()]
    }

    pub fn impl_def(&self, id: ImplId) -> &ImplDef {
        &self.impls[id.index()]
    }

    pub fn global(&self, id: GlobalId) -> &GlobalDef {
        &self.globals[id.index()]
    }

    pub fn local(&self, id: LocalId) -> &LocalDef {
        &self.locals[id.index()]
    }

    /// The entry module (the last one loaded).
    pub fn entry(&self) -> Option<&Module> {
        self.modules.last()
    }

    /// Renders a type for diagnostics. `generics` names `Param`s (pass the
    /// generics of the item being described, if any).
    pub fn show_ty(&self, interner: &Interner, ty: Ty, generics: &Generics) -> String {
        self.types.display(
            ty,
            &|a| interner.resolve(self.adts[a.index()].name).to_string(),
            &|i| match generics.params.get(i as usize) {
                Some(p) => interner.resolve(p.name).to_string(),
                None => format!("T{i}"),
            },
            &|v| format!("?{}", v.0),
        )
    }

    pub fn find_item(&self, module: ModuleId, name: Symbol) -> Option<Item> {
        self.modules[module.index()].scope.items.get(&name).copied()
    }
}

impl Program {
    /// The definition record of a top-level item.
    pub fn item_def(&self, item: Item) -> DefId {
        match item {
            Item::Fn(f) => self.fns[f.index()].def,
            Item::Adt(a) => self.adts[a.index()].def,
            Item::Trait(t) => self.traits[t.index()].def,
            Item::Global(g) => self.globals[g.index()].def,
        }
    }

    /// Looks `name` up in `module`'s scope, then in the prelude's exports.
    pub fn lookup_item(&self, module: ModuleId, name: Symbol) -> Option<Item> {
        self.modules[module.index()]
            .scope
            .items
            .get(&name)
            .or_else(|| self.modules.first().and_then(|p| p.scope.exports.get(&name)))
            .copied()
    }
}
