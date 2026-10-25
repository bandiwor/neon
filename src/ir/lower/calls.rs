//! Calls, resolved callees and builtin operations.

use super::func::FnLower;
use super::FuncKey;
use crate::builtin::Builtin;
use crate::ir::types::*;
use crate::runtime::numeric::{ArithOp, CmpOp};
use crate::runtime::NumTy;
use crate::sema::{resolve_trait_method, Callee, LangTrait, Prim, Resolved, Ty, TyKind};
use crate::syntax::NodeId;

impl FnLower<'_, '_> {
    pub fn call_expr(&mut self, id: NodeId, callee: NodeId, args: &[NodeId]) -> VReg {
        match self.res(callee) {
            Some(crate::sema::Res::Callee(c)) => {
                let regs = self.operands(args);
                let first = args.first().map(|&a| self.node_ty(a));
                self.emit_callee(id, &c, regs, first)
            }
            Some(crate::sema::Res::Variant { idx, .. }) => {
                let regs = self.operands(args);
                let ty = self.node_ty(id);
                let desc = self.cx.desc(ty);
                let dst = self.new_reg(IrTy::Ref);
                self.emit(Inst::NewEnum { dst, desc, tag: idx, fields: regs });
                dst
            }
            _ => {
                let mut all = vec![callee];
                all.extend_from_slice(args);
                let regs = self.operands(&all);
                let dst = self.new_reg_for(id);
                self.emit(Inst::CallClosure { dst, callee: regs[0], args: regs[1..].to_vec() });
                dst
            }
        }
    }

    pub fn method_call_expr(&mut self, id: NodeId, receiver: NodeId, args: &[NodeId]) -> VReg {
        let Some(callee) = self.cx.prog.modules[self.module.index()].info.callees.get(&id).cloned() else {
            self.cx.error("internal error: method call without a resolved callee");
            return self.unit();
        };
        let mut all = vec![receiver];
        all.extend_from_slice(args);
        let regs = self.operands(&all);
        let first = Some(self.node_ty(receiver));
        self.emit_callee(id, &callee, regs, first)
    }

    /// An operator that resolved to a trait method (generic code or user impls).
    pub fn call_resolved_operator(&mut self, id: NodeId, callee: &Callee, regs: Vec<VReg>) -> VReg {
        let _ = id;
        self.emit_callee(id, callee, regs, None)
    }

    fn call_func(&mut self, func: FuncId, regs: Vec<VReg>, node: NodeId) -> VReg {
        let ret = self.cx.out.funcs[func.index()].ret;
        let dst = self.new_reg(ret);
        let _ = node;
        self.emit(Inst::Call { dst, func, args: regs });
        dst
    }

    pub fn emit_callee(&mut self, node: NodeId, callee: &Callee, regs: Vec<VReg>, first_ty: Option<Ty>) -> VReg {
        match callee {
            Callee::Fn(f, targs) => {
                let targs = self.subst_all(targs);
                let func = self.cx.func_for(FuncKey::Fn(*f, targs));
                self.call_func(func, regs, node)
            }
            Callee::Trait(m, targs) => {
                let targs = self.subst_all(targs);
                match resolve_trait_method(self.cx.prog, *m, &targs) {
                    Some(Resolved::Fn(f, a)) => {
                        let func = self.cx.func_for(FuncKey::Fn(f, a));
                        self.call_func(func, regs, node)
                    }
                    Some(Resolved::BuiltinOp(kind)) => {
                        let name = self.cx.interner.resolve(self.cx.prog.fns[m.index()].name).to_string();
                        self.builtin_trait_op(kind, &name, &targs, regs)
                    }
                    None => {
                        let name = self.cx.interner.resolve(self.cx.prog.fns[m.index()].name).to_string();
                        self.cx.error(format!("internal error: no implementation found for trait method `{name}`"));
                        self.unit()
                    }
                }
            }
            Callee::Builtin(b, _) => self.builtin_call(node, *b, regs, first_ty),
        }
    }

    /// A function item used as a value: a closure object with no captures.
    pub fn callee_as_value(&mut self, c: Callee) -> VReg {
        let func = match &c {
            Callee::Fn(f, targs) => {
                let targs = self.subst_all(targs);
                self.cx.func_for(FuncKey::Fn(*f, targs))
            }
            Callee::Trait(m, targs) => {
                let targs = self.subst_all(targs);
                match resolve_trait_method(self.cx.prog, *m, &targs) {
                    Some(Resolved::Fn(f, a)) => self.cx.func_for(FuncKey::Fn(f, a)),
                    _ => {
                        self.cx.error("unsupported: a built-in operator method used as a function value");
                        return self.unit();
                    }
                }
            }
            Callee::Builtin(..) => {
                self.cx.error("unsupported: a builtin used as a function value");
                return self.unit();
            }
        };
        let dst = self.new_reg(IrTy::Ref);
        self.emit(Inst::MakeClosure { dst, func, captures: vec![] });
        dst
    }

    /// The compiler's own implementation of an operator trait for a primitive type.
    fn builtin_trait_op(&mut self, kind: LangTrait, method: &str, targs: &[Ty], regs: Vec<VReg>) -> VReg {
        let self_ty = targs[0];
        let arith = |op| Some(op);
        let aop = match kind {
            LangTrait::Add => arith(ArithOp::Add),
            LangTrait::Sub => arith(ArithOp::Sub),
            LangTrait::Mul => arith(ArithOp::Mul),
            LangTrait::Div => arith(ArithOp::Div),
            LangTrait::Rem => arith(ArithOp::Rem),
            LangTrait::BitAnd => arith(ArithOp::And),
            LangTrait::BitOr => arith(ArithOp::Or),
            LangTrait::BitXor => arith(ArithOp::Xor),
            LangTrait::Shl => arith(ArithOp::Shl),
            LangTrait::Shr => arith(ArithOp::Shr),
            _ => None,
        };
        if let Some(op) = aop {
            let rt = targs.get(1).copied().unwrap_or(self_ty);
            return self.builtin_arith(op, self_ty, rt, regs[0], regs[1]);
        }
        match kind {
            LangTrait::Neg => {
                let ir = self.cx.ir_ty(self_ty);
                let IrTy::Num(n) = ir else { return self.unit() };
                let dst = self.new_reg(ir);
                self.emit(Inst::Neg { dst, ty: n, src: regs[0] });
                dst
            }
            LangTrait::Not => {
                let ir = self.cx.ir_ty(self_ty);
                let dst = self.new_reg(ir);
                match ir {
                    IrTy::Bool => self.emit(Inst::BoolNot { dst, src: regs[0] }),
                    IrTy::Num(n) => self.emit(Inst::BitNot { dst, ty: n, src: regs[0] }),
                    _ => self.cx.error("internal error: `!` on a non-integer"),
                }
                dst
            }
            LangTrait::Eq | LangTrait::Ord => {
                let op = match method {
                    "eq" => CmpOp::Eq,
                    "ne" => CmpOp::Ne,
                    "lt" => CmpOp::Lt,
                    "le" => CmpOp::Le,
                    "gt" => CmpOp::Gt,
                    _ => CmpOp::Ge,
                };
                self.builtin_cmp(op, self_ty, regs[0], regs[1])
            }
            LangTrait::Display => {
                let desc = self.cx.desc(self_ty);
                let dst = self.new_reg(IrTy::Ref);
                self.emit(Inst::Intrinsic { dst, b: Builtin::ToString, desc: Some(desc), args: regs });
                dst
            }
            _ => {
                self.cx.error("internal error: unsupported built-in operator");
                self.unit()
            }
        }
    }

    /// Emits a builtin call. `first_ty` is the type of the receiver / first argument.
    fn builtin_call(&mut self, node: NodeId, b: Builtin, regs: Vec<VReg>, first_ty: Option<Ty>) -> VReg {
        let mut regs = regs;
        let mut desc = first_ty.map(|t| self.cx.desc(t));

        // `print(x)` of a user type with a `Display` impl prints its `to_string()`.
        if matches!(b, Builtin::Print | Builtin::Println) && !regs.is_empty() {
            if let Some(t) = first_ty {
                if matches!(self.cx.prog.types.kind(t), TyKind::Adt(..)) {
                    if let Some(s) = self.display_string(t, regs[0]) {
                        regs[0] = s;
                        let str_ty = self.cx.prog.types.prim(Prim::Str);
                        desc = Some(self.cx.desc(str_ty));
                    }
                }
            }
        }

        if b.returns_adt() {
            // the descriptor describes the Option / Result the builtin builds
            let ret = self.node_ty(node);
            desc = Some(self.cx.desc(ret));
        }

        let ir = self.ir_ty_of_node(node);
        let dst = self.new_reg(ir);
        self.emit(Inst::Intrinsic { dst, b, desc, args: regs });
        if b == Builtin::Panic {
            self.terminate(Term::Unreachable);
        }
        dst
    }

    /// `to_string()` through a user `Display` impl, if the type has one.
    fn display_string(&mut self, ty: Ty, value: VReg) -> Option<VReg> {
        let trait_id = self.cx.prog.lang.get(LangTrait::Display)?;
        let method = *self.cx.prog.traits[trait_id.index()].methods.first()?;
        match resolve_trait_method(self.cx.prog, method, &[ty])? {
            Resolved::Fn(f, a) => {
                let func = self.cx.func_for(FuncKey::Fn(f, a));
                let dst = self.new_reg(IrTy::Ref);
                self.emit(Inst::Call { dst, func, args: vec![value] });
                Some(dst)
            }
            Resolved::BuiltinOp(_) => None,
        }
    }

    // ---- indexing ---------------------------------------------------------------------------------

    pub fn index_get(&mut self, id: NodeId, base: NodeId, base_reg: VReg, index: NodeId, index_reg: VReg) -> VReg {
        let bt = self.node_ty(base);
        let dst = self.new_reg_for(id);
        match self.cx.prog.types.kind(bt) {
            TyKind::Array(_) => {
                let idx_ty = self.index_num_ty(index);
                self.emit(Inst::ArrayGet { dst, arr: base_reg, idx: index_reg, idx_ty });
            }
            TyKind::Map(..) => self.emit(Inst::MapGet { dst, map: base_reg, key: index_reg }),
            _ => self.cx.error("internal error: indexing a non-collection"),
        }
        dst
    }

    pub fn index_num_ty(&mut self, index: NodeId) -> NumTy {
        match self.ir_ty_of_node(index) {
            IrTy::Num(n) => n,
            _ => NumTy::I64,
        }
    }
}
