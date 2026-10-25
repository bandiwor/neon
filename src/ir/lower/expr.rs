//! Expression lowering.

use super::func::FnLower;
use crate::builtin::Builtin;
use crate::ir::types::*;
use crate::runtime::numeric::{const_int, ArithOp, CmpOp};
use crate::runtime::NumTy;
use crate::sema::{ConstVal, Prim, Res, TyKind};
use crate::syntax::{BinOp, NodeId, NodeKind, UnOp};

impl FnLower<'_, '_> {
    pub fn res(&self, n: NodeId) -> Option<Res> {
        self.cx.prog.modules[self.module.index()].info.res.get(&n).cloned()
    }

    /// Expressions that cannot change any variable (so a live local register
    /// may safely be used as an operand while they are evaluated).
    pub fn is_simple(&self, e: NodeId) -> bool {
        match self.ast.kind(e) {
            NodeKind::IntLit(_)
            | NodeKind::FloatLit(_)
            | NodeKind::StrLit(_)
            | NodeKind::CharLit(_)
            | NodeKind::BoolLit(_)
            | NodeKind::UnitLit
            | NodeKind::Ident(_)
            | NodeKind::Path { .. } => true,
            NodeKind::Field { base, .. } => self.is_simple(*base),
            NodeKind::Unary { operand, .. } => self.is_simple(*operand),
            NodeKind::Cast { expr, .. } => self.is_simple(*expr),
            NodeKind::Binary { lhs, rhs, .. } => self.is_simple(*lhs) && self.is_simple(*rhs),
            NodeKind::Index { base, index } => self.is_simple(*base) && self.is_simple(*index),
            _ => false,
        }
    }

    /// Lowers a list of operands evaluated left to right; a mutable local is
    /// only used in place when nothing later could modify it.
    pub fn operands(&mut self, es: &[NodeId]) -> Vec<VReg> {
        let mut out = Vec::with_capacity(es.len());
        for (i, &e) in es.iter().enumerate() {
            let later_simple = es[i + 1..].iter().all(|&l| self.is_simple(l));
            out.push(if later_simple { self.expr_fast(e) } else { self.expr(e) });
        }
        out
    }

    /// The value of `id`; mutable locals are copied so later assignments
    /// cannot change it.
    pub fn expr(&mut self, id: NodeId) -> VReg {
        self.expr_with(id, false)
    }

    /// Like [`expr`](Self::expr) but may return a live mutable local's own register.
    pub fn expr_fast(&mut self, id: NodeId) -> VReg {
        self.expr_with(id, true)
    }

    fn expr_with(&mut self, id: NodeId, fast: bool) -> VReg {
        let ast = self.ast.clone();
        match *ast.kind(id) {
            NodeKind::IntLit(lit) => {
                let ir = self.ir_ty_of_node(id);
                let v = ast.lits.int(lit).value;
                let bits = match ir {
                    IrTy::Num(n) => const_int(n, v as i128),
                    _ => v,
                };
                self.const_bits(ir, bits)
            }
            NodeKind::FloatLit(lit) => {
                let ir = self.ir_ty_of_node(id);
                let v = ast.lits.float(lit).value;
                let bits = if ir == IrTy::Num(NumTy::F32) { (v as f32).to_bits() as u64 } else { v.to_bits() };
                self.const_bits(ir, bits)
            }
            NodeKind::StrLit(s) => {
                let idx = self.cx.string(ast.lits.str(s));
                let dst = self.new_reg(IrTy::Ref);
                self.emit(Inst::Const { dst, val: Const::Str(idx) });
                dst
            }
            NodeKind::CharLit(c) => self.const_bits(IrTy::Char, c as u64),
            NodeKind::BoolLit(b) => self.const_bool(b),
            NodeKind::UnitLit => self.unit(),
            NodeKind::Ident(_) | NodeKind::Path { .. } => self.value_path(id, fast),
            NodeKind::Unary { op, operand } => self.unary(id, op, operand),
            NodeKind::Binary { op, lhs, rhs } => self.binary(id, op, lhs, rhs),
            NodeKind::Assign { op, target, value } => {
                self.assign(id, op, target, value);
                self.unit()
            }
            NodeKind::Cast { expr, .. } => {
                let from = self.node_ty(expr);
                let to = self.node_ty(id);
                let v = self.expr_fast(expr);
                self.cast(from, to, v)
            }
            NodeKind::Call { callee, args } => self.call_expr(id, callee, ast.list(args)),
            NodeKind::MethodCall { receiver, args, .. } => self.method_call_expr(id, receiver, ast.list(args)),
            NodeKind::Field { base, .. } => {
                let obj = self.expr_fast(base);
                let idx = self.cx.prog.modules[self.module.index()].info.field_indices[&id];
                let dst = self.new_reg_for(id);
                self.emit(Inst::GetField { dst, obj, idx });
                dst
            }
            NodeKind::Index { base, index } => {
                let regs = self.operands(&[base, index]);
                self.index_get(id, base, regs[0], index, regs[1])
            }
            NodeKind::ArrayLit { elems } => {
                let ty = self.node_ty(id);
                let desc = self.cx.desc(ty);
                self.collection_lit(desc, ast.list(elems), Builtin::ArrPush)
            }
            NodeKind::SetLit { elems } => {
                let ty = self.node_ty(id);
                let desc = self.cx.desc(ty);
                self.collection_lit(desc, ast.list(elems), Builtin::SetInsert)
            }
            NodeKind::MapLit { entries } => {
                let ty = self.node_ty(id);
                let desc = self.cx.desc(ty);
                let entries = ast.list(entries);
                let (head, tail) = entries.split_at(entries.len().min(LITERAL_CHUNK));
                let mut pairs = Vec::new();
                for &e in head {
                    let NodeKind::MapEntry { key, value } = *ast.kind(e) else { continue };
                    let regs = self.operands(&[key, value]);
                    pairs.push((regs[0], regs[1]));
                }
                let dst = self.new_reg(IrTy::Ref);
                self.emit(Inst::NewMap { dst, desc, entries: pairs });
                // a long literal is filled in as it is evaluated, so the frame
                // never has to hold every entry at once
                for &e in tail {
                    let NodeKind::MapEntry { key, value } = *ast.kind(e) else { continue };
                    let regs = self.operands(&[key, value]);
                    self.emit(Inst::MapSet { map: dst, key: regs[0], val: regs[1] });
                }
                dst
            }
            NodeKind::StructLit { fields, .. } => {
                let ty = self.node_ty(id);
                let desc = self.cx.desc(ty);
                let n = match self.cx.prog.types.kind(ty) {
                    TyKind::Adt(adt, _) => self.cx.prog.adts[adt.index()].fields().len(),
                    _ => 0,
                };
                let mut slots: Vec<Option<VReg>> = vec![None; n];
                for &f in ast.list(fields) {
                    let NodeKind::FieldInit { value, .. } = *ast.kind(f) else { continue };
                    let idx = self.cx.prog.modules[self.module.index()].info.field_indices[&f] as usize;
                    slots[idx] = Some(self.expr(value));
                }
                let fields: Vec<VReg> =
                    slots.into_iter().map(|s| s.expect("sema checked all fields are present")).collect();
                let dst = self.new_reg(IrTy::Ref);
                self.emit(Inst::NewStruct { dst, desc, fields });
                dst
            }
            NodeKind::Closure { .. } => self.closure(id),
            NodeKind::Block { stmts, tail } => {
                self.push_scope();
                for &s in ast.list(stmts) {
                    self.stmt(s);
                }
                let v = match tail.get() {
                    Some(t) => self.expr(t),
                    None => self.unit(),
                };
                self.pop_scope();
                v
            }
            NodeKind::If { cond, then_branch, else_branch } => self.if_expr(id, cond, then_branch, else_branch.get()),
            NodeKind::While { cond, body } => self.while_loop(cond, body),
            NodeKind::Loop { body } => self.loop_expr(id, body),
            NodeKind::For { pat, iter, body } => self.for_loop(id, pat, iter, body),
            NodeKind::Match { scrutinee, arms } => self.match_expr(id, scrutinee, ast.list(arms)),
            NodeKind::Return { value } => {
                let v = match value.get() {
                    Some(v) => self.expr(v),
                    None => self.unit(),
                };
                if self.f.reg_ty(v) == self.f.ret {
                    self.terminate(Term::Return(v));
                } else {
                    self.terminate(Term::Unreachable);
                }
                self.unit()
            }
            NodeKind::Break { value } => {
                let v = value.get().map(|v| self.expr(v));
                let Some(lp) = self.loops.last() else {
                    self.cx.error("internal error: `break` outside of a loop");
                    return self.unit();
                };
                let (to, depth, result) = (lp.break_to, lp.scope_depth, lp.result);
                if let (Some(v), Some(r)) = (v, result) {
                    if self.f.reg_ty(v) == self.f.reg_ty(r) {
                        self.emit(Inst::Move { dst: r, src: v });
                    }
                }
                self.close_scopes_above(depth);
                self.jump(to);
                self.unit()
            }
            NodeKind::Continue => {
                let Some(lp) = self.loops.last() else {
                    self.cx.error("internal error: `continue` outside of a loop");
                    return self.unit();
                };
                let (to, depth) = (lp.continue_to, lp.scope_depth);
                self.close_scopes_above(depth);
                self.jump(to);
                self.unit()
            }
            _ => {
                self.cx.error("internal error: unexpected node kind in expression position");
                self.unit()
            }
        }
    }

    // ---- paths ------------------------------------------------------------------------------------

    fn value_path(&mut self, id: NodeId, fast: bool) -> VReg {
        match self.res(id) {
            Some(Res::Local(l)) => self.read_local(l, fast),
            Some(Res::Global(g)) => {
                let slot = self.cx.global_slot(g);
                let ty = self.cx.out.globals[slot.index()].ty;
                let dst = self.new_reg(ty);
                self.emit(Inst::GetGlobal { dst, slot });
                dst
            }
            Some(Res::Const(c)) => {
                let ir = self.ir_ty_of_node(id);
                let bits = match (c, ir) {
                    (ConstVal::Int(v), IrTy::Num(n)) => const_int(n, v),
                    (ConstVal::Float(v), IrTy::Num(NumTy::F32)) => (v as f32).to_bits() as u64,
                    (ConstVal::Float(v), _) => v.to_bits(),
                    (ConstVal::Int(v), _) => v as u64,
                };
                self.const_bits(ir, bits)
            }
            Some(Res::Variant { adt, idx, .. }) => {
                let ty = self.node_ty(id);
                let desc = self.cx.desc(ty);
                let _ = adt;
                let dst = self.new_reg(IrTy::Ref);
                self.emit(Inst::NewEnum { dst, desc, tag: idx, fields: vec![] });
                dst
            }
            Some(Res::Callee(c)) => self.callee_as_value(c),
            None => {
                self.cx.error("internal error: unresolved name reached lowering");
                self.unit()
            }
        }
    }

    pub fn read_local(&mut self, l: crate::sema::LocalId, fast: bool) -> VReg {
        match self.slot_of(l) {
            super::func::Slot::Reg(r) => {
                if fast || !self.cx.prog.locals[l.index()].mutable {
                    r
                } else {
                    let dst = self.new_reg(self.f.reg_ty(r));
                    self.emit(Inst::Move { dst, src: r });
                    dst
                }
            }
            super::func::Slot::Up(i) => {
                let dst = self.new_reg(self.f.upvals[i as usize]);
                self.emit(Inst::GetUpval { dst, idx: i });
                dst
            }
        }
    }

    // ---- closures ------------------------------------------------------------------------------------

    fn closure(&mut self, id: NodeId) -> VReg {
        let info = self.cx.prog.modules[self.module.index()].info.closures[&id].clone();
        let func = self.cx.func_for(super::FuncKey::Closure(self.module, id, self.args.clone()));
        let mut captures = Vec::new();
        for l in &info.captures {
            captures.push(match self.slot_of(*l) {
                super::func::Slot::Reg(r) => Capture::Local(r),
                super::func::Slot::Up(i) => Capture::Upvalue(i),
            });
        }
        let dst = self.new_reg(IrTy::Ref);
        self.emit(Inst::MakeClosure { dst, func, captures });
        dst
    }

    // ---- operators ---------------------------------------------------------------------------------------

    fn unary(&mut self, id: NodeId, op: UnOp, operand: NodeId) -> VReg {
        if let Some(callee) = self.cx.prog.modules[self.module.index()].info.callees.get(&id).cloned() {
            let v = self.expr(operand);
            return self.call_resolved_operator(id, &callee, vec![v]);
        }
        let ty = self.node_ty(id);
        let ir = self.cx.ir_ty(ty);
        // `-5`: fold the sign into the literal so `-128i8` is representable
        if op == UnOp::Neg {
            if let (NodeKind::IntLit(lit), IrTy::Num(n)) = (*self.ast.kind(operand), ir) {
                let v = self.ast.lits.int(lit).value as i128;
                return self.const_bits(ir, const_int(n, -v));
            }
        }
        let src = self.expr_fast(operand);
        let dst = self.new_reg(ir);
        match (op, ir) {
            (UnOp::Neg, IrTy::Num(n)) => self.emit(Inst::Neg { dst, ty: n, src }),
            (UnOp::Not, IrTy::Bool) => self.emit(Inst::BoolNot { dst, src }),
            (UnOp::Not | UnOp::BitNot, IrTy::Num(n)) => self.emit(Inst::BitNot { dst, ty: n, src }),
            _ => self.cx.error("internal error: invalid unary operand"),
        }
        dst
    }

    fn binary(&mut self, id: NodeId, op: BinOp, lhs: NodeId, rhs: NodeId) -> VReg {
        if op.is_logical() {
            return self.short_circuit(op, lhs, rhs);
        }
        let callee = self.cx.prog.modules[self.module.index()].info.callees.get(&id).cloned();
        let regs = self.operands(&[lhs, rhs]);
        if let Some(callee) = callee {
            return self.call_resolved_operator(id, &callee, regs);
        }
        let lt = self.node_ty(lhs);
        let rt = self.node_ty(rhs);
        if op.is_comparison() {
            return self.builtin_cmp(cmp_op(op), lt, regs[0], regs[1]);
        }
        self.builtin_arith(arith_op(op), lt, rt, regs[0], regs[1])
    }

    /// `a && b` / `a || b` with short-circuit evaluation.
    fn short_circuit(&mut self, op: BinOp, lhs: NodeId, rhs: NodeId) -> VReg {
        let res = self.new_reg(IrTy::Bool);
        let l = self.expr_fast(lhs);
        let rhs_b = self.new_block();
        let short_b = self.new_block();
        let end = self.new_block();
        if op == BinOp::And {
            self.terminate(Term::Branch { cond: l, then: rhs_b, els: short_b });
        } else {
            self.terminate(Term::Branch { cond: l, then: short_b, els: rhs_b });
        }
        self.switch_to(short_b);
        let c = self.const_bool(op == BinOp::Or);
        self.emit(Inst::Move { dst: res, src: c });
        self.jump(end);
        self.switch_to(rhs_b);
        let r = self.expr(rhs);
        self.emit(Inst::Move { dst: res, src: r });
        self.jump(end);
        self.switch_to(end);
        res
    }

    pub fn builtin_cmp(&mut self, op: CmpOp, ty: crate::sema::Ty, l: VReg, r: VReg) -> VReg {
        let cty = match self.cx.prog.types.kind(ty) {
            TyKind::Prim(Prim::Str) => CmpTy::Str,
            TyKind::Prim(Prim::Bool) => CmpTy::Num(NumTy::U8),
            TyKind::Prim(Prim::Char) => CmpTy::Num(NumTy::U32),
            TyKind::Prim(p) => CmpTy::Num(NumTy::from_prim(*p).expect("numeric")),
            _ => {
                self.cx.error("internal error: comparison of a non-primitive type");
                return self.unit();
            }
        };
        let dst = self.new_reg(IrTy::Bool);
        self.emit(Inst::Cmp { dst, op, ty: cty, lhs: l, rhs: r });
        dst
    }

    /// Arithmetic on primitives. `rt` is the right operand's type (shifts
    /// accept any integer type there).
    pub fn builtin_arith(&mut self, op: ArithOp, lt: crate::sema::Ty, rt: crate::sema::Ty, l: VReg, r: VReg) -> VReg {
        match self.cx.prog.types.kind(lt).clone() {
            TyKind::Prim(Prim::Str) if op == ArithOp::Add => {
                let dst = self.new_reg(IrTy::Ref);
                self.emit(Inst::Intrinsic { dst, b: crate::builtin::Builtin::StrConcat, desc: None, args: vec![l, r] });
                dst
            }
            TyKind::Prim(Prim::Bool) => {
                let dst = self.new_reg(IrTy::Bool);
                self.emit(Inst::Arith { dst, op, ty: NumTy::U8, lhs: l, rhs: r });
                dst
            }
            TyKind::Prim(p) if p.is_numeric() => {
                let n = NumTy::from_prim(p).expect("numeric");
                let mut r = r;
                if op.is_shift() {
                    // the VM takes the shift amount as an i64
                    if let TyKind::Prim(rp) = self.cx.prog.types.kind(rt).clone() {
                        if let Some(rn) = NumTy::from_prim(rp) {
                            if rn != NumTy::I64 {
                                let c = self.new_reg(IrTy::Num(NumTy::I64));
                                self.emit(Inst::Cast { dst: c, from: rn, to: NumTy::I64, src: r });
                                r = c;
                            }
                        }
                    }
                }
                let dst = self.new_reg(IrTy::Num(n));
                self.emit(Inst::Arith { dst, op, ty: n, lhs: l, rhs: r });
                dst
            }
            _ => {
                self.cx.error("internal error: arithmetic on a non-primitive type");
                self.unit()
            }
        }
    }

    pub fn cast(&mut self, from: crate::sema::Ty, to: crate::sema::Ty, v: VReg) -> VReg {
        let num = |s: &Self, t: crate::sema::Ty| match s.cx.prog.types.kind(t) {
            TyKind::Prim(Prim::Bool) => Some(NumTy::U8),
            TyKind::Prim(Prim::Char) => Some(NumTy::U32),
            TyKind::Prim(p) => NumTy::from_prim(*p),
            _ => None,
        };
        let (Some(f), Some(t)) = (num(self, from), num(self, to)) else {
            self.cx.error("internal error: cast between non-numeric types");
            return self.unit();
        };
        let ir = self.cx.ir_ty(to);
        let dst = self.new_reg(ir);
        if f == t && self.f.reg_ty(v) == ir {
            self.emit(Inst::Move { dst, src: v });
        } else {
            self.emit(Inst::Cast { dst, from: f, to: t, src: v });
        }
        dst
    }
}

pub(super) fn arith_op(op: BinOp) -> ArithOp {
    match op {
        BinOp::Add => ArithOp::Add,
        BinOp::Sub => ArithOp::Sub,
        BinOp::Mul => ArithOp::Mul,
        BinOp::Div => ArithOp::Div,
        BinOp::Rem => ArithOp::Rem,
        BinOp::BitAnd => ArithOp::And,
        BinOp::BitOr => ArithOp::Or,
        BinOp::BitXor => ArithOp::Xor,
        BinOp::Shl => ArithOp::Shl,
        BinOp::Shr => ArithOp::Shr,
        other => unreachable!("{other:?} is not an arithmetic operator"),
    }
}

pub(super) fn cmp_op(op: BinOp) -> CmpOp {
    match op {
        BinOp::Eq => CmpOp::Eq,
        BinOp::Ne => CmpOp::Ne,
        BinOp::Lt => CmpOp::Lt,
        BinOp::Le => CmpOp::Le,
        BinOp::Gt => CmpOp::Gt,
        BinOp::Ge => CmpOp::Ge,
        other => unreachable!("{other:?} is not a comparison"),
    }
}

/// Array, set and map literals longer than this are built incrementally.
const LITERAL_CHUNK: usize = 48;

impl FnLower<'_, '_> {
    /// An array or set literal: the first [`LITERAL_CHUNK`] elements go straight
    /// into the allocation, the rest are appended as they are evaluated.
    fn collection_lit(&mut self, desc: DescId, elems: &[NodeId], append: Builtin) -> VReg {
        let (head, tail) = elems.split_at(elems.len().min(LITERAL_CHUNK));
        let regs = self.operands(head);
        let dst = self.new_reg(IrTy::Ref);
        if append == Builtin::ArrPush {
            self.emit(Inst::NewArray { dst, desc, elems: regs });
        } else {
            self.emit(Inst::NewSet { dst, desc, elems: regs });
        }
        for &e in tail {
            let v = self.operands(&[e])[0];
            let res = self.new_reg(if append == Builtin::ArrPush { IrTy::Unit } else { IrTy::Bool });
            self.emit(Inst::Intrinsic { dst: res, b: append, desc: Some(desc), args: vec![dst, v] });
        }
        dst
    }
}
