//! Per-function lowering state and the entry points that lower one instance.

use std::collections::HashMap;
use std::sync::Arc;

use super::{FuncKey, Lowerer};
use crate::ir::types::*;
use crate::sema::{FnOwner, LocalId, ModuleId, Ty};
use crate::syntax::{Ast, NodeId, NodeKind};

/// Where a local variable lives in the function being lowered.
#[derive(Debug, Clone, Copy)]
pub(super) enum Slot {
    Reg(VReg),
    Up(u32),
}

pub(super) struct LoopCtx {
    pub break_to: BlockId,
    pub continue_to: BlockId,
    /// Number of open scopes when the loop began (scopes above it are exited by break/continue).
    pub scope_depth: usize,
    /// Receives `break value`s of a `loop` expression.
    pub result: Option<VReg>,
}

pub(super) struct FnLower<'l, 'a> {
    pub cx: &'l mut Lowerer<'a>,
    pub ast: Arc<Ast>,
    pub module: ModuleId,
    /// Generic arguments of the instance being lowered.
    pub args: Vec<Ty>,
    pub f: IrFunc,
    pub cur: BlockId,
    /// The current block already has its terminator.
    pub sealed: bool,
    pub locals: HashMap<LocalId, Slot>,
    pub scopes: Vec<Vec<VReg>>,
    pub loops: Vec<LoopCtx>,
    pub unit: Option<VReg>,
    /// The closure's captured variables, in upvalue order (empty otherwise).
    pub captures: Vec<LocalId>,
    /// The (instantiated) return type of the function, which `?` builds early returns of.
    pub ret_ty: Ty,
}

/// An `IrFunc` with its signature but no body, for call sites to refer to.
pub(super) fn skeleton(cx: &mut Lowerer<'_>, key: &FuncKey) -> IrFunc {
    match key {
        FuncKey::Fn(fid, args) => {
            let (params, ret, name) = {
                let f = &cx.prog.fns[fid.index()];
                (f.params.clone(), f.ret, cx.interner.resolve(f.name).to_string())
            };
            let mut ir = IrFunc::new(display_name(cx, &name, args));
            for p in params {
                let t = cx.prog.types.subst(p, args);
                let t = cx.ir_ty(t);
                ir.new_reg(t);
            }
            ir.params = ir.regs.len() as u32;
            let ret = cx.prog.types.subst(ret, args);
            ir.ret = cx.ir_ty(ret);
            ir
        }
        FuncKey::Closure(m, node, args) => {
            let info = cx.prog.modules[m.index()].info.closures[node].clone();
            let mut ir = IrFunc::new(format!("<closure@{}>", node.0));
            ir.is_closure = true;
            for l in &info.params {
                let t = cx.prog.locals[l.index()].ty;
                let t = cx.prog.types.subst(t, args);
                let t = cx.ir_ty(t);
                ir.new_reg(t);
            }
            ir.params = ir.regs.len() as u32;
            let ret = cx.prog.types.subst(info.ret, args);
            ir.ret = cx.ir_ty(ret);
            for l in &info.captures {
                let t = cx.prog.locals[l.index()].ty;
                let t = cx.prog.types.subst(t, args);
                ir.upvals.push(cx.ir_ty(t));
            }
            ir
        }
    }
}

fn display_name(cx: &Lowerer<'_>, name: &str, args: &[Ty]) -> String {
    if args.is_empty() {
        return name.to_string();
    }
    let shown: Vec<String> = args.iter().map(|&a| cx.prog.show_ty(cx.interner, a, &Default::default())).collect();
    format!("{name}<{}>", shown.join(", "))
}

/// Lowers the initializer `init` of a `const` of `module` into a stand-alone,
/// call-free function that returns its value.
pub(super) fn lower_const_init(cx: &mut Lowerer<'_>, module: ModuleId, init: NodeId) -> IrFunc {
    let ast = cx.prog.modules[module.index()].ast.clone();
    let mut fl = FnLower {
        cx,
        ast,
        module,
        args: Vec::new(),
        f: IrFunc::new("<const>"),
        cur: BlockId(0),
        sealed: false,
        locals: HashMap::new(),
        scopes: vec![Vec::new()],
        loops: Vec::new(),
        unit: None,
        captures: Vec::new(),
        ret_ty: crate::sema::Ty(0),
    };
    let ty = fl.node_ty(init);
    fl.ret_ty = ty;
    fl.f.ret = fl.cx.ir_ty(ty);
    let entry = fl.f.new_block();
    fl.cur = entry;
    let v = fl.expr(init);
    fl.finish_with(v);
    std::mem::replace(&mut fl.f, IrFunc::new(""))
}

/// Lowers the body of one reserved instance.
pub(super) fn lower_instance(cx: &mut Lowerer<'_>, id: FuncId, key: FuncKey) {
    // Keep the signature in place while lowering, so recursive calls can read it.
    let skeleton = cx.out.funcs[id.index()].clone();
    let module = match &key {
        FuncKey::Fn(f, _) => cx.prog.fns[f.index()].module,
        FuncKey::Closure(m, ..) => *m,
    };
    let ast = cx.prog.modules[module.index()].ast.clone();
    let args = match &key {
        FuncKey::Fn(_, a) | FuncKey::Closure(_, _, a) => a.clone(),
    };
    let ret_ty = match &key {
        FuncKey::Fn(f, a) => {
            let r = cx.prog.fns[f.index()].ret;
            cx.prog.types.subst(r, a)
        }
        FuncKey::Closure(m, node, a) => {
            let r = cx.prog.modules[m.index()].info.closures[node].ret;
            cx.prog.types.subst(r, a)
        }
    };
    let mut fl = FnLower {
        cx,
        ast,
        module,
        args,
        f: skeleton,
        cur: BlockId(0),
        sealed: false,
        locals: HashMap::new(),
        scopes: vec![Vec::new()],
        loops: Vec::new(),
        unit: None,
        captures: Vec::new(),
        ret_ty,
    };
    let entry = fl.f.new_block();
    fl.cur = entry;
    match key {
        FuncKey::Fn(fid, _) => fl.lower_fn(fid),
        FuncKey::Closure(_, node, _) => fl.lower_closure(node),
    }
    let done = std::mem::replace(&mut fl.f, IrFunc::new(""));
    fl.cx.out.funcs[id.index()] = done;
}

impl FnLower<'_, '_> {
    // ---- entry points ----------------------------------------------------------------------

    fn lower_fn(&mut self, fid: crate::sema::FnId) {
        let (owner, body, params) = {
            let f = &self.cx.prog.fns[fid.index()];
            (f.owner, f.body, f.param_locals.clone())
        };
        if owner == FnOwner::ModuleInit {
            let root = self.ast.root();
            self.lower_module_init(root);
            let unit = self.unit();
            self.finish_with(unit);
            return;
        }
        for (i, l) in params.iter().enumerate() {
            self.bind_param(*l, VReg(i as u32));
        }
        let body = body.expect("only functions with bodies are instantiated");
        let v = self.expr(body);
        let ty = self.node_ty(body);
        if matches!(self.cx.prog.types.kind(ty), crate::sema::TyKind::Never) {
            self.terminate(Term::Unreachable);
        } else {
            self.finish_with(v);
        }
    }

    fn lower_closure(&mut self, node: NodeId) {
        let info = self.cx.prog.modules[self.module.index()].info.closures[&node].clone();
        self.captures = info.captures.clone();
        for (i, l) in info.params.iter().enumerate() {
            self.bind_param(*l, VReg(i as u32));
        }
        let NodeKind::Closure { body, .. } = *self.ast.kind(node) else {
            unreachable!("closure instance of a non-closure node")
        };
        let v = self.expr(body);
        let ty = self.node_ty(body);
        if matches!(self.cx.prog.types.kind(ty), crate::sema::TyKind::Never) {
            self.terminate(Term::Unreachable);
        } else {
            self.finish_with(v);
        }
    }

    fn bind_param(&mut self, l: LocalId, reg: VReg) {
        self.locals.insert(l, Slot::Reg(reg));
        if self.cx.prog.locals[l.index()].captured {
            self.f.pinned[reg.index()] = true;
        }
    }

    /// Returns `v`, unless the block is already terminated or `v` has the wrong
    /// type (only possible in dead code after a diverging expression).
    fn finish_with(&mut self, v: VReg) {
        if self.sealed {
            return;
        }
        if self.f.reg_ty(v) == self.f.ret {
            self.terminate(Term::Return(v));
        } else {
            self.terminate(Term::Unreachable);
        }
    }

    // ---- blocks and registers ----------------------------------------------------------------------

    pub fn new_reg(&mut self, ty: IrTy) -> VReg {
        self.f.new_reg(ty)
    }

    pub fn new_block(&mut self) -> BlockId {
        self.f.new_block()
    }

    pub fn switch_to(&mut self, b: BlockId) {
        self.cur = b;
        self.sealed = false;
    }

    pub fn terminate(&mut self, t: Term) {
        if self.sealed {
            return;
        }
        self.f.blocks[self.cur.index()].term = t;
        self.sealed = true;
    }

    pub fn emit(&mut self, inst: Inst) {
        if self.sealed {
            // code after `return`/`break`: unreachable, parked in its own block
            let dead = self.new_block();
            self.switch_to(dead);
        }
        self.f.blocks[self.cur.index()].insts.push(inst);
    }

    pub fn jump(&mut self, to: BlockId) {
        self.terminate(Term::Jump(to));
    }

    /// The shared `()` value of this function.
    pub fn unit(&mut self) -> VReg {
        if let Some(u) = self.unit {
            return u;
        }
        let r = self.f.new_reg(IrTy::Unit);
        self.f.blocks[0].insts.insert(0, Inst::Const { dst: r, val: Const::Unit });
        self.unit = Some(r);
        r
    }

    pub fn const_bits(&mut self, ty: IrTy, bits: u64) -> VReg {
        let dst = self.new_reg(ty);
        self.emit(Inst::Const { dst, val: Const::Bits(bits) });
        dst
    }

    pub fn const_bool(&mut self, b: bool) -> VReg {
        self.const_bits(IrTy::Bool, b as u64)
    }

    // ---- types ---------------------------------------------------------------------------------------

    /// Concrete type of a sema-checked node in this instance.
    pub fn node_ty(&mut self, n: NodeId) -> Ty {
        let t = self.cx.prog.modules[self.module.index()].info.node_types[n.0 as usize];
        self.cx.prog.types.subst(t, &self.args)
    }

    pub fn subst(&mut self, t: Ty) -> Ty {
        self.cx.prog.types.subst(t, &self.args)
    }

    pub fn subst_all(&mut self, ts: &[Ty]) -> Vec<Ty> {
        ts.iter().map(|&t| self.cx.prog.types.subst(t, &self.args)).collect()
    }

    /// A fresh register typed like the value of node `n`.
    pub fn new_reg_for(&mut self, n: NodeId) -> VReg {
        let t = self.ir_ty_of_node(n);
        self.new_reg(t)
    }

    pub fn ir_ty_of_node(&mut self, n: NodeId) -> IrTy {
        let t = self.node_ty(n);
        self.cx.ir_ty(t)
    }

    // ---- scopes and locals -------------------------------------------------------------------------------

    pub fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    /// Leaves a scope, closing upvalues of captured variables declared in it.
    pub fn pop_scope(&mut self) {
        let regs = self.scopes.pop().unwrap_or_default();
        if !regs.is_empty() && !self.sealed {
            self.emit(Inst::CloseUpvals { regs });
        }
    }

    /// Emits `CloseUpvals` for every scope above `depth` without popping them
    /// (for `break` / `continue`).
    pub fn close_scopes_above(&mut self, depth: usize) {
        let regs: Vec<VReg> = self.scopes[depth..].iter().flatten().copied().collect();
        if !regs.is_empty() && !self.sealed {
            self.emit(Inst::CloseUpvals { regs });
        }
    }

    /// Creates the register for a new local variable.
    pub fn declare_local(&mut self, l: LocalId) -> VReg {
        let ty = self.cx.prog.locals[l.index()].ty;
        let ty = self.subst(ty);
        let ir = self.cx.ir_ty(ty);
        let reg = self.new_reg(ir);
        self.locals.insert(l, Slot::Reg(reg));
        if self.cx.prog.locals[l.index()].captured {
            self.f.pinned[reg.index()] = true;
            if let Some(s) = self.scopes.last_mut() {
                s.push(reg);
            }
        }
        reg
    }

    pub fn slot_of(&mut self, l: LocalId) -> Slot {
        if let Some(&s) = self.locals.get(&l) {
            return s;
        }
        match self.captures.iter().position(|&c| c == l) {
            Some(i) => Slot::Up(i as u32),
            None => {
                let name = self.cx.interner.resolve(self.cx.prog.locals[l.index()].name).to_string();
                self.cx.error(format!("internal error: variable `{name}` has no storage in `{}`", self.f.name));
                Slot::Reg(self.unit())
            }
        }
    }
}
