//! Lowering of the typed AST to IR, with monomorphization.
//!
//! Every IR function is an *instance*: a source function (or closure) plus
//! concrete generic arguments. Lowering substitutes those arguments into the
//! types sema recorded, resolves trait-method calls to concrete impls with
//! the trait solver, and queues every callee instance it meets, so the output
//! contains no type parameters.

mod calls;
mod ctrl;
mod expr;
mod func;
mod pat;
mod place;
mod stmt;

use crate::fx::FxHashMap as HashMap;
use std::collections::VecDeque;

use super::types::*;
use crate::diag::{DiagBag, Diagnostic};
use crate::runtime::NumTy;
use crate::sema::{AdtKind, FnId, GlobalId, ModuleId, Prim, Program, Ty, TyKind};
use crate::syntax::{Interner, NodeId};

/// Instances beyond this many indicate runaway polymorphic recursion.
const MAX_INSTANCES: usize = 20_000;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum FuncKey {
    Fn(FnId, Vec<Ty>),
    /// A closure expression inside an instance with the given arguments.
    Closure(ModuleId, NodeId, Vec<Ty>),
}

pub(super) struct Lowerer<'a> {
    pub prog: &'a mut Program,
    pub interner: &'a Interner,
    pub diags: &'a mut DiagBag,
    pub out: IrProgram,
    funcs_by_key: HashMap<FuncKey, FuncId>,
    queue: VecDeque<(FuncId, FuncKey)>,
    descs: HashMap<Ty, DescId>,
    strings: HashMap<String, u32>,
    globals: HashMap<GlobalId, GlobalSlot>,
    /// Evaluated `const`s (`None` once evaluation failed, so the error is reported once).
    consts: HashMap<GlobalId, ConstState>,
    pub failed: bool,
}

#[derive(Debug, Clone, Copy)]
enum ConstState {
    InProgress,
    Done(Option<ConstResult>),
}

/// A compile-time constant, ready to be materialized at its uses.
#[derive(Debug, Clone, Copy)]
pub(super) enum ConstResult {
    Bits(u64),
    Str(u32),
}

/// Lowers a checked program. Returns `None` (with diagnostics) if lowering
/// fails; sema must have reported no errors beforehand.
pub fn lower_program(prog: &mut Program, interner: &Interner, diags: &mut DiagBag) -> Option<IrProgram> {
    let mut cx = Lowerer {
        prog,
        interner,
        diags,
        out: IrProgram::default(),
        funcs_by_key: HashMap::default(),
        queue: VecDeque::new(),
        descs: HashMap::default(),
        strings: HashMap::default(),
        globals: HashMap::default(),
        consts: HashMap::default(),
        failed: false,
    };
    // Evaluate every constant up front, so a broken one is reported even if nothing uses it.
    for i in 0..cx.prog.globals.len() {
        if cx.prog.globals[i].is_const {
            cx.const_value(GlobalId(i as u32));
        }
    }
    // Reserve `<start>` first so it has a stable id; it is filled in below.
    let start = cx.reserve_entry();
    let mut calls: Vec<FuncId> = Vec::new();
    let module_count = cx.prog.modules.len();
    for m in 0..module_count {
        let init = cx.prog.modules[m].init;
        calls.push(cx.func_for(FuncKey::Fn(init, vec![])));
    }
    // `main()` of the entry module runs after all top-level code.
    if let Some(main) = cx.find_main() {
        calls.push(cx.func_for(FuncKey::Fn(main, vec![])));
    }
    cx.build_entry(start, &calls);

    while let Some((id, key)) = cx.queue.pop_front() {
        if cx.failed {
            break;
        }
        func::lower_instance(&mut cx, id, key);
    }
    if cx.failed {
        return None;
    }
    cx.out.entry = start;
    Some(cx.out)
}

impl Lowerer<'_> {
    pub fn error(&mut self, message: impl Into<String>) {
        self.failed = true;
        self.diags.report(Diagnostic::error(message.into()).with_code("E0400"));
    }

    /// An error attached to a source location.
    pub fn error_at(&mut self, span: crate::diag::Span, code: &'static str, message: impl Into<String>) {
        self.failed = true;
        self.diags.report(Diagnostic::error(message.into()).with_code(code).with_label(span, "in this constant"));
    }

    /// The value of the `const` `g`, evaluated by the compiler (once).
    pub(super) fn const_value(&mut self, g: GlobalId) -> Option<ConstResult> {
        match self.consts.get(&g) {
            Some(ConstState::Done(r)) => return *r,
            Some(ConstState::InProgress) => {
                let (name, span) = {
                    let d = &self.prog.globals[g.index()];
                    (self.interner.resolve(d.name).to_string(), d.name_span)
                };
                self.error_at(span, "E0402", format!("constant `{name}` is defined in terms of itself"));
                self.consts.insert(g, ConstState::Done(None));
                return None;
            }
            None => {}
        }
        self.consts.insert(g, ConstState::InProgress);
        let result = self.eval_const_global(g);
        self.consts.insert(g, ConstState::Done(result));
        result
    }

    fn eval_const_global(&mut self, g: GlobalId) -> Option<ConstResult> {
        use crate::syntax::NodeKind;
        let (module, name_span, name) = {
            let d = &self.prog.globals[g.index()];
            (d.module, d.name_span, self.interner.resolve(d.name).to_string())
        };
        // the declaration node of this global
        let decl = self.prog.modules[module.index()].info.globals.iter().find(|(_, &gid)| gid == g).map(|(&n, _)| n)?;
        let ast = self.prog.modules[module.index()].ast.clone();
        let NodeKind::ConstDecl { init, .. } = *ast.kind(decl) else { return None };
        let func = func::lower_const_init(self, module, init);
        let mut prog = IrProgram { funcs: vec![func], ..IrProgram::default() };
        prog.strings = self.out.strings.clone();
        prog.descs = self.out.descs.clone();
        match crate::ir::interp::eval_const(&prog) {
            Ok(crate::ir::interp::ConstValue::Bits(b)) => Some(ConstResult::Bits(b)),
            Ok(crate::ir::interp::ConstValue::Str(s)) => Some(ConstResult::Str(self.string(&s))),
            Err(trap) => {
                self.error_at(name_span, "E0401", format!("cannot evaluate constant `{name}` at compile time: {trap}"));
                None
            }
        }
    }

    fn reserve_entry(&mut self) -> FuncId {
        let mut f = IrFunc::new("<start>");
        f.ret = IrTy::Unit;
        self.out.funcs.push(f);
        FuncId(self.out.funcs.len() as u32 - 1)
    }

    fn find_main(&mut self) -> Option<FnId> {
        let entry = self.prog.modules.last()?;
        let sym = self.interner.get("main")?;
        match entry.scope.items.get(&sym) {
            Some(crate::sema::Item::Fn(f)) if self.prog.fns[f.index()].params.is_empty() => Some(*f),
            _ => None,
        }
    }

    fn build_entry(&mut self, start: FuncId, calls: &[FuncId]) {
        let f = &mut self.out.funcs[start.index()];
        let b = f.new_block();
        let unit = f.new_reg(IrTy::Unit);
        f.blocks[b.index()].insts.push(Inst::Const { dst: unit, val: Const::Unit });
        for &c in calls {
            let dst = f.new_reg(IrTy::Unit);
            f.blocks[b.index()].insts.push(Inst::Call { dst, func: c, args: vec![] });
        }
        f.blocks[b.index()].term = Term::Return(unit);
    }

    // ---- types ---------------------------------------------------------------------------------

    /// Machine type of a concrete semantic type.
    pub fn ir_ty(&self, ty: Ty) -> IrTy {
        match self.prog.types.kind(ty) {
            TyKind::Prim(Prim::Bool) => IrTy::Bool,
            TyKind::Prim(Prim::Char) => IrTy::Char,
            TyKind::Prim(Prim::Str) => IrTy::Ref,
            TyKind::Prim(p) => IrTy::Num(NumTy::from_prim(*p).expect("numeric primitive")),
            TyKind::Unit | TyKind::Never | TyKind::Error => IrTy::Unit,
            TyKind::Array(_) | TyKind::Set(_) | TyKind::Map(..) | TyKind::Fn(..) | TyKind::Adt(..) => IrTy::Ref,
            TyKind::Param(_) | TyKind::Infer(_) => IrTy::Unit,
        }
    }

    /// Run-time descriptor of a concrete type (created on first use; recursive
    /// types are handled by registering the id before visiting components).
    pub fn desc(&mut self, ty: Ty) -> DescId {
        if let Some(&d) = self.descs.get(&ty) {
            return d;
        }
        let id = DescId(self.out.descs.len() as u32);
        self.out.descs.push(TypeDesc::Unit);
        self.descs.insert(ty, id);
        let desc = match self.prog.types.kind(ty).clone() {
            TyKind::Prim(Prim::Bool) => TypeDesc::Bool,
            TyKind::Prim(Prim::Char) => TypeDesc::Char,
            TyKind::Prim(Prim::Str) => TypeDesc::Str,
            TyKind::Prim(p) => TypeDesc::Num(NumTy::from_prim(p).expect("numeric primitive")),
            TyKind::Unit | TyKind::Never | TyKind::Error | TyKind::Param(_) | TyKind::Infer(_) => TypeDesc::Unit,
            TyKind::Array(e) => TypeDesc::Array(self.desc(e)),
            TyKind::Set(e) => TypeDesc::Set(self.desc(e)),
            TyKind::Map(k, v) => TypeDesc::Map(self.desc(k), self.desc(v)),
            TyKind::Fn(..) => TypeDesc::Fn,
            TyKind::Adt(adt, args) => {
                let name = self.prog.show_ty(self.interner, ty, &Default::default());
                let short = self.interner.resolve(self.prog.adts[adt.index()].name).to_string();
                let kind_fields: Vec<(String, Vec<Ty>)> = match &self.prog.adts[adt.index()].kind {
                    AdtKind::Struct { fields } => {
                        fields.iter().map(|f| (self.interner.resolve(f.name).to_string(), vec![f.ty])).collect()
                    }
                    AdtKind::Enum { variants } => {
                        variants.iter().map(|v| (self.interner.resolve(v.name).to_string(), v.fields.clone())).collect()
                    }
                };
                let is_enum = self.prog.adts[adt.index()].is_enum();
                let mut out: Vec<(String, Vec<DescId>)> = Vec::new();
                for (n, tys) in kind_fields {
                    let mut ds = Vec::new();
                    for t in tys {
                        let t = self.prog.types.subst(t, &args);
                        ds.push(self.desc(t));
                    }
                    out.push((n, ds));
                }
                if is_enum {
                    TypeDesc::Enum { name: short, variants: out }
                } else {
                    let fields = out.into_iter().map(|(n, d)| (n, d[0])).collect();
                    let _ = name;
                    TypeDesc::Struct { name: short, fields }
                }
            }
        };
        self.out.descs[id.index()] = desc;
        id
    }

    pub fn string(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.strings.get(s) {
            return i;
        }
        let i = self.out.strings.len() as u32;
        self.out.strings.push(s.to_string());
        self.strings.insert(s.to_string(), i);
        i
    }

    pub fn global_slot(&mut self, g: GlobalId) -> GlobalSlot {
        if let Some(&s) = self.globals.get(&g) {
            return s;
        }
        let (name, ty) = {
            let d = &self.prog.globals[g.index()];
            (self.interner.resolve(d.name).to_string(), d.ty)
        };
        let ir_ty = self.ir_ty(ty);
        let desc = self.desc(ty);
        let slot = GlobalSlot(self.out.globals.len() as u32);
        self.out.globals.push(GlobalInfo { name, ty: ir_ty, desc });
        self.globals.insert(g, slot);
        slot
    }

    // ---- instances ---------------------------------------------------------------------------------

    /// The IR function for `key`, reserving it (and queueing its body) on first use.
    pub fn func_for(&mut self, key: FuncKey) -> FuncId {
        if let Some(&id) = self.funcs_by_key.get(&key) {
            return id;
        }
        if self.out.funcs.len() >= MAX_INSTANCES {
            self.error("too many generic instantiations (runaway polymorphic recursion?)");
            return FuncId(0);
        }
        let skeleton = func::skeleton(self, &key);
        self.out.funcs.push(skeleton);
        let id = FuncId(self.out.funcs.len() as u32 - 1);
        self.funcs_by_key.insert(key.clone(), id);
        self.queue.push_back((id, key));
        id
    }
}
