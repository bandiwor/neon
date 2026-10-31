//! Lazy iterators, fused into loops while lowering.
//!
//! `xs.iter().filter(p).map(f).sum()` is a *pipeline*: a source (an array or a range), steps (`map`,
//! `filter`, `take`, ...) and a last step that makes a value (`sum`, `collect`, `for_each`, a `for` loop).
//! An `Iter<T>` never exists at run time. The pipeline is described while the calls are lowered, and the
//! last step emits **one loop** whose body applies the steps in order with the closures' bodies inlined:
//! no array is built between the steps and no closure object is made.
//!
//! The loops are generated in continuation-passing style: every step receives the element, may skip it
//! (jump to `cont`, the next element of the innermost source loop) or stop the whole pipeline (jump to
//! `done`), and hands what it produced to the rest of the steps and finally to the sink, the code of the
//! last step.

use super::func::{FnLower, LoopCtx};
use crate::builtin::Builtin;
use crate::ir::types::*;
use crate::runtime::value::SOME_TAG;
use crate::runtime::{ArithOp, CmpOp, NumTy};
use crate::sema::{Res, Ty, TyKind};
use crate::syntax::{NodeId, NodeKind};

/// A function argument of a step: a closure literal whose body is inlined, or any other value that is
/// called through the closure machinery.
#[derive(Clone)]
pub(super) enum Fun {
    Inline(NodeId),
    Value(VReg),
}

#[derive(Clone)]
pub(super) enum Origin {
    Array {
        arr: VReg,
        ty: Ty,
        rev: bool,
    },
    Range {
        lo: VReg,
        hi: VReg,
        inclusive: bool,
        num: NumTy,
        rev: bool,
    },
    /// Everything the first pipeline yields, then everything the second one does.
    Chain(Box<Pipeline>, Box<Pipeline>),
}

#[derive(Clone)]
pub(super) enum Stage {
    Map {
        f: Fun,
        out: Ty,
    },
    Filter {
        f: Fun,
    },
    /// The closure returns an `Option` (of type `opt`): `None` skips the element.
    FilterMap {
        f: Fun,
        opt: Ty,
    },
    /// The closure returns an array (of type `arr`) whose elements are passed on one by one.
    FlatMap {
        f: Fun,
        arr: Ty,
    },
    Take {
        n: VReg,
        count: VReg,
    },
    Skip {
        n: VReg,
        count: VReg,
    },
    TakeWhile {
        f: Fun,
    },
    SkipWhile {
        f: Fun,
        skipping: VReg,
    },
    StepBy {
        n: VReg,
        count: VReg,
    },
    Enumerate {
        count: VReg,
        pair: DescId,
    },
    Zip {
        right: Box<Pull>,
        pair: DescId,
    },
    Inspect {
        f: Fun,
    },
}

/// The right-hand side of a `zip`: something that can produce its next element on demand.
#[derive(Clone)]
pub(super) struct Pull {
    origin: Origin,
    maps: Vec<(Fun, Ty)>,
    /// Index of the next element (arrays) or the next value (ranges).
    cursor: VReg,
    /// Set once the last value of an inclusive range was produced.
    finished: VReg,
}

#[derive(Clone)]
pub(super) struct Pipeline {
    origin: Origin,
    stages: Vec<Stage>,
    /// The type of the elements that come out of the last step.
    elem: Ty,
}

/// Where the code of a step can send control.
#[derive(Clone, Copy)]
struct Exits {
    /// The pipeline is over.
    done: BlockId,
    /// Go on with the next element of the innermost loop.
    cont: BlockId,
}

type Sink<'s, 'l, 'a> = &'s mut dyn FnMut(&mut FnLower<'l, 'a>, Exits, VReg);

/// A `return` in an inline closure leaves the closure, not the function.
pub(super) struct InlineRet {
    pub exit: BlockId,
    pub result: Option<VReg>,
}

impl<'l, 'a> FnLower<'l, 'a> {
    // ---- recognizing lazy calls ----------------------------------------------------------------------

    /// If the method call `id` is one of the prelude's lazy iterator methods, its name.
    pub(super) fn lazy_method(&mut self, id: NodeId) -> Option<String> {
        let callee = self.cx.prog.modules[self.module.index()].info.callees.get(&id)?.clone();
        let crate::sema::Callee::Fn(f, _) = callee else { return None };
        self.cx.prog.lazy_owner(self.cx.interner, f)?;
        Some(self.cx.interner.resolve(self.cx.prog.fns[f.index()].name).to_string())
    }

    fn iter_error(&mut self, node: NodeId, what: &str) {
        let span = self.ast.span(node);
        self.cx.error_labeled(
            span,
            "E0270",
            format!(
                "{what}: an iterator is a lazy view that exists only inside one chain of calls; end the chain with \
                 `collect()`, `sum()`, `for_each()`, ... or use it in a `for` loop"
            ),
            "iterator",
        );
    }

    fn lazy_error(&mut self, node: NodeId, message: &str) {
        let span = self.ast.span(node);
        self.cx.error_labeled(span, "E0270", message, "here");
    }

    /// The element type of the `Iter<T>`, `Range<T>` or array type `ty`.
    fn lazy_elem(&mut self, ty: Ty) -> Option<Ty> {
        let la = self.cx.prog.lang_adts;
        match self.cx.prog.types.kind(ty).clone() {
            TyKind::Adt(a, args) if Some(a) == la.iter || Some(a) == la.range || Some(a) == la.range_inclusive => {
                Some(args[0])
            }
            TyKind::Array(e) => Some(e),
            _ => None,
        }
    }

    pub(super) fn is_iter_type(&self, ty: Ty) -> bool {
        matches!(self.cx.prog.types.kind(ty), TyKind::Adt(a, _) if Some(*a) == self.cx.prog.lang_adts.iter)
    }

    // ---- building pipelines ----------------------------------------------------------------------------

    fn fun_arg(&mut self, node: NodeId) -> Fun {
        if self.cx.prog.modules[self.module.index()].info.inline_closures.contains(&node) {
            Fun::Inline(node)
        } else {
            Fun::Value(self.expr(node))
        }
    }

    fn counter(&mut self) -> VReg {
        self.new_reg(IrTy::Num(NumTy::I64))
    }

    fn array_origin(&mut self, node: NodeId) -> Origin {
        let ty = self.node_ty(node);
        let arr = self.expr(node);
        Origin::Array { arr, ty, rev: false }
    }

    fn range_origin(&mut self, node: NodeId) -> Option<Origin> {
        let ty = self.node_ty(node);
        let TyKind::Adt(a, args) = self.cx.prog.types.kind(ty).clone() else { return None };
        let inclusive = Some(a) == self.cx.prog.lang_adts.range_inclusive;
        let num = match self.cx.ir_ty(args[0]) {
            IrTy::Num(n) if n.is_int() => n,
            _ => {
                self.lazy_error(node, "only ranges of integers can be iterated");
                return None;
            }
        };
        let (lo, hi) = match *self.ast.kind(node) {
            // `a..b` written out: the bounds are used directly, the range is never built
            NodeKind::Range { start, end, .. } => {
                let s = self.expr(start.get().expect("sema requires a start"));
                let e = match end.get() {
                    Some(e) => self.expr(e),
                    // `a..` runs up to the largest value (and `inclusive` was forced by sema)
                    None => self.const_bits(IrTy::Num(num), crate::runtime::numeric::const_int(num, num.max_value())),
                };
                (s, e)
            }
            _ => {
                let r = self.expr(node);
                let ir = IrTy::Num(num);
                let (s, e) = (self.new_reg(ir), self.new_reg(ir));
                self.emit(Inst::GetField { dst: s, obj: r, idx: 0 });
                self.emit(Inst::GetField { dst: e, obj: r, idx: 1 });
                (s, e)
            }
        };
        Some(Origin::Range { lo, hi, inclusive, num, rev: false })
    }

    /// The pipeline an `Iter`, range or array expression stands for. Reports an error (and returns
    /// `None`) for iterator expressions that cannot be fused.
    pub(super) fn pipeline(&mut self, node: NodeId) -> Option<Pipeline> {
        let ty = self.node_ty(node);
        let elem = self.lazy_elem(ty)?;
        let la = self.cx.prog.lang_adts;
        let origin = match self.cx.prog.types.kind(ty).clone() {
            TyKind::Array(_) => self.array_origin(node),
            TyKind::Adt(a, _) if Some(a) == la.range || Some(a) == la.range_inclusive => self.range_origin(node)?,
            _ => return self.iter_expr(node),
        };
        Some(Pipeline { origin, stages: Vec::new(), elem })
    }

    /// An expression of type `Iter<T>`: a chain of lazy method calls, or a variable holding one.
    fn iter_expr(&mut self, node: NodeId) -> Option<Pipeline> {
        let ast = self.ast.clone();
        match *ast.kind(node) {
            NodeKind::MethodCall { receiver, args, .. } => {
                let Some(name) = self.lazy_method(node) else {
                    self.iter_error(node, "this call produces an iterator that cannot be fused");
                    return None;
                };
                let args = ast.list(args).to_vec();
                self.adapter(node, &name, receiver, &args)
            }
            NodeKind::Ident(_) | NodeKind::Path { .. } => match self.res(node) {
                Some(Res::Local(l)) => match self.iter_locals.get(&l) {
                    Some(p) => Some(p.clone()),
                    None => {
                        self.iter_error(
                            node,
                            "this iterator variable was not made from a chain of calls in this function",
                        );
                        None
                    }
                },
                _ => {
                    self.iter_error(node, "this iterator cannot be fused");
                    None
                }
            },
            _ => {
                self.iter_error(node, "this iterator expression cannot be fused");
                None
            }
        }
    }

    fn adapter(&mut self, id: NodeId, name: &str, receiver: NodeId, args: &[NodeId]) -> Option<Pipeline> {
        let mut p = self.pipeline(receiver)?;
        let out_ty = self.node_ty(id);
        let out_elem = self.lazy_elem(out_ty).unwrap_or(p.elem);
        match name {
            "iter" => return Some(p),
            "map" => {
                let f = self.fun_arg(args[0]);
                p.stages.push(Stage::Map { f, out: out_elem });
            }
            "filter" => {
                let f = self.fun_arg(args[0]);
                p.stages.push(Stage::Filter { f });
            }
            "filter_map" => {
                let f = self.fun_arg(args[0]);
                let opt = self.cx.prog.lang_adts.option.map(|o| self.cx.prog.types.adt(o, vec![out_elem]));
                let opt = opt?;
                p.stages.push(Stage::FilterMap { f, opt });
            }
            "flat_map" => {
                let f = self.fun_arg(args[0]);
                let arr = self.cx.prog.types.array(out_elem);
                p.stages.push(Stage::FlatMap { f, arr });
            }
            "inspect" => {
                let f = self.fun_arg(args[0]);
                p.stages.push(Stage::Inspect { f });
            }
            "take_while" => {
                let f = self.fun_arg(args[0]);
                p.stages.push(Stage::TakeWhile { f });
            }
            "skip_while" => {
                let f = self.fun_arg(args[0]);
                let skipping = self.new_reg(IrTy::Bool);
                p.stages.push(Stage::SkipWhile { f, skipping });
            }
            "take" | "skip" | "step_by" => {
                let n = self.expr(args[0]);
                let count = self.counter();
                p.stages.push(match name {
                    "take" => Stage::Take { n, count },
                    "skip" => Stage::Skip { n, count },
                    _ => Stage::StepBy { n, count },
                });
            }
            "enumerate" => {
                let count = self.counter();
                let pair = self.cx.desc(out_elem);
                p.stages.push(Stage::Enumerate { count, pair });
            }
            "zip" => {
                let right = self.pipeline(args[0])?;
                let pull = self.pull_of(args[0], right)?;
                let pair = self.cx.desc(out_elem);
                p.stages.push(Stage::Zip { right: Box::new(pull), pair });
            }
            "chain" => {
                let right = self.pipeline(args[0])?;
                p = Pipeline {
                    origin: Origin::Chain(Box::new(p), Box::new(right)),
                    stages: Vec::new(),
                    elem: out_elem,
                };
            }
            "rev" => {
                // only steps that look at one element at a time can be run backwards
                let ok = p.stages.iter().all(|s| {
                    matches!(
                        s,
                        Stage::Map { .. } | Stage::Filter { .. } | Stage::FilterMap { .. } | Stage::Inspect { .. }
                    )
                });
                match &mut p.origin {
                    Origin::Array { rev, .. } | Origin::Range { rev, .. } if ok => *rev = !*rev,
                    _ => {
                        self.iter_error(id, "`rev()` must come right after the source, `map` or `filter`");
                        return None;
                    }
                }
            }
            other => {
                self.cx.error(format!("internal error: unknown lazy method `{other}`"));
                return None;
            }
        }
        p.elem = out_elem;
        Some(p)
    }

    fn pull_of(&mut self, node: NodeId, p: Pipeline) -> Option<Pull> {
        let mut maps = Vec::new();
        for s in &p.stages {
            match s {
                Stage::Map { f, out } => maps.push((f.clone(), *out)),
                _ => {
                    self.iter_error(
                        node,
                        "the right side of `zip` can only be a source with `map`s; `collect()` it first",
                    );
                    return None;
                }
            }
        }
        match &p.origin {
            Origin::Array { rev: false, .. } | Origin::Range { rev: false, .. } => {}
            _ => {
                self.iter_error(
                    node,
                    "the right side of `zip` can only be an array or a range, not reversed or chained",
                );
                return None;
            }
        }
        let cursor = match &p.origin {
            Origin::Range { num, .. } => self.new_reg(IrTy::Num(*num)),
            _ => self.counter(),
        };
        let finished = self.new_reg(IrTy::Bool);
        Some(Pull { origin: p.origin, maps, cursor, finished })
    }

    // ---- calling closures -------------------------------------------------------------------------------

    /// Applies a step's function to `args`; the result is a register of type `ret`.
    fn call_fun(&mut self, f: &Fun, args: &[VReg], ret: Ty) -> VReg {
        match f {
            Fun::Value(callee) => {
                let ir = self.cx.ir_ty(ret);
                let dst = self.new_reg(ir);
                self.emit(Inst::CallClosure { dst, callee: *callee, args: args.to_vec() });
                dst
            }
            Fun::Inline(node) => self.inline_closure(*node, args),
        }
    }

    /// The body of the closure literal `node`, lowered in place with its parameters bound to `args`.
    fn inline_closure(&mut self, node: NodeId, args: &[VReg]) -> VReg {
        let info = self.cx.prog.modules[self.module.index()].info.closures[&node].clone();
        let NodeKind::Closure { body, .. } = *self.ast.kind(node) else { unreachable!("an inline closure") };
        let ret_ty = self.subst(info.ret);
        let ret_ir = self.cx.ir_ty(ret_ty);
        let never = matches!(self.cx.prog.types.kind(ret_ty), TyKind::Never);
        let result = if never || ret_ir == IrTy::Unit { None } else { Some(self.new_reg(ret_ir)) };
        let exit = self.new_block();
        self.push_scope();
        for (l, a) in info.params.iter().zip(args) {
            let r = self.declare_local(*l);
            if self.f.reg_ty(r) == self.f.reg_ty(*a) {
                self.emit(Inst::Move { dst: r, src: *a });
            }
        }
        self.inline_rets.push(InlineRet { exit, result });
        let saved = std::mem::replace(&mut self.ret_ty, ret_ty);
        let v = self.expr(body);
        if !self.sealed {
            if let Some(r) = result {
                if self.f.reg_ty(v) == ret_ir {
                    self.emit(Inst::Move { dst: r, src: v });
                }
            }
            self.jump(exit);
        }
        self.ret_ty = saved;
        self.inline_rets.pop();
        self.switch_to(exit);
        self.pop_scope();
        match result {
            Some(r) => r,
            None => self.unit(),
        }
    }

    /// `return v` inside an inline closure: leave the closure.
    pub(super) fn inline_return(&mut self, v: VReg) -> bool {
        let Some(ir) = self.inline_rets.last() else { return false };
        let (exit, result) = (ir.exit, ir.result);
        if let Some(r) = result {
            if self.f.reg_ty(r) == self.f.reg_ty(v) {
                self.emit(Inst::Move { dst: r, src: v });
            }
        }
        self.jump(exit);
        true
    }

    // ---- driving a pipeline ------------------------------------------------------------------------------

    /// Some value of type `ir`, for registers that are only read on paths where they were set (the
    /// bytecode wants every register written on every path).
    fn placeholder(&mut self, ir: IrTy) -> VReg {
        match ir {
            IrTy::Unit => self.unit(),
            IrTy::Ref => {
                let idx = self.cx.string("");
                let dst = self.new_reg(IrTy::Ref);
                self.emit(Inst::Const { dst, val: Const::Str(idx) });
                dst
            }
            other => self.const_bits(other, 0),
        }
    }

    fn const_i64(&mut self, v: i64) -> VReg {
        self.const_bits(IrTy::Num(NumTy::I64), v as u64)
    }

    fn reset_states(&mut self, p: &Pipeline) {
        match &p.origin {
            Origin::Chain(a, b) => {
                self.reset_states(a);
                self.reset_states(b);
            }
            Origin::Array { .. } | Origin::Range { .. } => {}
        }
        for s in &p.stages {
            match s {
                Stage::Take { count, .. }
                | Stage::Skip { count, .. }
                | Stage::StepBy { count, .. }
                | Stage::Enumerate { count, .. } => {
                    let z = self.const_i64(0);
                    self.emit(Inst::Move { dst: *count, src: z });
                }
                Stage::SkipWhile { skipping, .. } => {
                    let t = self.const_bool(true);
                    self.emit(Inst::Move { dst: *skipping, src: t });
                }
                Stage::Zip { right, .. } => {
                    let f = self.const_bool(false);
                    self.emit(Inst::Move { dst: right.finished, src: f });
                    match &right.origin {
                        Origin::Range { lo, .. } => self.emit(Inst::Move { dst: right.cursor, src: *lo }),
                        _ => {
                            let z = self.const_i64(0);
                            self.emit(Inst::Move { dst: right.cursor, src: z });
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Emits the loop for `p`; `sink` produces the code run for every element that comes out. Control
    /// continues after the loop (in the current block) when the loop is over.
    fn drive(&mut self, p: &Pipeline, sink: Sink<'_, 'l, 'a>) {
        self.reset_states(p);
        let done = self.new_block();
        self.run(p, done, done, sink);
        self.switch_to(done);
    }

    /// `done`: where an early stop goes; `exit`: where the control goes when the sources are used up.
    fn run(&mut self, p: &Pipeline, done: BlockId, exit: BlockId, sink: Sink<'_, 'l, 'a>) {
        let stages = &p.stages;
        match &p.origin {
            Origin::Chain(a, b) => {
                let mid = self.new_block();
                self.run(a, done, mid, &mut |me: &mut Self, cx: Exits, v: VReg| me.apply(stages, cx, v, &mut *sink));
                self.switch_to(mid);
                self.run(b, done, exit, &mut |me: &mut Self, cx: Exits, v: VReg| me.apply(stages, cx, v, &mut *sink));
            }
            origin => {
                self.source_loop(origin, done, exit, &mut |me: &mut Self, cx: Exits, v: VReg| {
                    me.apply(stages, cx, v, &mut *sink)
                });
            }
        }
    }

    /// The loop over an array or a range; `body` is called with the element inside the loop.
    fn source_loop(&mut self, origin: &Origin, done: BlockId, exit: BlockId, body: Sink<'_, 'l, 'a>) {
        match *origin {
            Origin::Array { arr, ty, rev } => {
                let desc = self.cx.desc(ty);
                let i64t = IrTy::Num(NumTy::I64);
                let TyKind::Array(elem) = self.cx.prog.types.kind(ty).clone() else { unreachable!("an array source") };
                let elem_ir = self.cx.ir_ty(elem);
                let idx = self.new_reg(i64t);
                if rev {
                    let len = self.new_reg(i64t);
                    self.emit(Inst::Intrinsic { dst: len, b: Builtin::ArrLen, desc: Some(desc), args: vec![arr] });
                    let one = self.const_i64(1);
                    self.emit(Inst::Arith { dst: idx, op: ArithOp::Sub, ty: NumTy::I64, lhs: len, rhs: one });
                } else {
                    let z = self.const_i64(0);
                    self.emit(Inst::Move { dst: idx, src: z });
                }
                let head = self.new_block();
                let body_b = self.new_block();
                let latch = self.new_block();
                self.jump(head);
                self.switch_to(head);
                let more = self.new_reg(IrTy::Bool);
                if rev {
                    let z = self.const_i64(0);
                    self.emit(Inst::Cmp { dst: more, op: CmpOp::Ge, ty: CmpTy::Num(NumTy::I64), lhs: idx, rhs: z });
                } else {
                    let len = self.new_reg(i64t);
                    self.emit(Inst::Intrinsic { dst: len, b: Builtin::ArrLen, desc: Some(desc), args: vec![arr] });
                    self.emit(Inst::Cmp { dst: more, op: CmpOp::Lt, ty: CmpTy::Num(NumTy::I64), lhs: idx, rhs: len });
                }
                self.terminate(Term::Branch { cond: more, then: body_b, els: exit });
                self.switch_to(body_b);
                let elem_reg = self.new_reg(elem_ir);
                self.emit(Inst::ArrayGet { dst: elem_reg, arr, idx, idx_ty: NumTy::I64 });
                body(self, Exits { done, cont: latch }, elem_reg);
                self.jump(latch);
                self.switch_to(latch);
                let one = self.const_i64(1);
                let op = if rev { ArithOp::Sub } else { ArithOp::Add };
                self.emit(Inst::Arith { dst: idx, op, ty: NumTy::I64, lhs: idx, rhs: one });
                self.jump(head);
            }
            Origin::Range { lo, hi, inclusive, num, rev } => {
                let ir = IrTy::Num(num);
                let cur = self.new_reg(ir);
                let head = self.new_block();
                let body_b = self.new_block();
                let latch = self.new_block();
                let elem_reg = self.new_reg(ir);
                let cmp = |me: &mut Self, op: CmpOp, a: VReg, b: VReg| {
                    let d = me.new_reg(IrTy::Bool);
                    me.emit(Inst::Cmp { dst: d, op, ty: CmpTy::Num(num), lhs: a, rhs: b });
                    d
                };
                match (rev, inclusive) {
                    (false, _) => {
                        self.emit(Inst::Move { dst: cur, src: lo });
                        self.jump(head);
                        self.switch_to(head);
                        let ok = cmp(self, if inclusive { CmpOp::Le } else { CmpOp::Lt }, cur, hi);
                        self.terminate(Term::Branch { cond: ok, then: body_b, els: exit });
                        self.switch_to(body_b);
                        self.emit(Inst::Move { dst: elem_reg, src: cur });
                    }
                    (true, false) => {
                        // count down from `hi`: decrement first, so `hi - 1` never underflows the start
                        self.emit(Inst::Move { dst: cur, src: hi });
                        self.jump(head);
                        self.switch_to(head);
                        let ok = cmp(self, CmpOp::Gt, cur, lo);
                        self.terminate(Term::Branch { cond: ok, then: body_b, els: exit });
                        self.switch_to(body_b);
                        let one = self.const_bits(ir, 1);
                        self.emit(Inst::Arith { dst: cur, op: ArithOp::Sub, ty: num, lhs: cur, rhs: one });
                        self.emit(Inst::Move { dst: elem_reg, src: cur });
                    }
                    (true, true) => {
                        self.emit(Inst::Move { dst: cur, src: hi });
                        self.jump(head);
                        self.switch_to(head);
                        let ok = cmp(self, CmpOp::Ge, cur, lo);
                        self.terminate(Term::Branch { cond: ok, then: body_b, els: exit });
                        self.switch_to(body_b);
                        self.emit(Inst::Move { dst: elem_reg, src: cur });
                    }
                }
                body(self, Exits { done, cont: latch }, elem_reg);
                self.jump(latch);
                self.switch_to(latch);
                match (rev, inclusive) {
                    (false, false) => {
                        let one = self.const_bits(ir, 1);
                        self.emit(Inst::Arith { dst: cur, op: ArithOp::Add, ty: num, lhs: cur, rhs: one });
                        self.jump(head);
                    }
                    (false, true) => {
                        // stop at `hi` before incrementing past it (which could overflow)
                        let last = cmp(self, CmpOp::Eq, cur, hi);
                        let inc = self.new_block();
                        self.terminate(Term::Branch { cond: last, then: exit, els: inc });
                        self.switch_to(inc);
                        let one = self.const_bits(ir, 1);
                        self.emit(Inst::Arith { dst: cur, op: ArithOp::Add, ty: num, lhs: cur, rhs: one });
                        self.jump(head);
                    }
                    (true, false) => self.jump(head),
                    (true, true) => {
                        let last = cmp(self, CmpOp::Eq, cur, lo);
                        let dec = self.new_block();
                        self.terminate(Term::Branch { cond: last, then: exit, els: dec });
                        self.switch_to(dec);
                        let one = self.const_bits(ir, 1);
                        self.emit(Inst::Arith { dst: cur, op: ArithOp::Sub, ty: num, lhs: cur, rhs: one });
                        self.jump(head);
                    }
                }
            }
            Origin::Chain(..) => unreachable!("chains are handled by `run`"),
        }
    }

    /// Applies the steps to one element and passes the result on to `sink`.
    fn apply(&mut self, stages: &[Stage], cx: Exits, v: VReg, sink: Sink<'_, 'l, 'a>) {
        let Some((stage, rest)) = stages.split_first() else {
            sink(self, cx, v);
            return;
        };
        match stage {
            Stage::Map { f, out } => {
                let w = self.call_fun(f, &[v], *out);
                self.apply(rest, cx, w, sink);
            }
            Stage::Inspect { f } => {
                let unit = self.cx.prog.types.unit();
                self.call_fun(f, &[v], unit);
                self.apply(rest, cx, v, sink);
            }
            Stage::Filter { f } => {
                let bool_ty = self.cx.prog.types.bool();
                let keep = self.call_fun(f, &[v], bool_ty);
                let ok = self.new_block();
                self.terminate(Term::Branch { cond: keep, then: ok, els: cx.cont });
                self.switch_to(ok);
                self.apply(rest, cx, v, sink);
            }
            Stage::TakeWhile { f } => {
                let bool_ty = self.cx.prog.types.bool();
                let keep = self.call_fun(f, &[v], bool_ty);
                let ok = self.new_block();
                self.terminate(Term::Branch { cond: keep, then: ok, els: cx.done });
                self.switch_to(ok);
                self.apply(rest, cx, v, sink);
            }
            Stage::SkipWhile { f, skipping } => {
                let bool_ty = self.cx.prog.types.bool();
                let check = self.new_block();
                let pass = self.new_block();
                self.terminate(Term::Branch { cond: *skipping, then: check, els: pass });
                self.switch_to(check);
                let drop = self.call_fun(f, &[v], bool_ty);
                let stop = self.new_block();
                self.terminate(Term::Branch { cond: drop, then: cx.cont, els: stop });
                self.switch_to(stop);
                let f_ = self.const_bool(false);
                self.emit(Inst::Move { dst: *skipping, src: f_ });
                self.jump(pass);
                self.switch_to(pass);
                self.apply(rest, cx, v, sink);
            }
            Stage::FilterMap { f, opt: opt_ty } => {
                let opt = self.call_fun(f, &[v], *opt_ty);
                let some = self.new_block();
                self.terminate(Term::BranchTag { obj: opt, tag: SOME_TAG, then: some, els: cx.cont });
                self.switch_to(some);
                let payload_ty = match self.cx.prog.types.kind(*opt_ty).clone() {
                    TyKind::Adt(_, args) if !args.is_empty() => self.cx.ir_ty(args[0]),
                    _ => IrTy::Unit,
                };
                let w = self.new_reg(payload_ty);
                self.emit(Inst::EnumField { dst: w, obj: opt, idx: 0 });
                self.apply(rest, cx, w, sink);
            }
            Stage::FlatMap { f, arr: arr_ty } => {
                let arr_ty = *arr_ty;
                let arr = self.call_fun(f, &[v], arr_ty);
                let desc = self.cx.desc(arr_ty);
                let TyKind::Array(elem) = self.cx.prog.types.kind(arr_ty).clone() else {
                    self.cx.error("internal error: flat_map without an array");
                    return;
                };
                let elem_ir = self.cx.ir_ty(elem);
                let i64t = IrTy::Num(NumTy::I64);
                let idx = self.new_reg(i64t);
                let z = self.const_i64(0);
                self.emit(Inst::Move { dst: idx, src: z });
                let head = self.new_block();
                let body_b = self.new_block();
                let latch = self.new_block();
                self.jump(head);
                self.switch_to(head);
                let len = self.new_reg(i64t);
                self.emit(Inst::Intrinsic { dst: len, b: Builtin::ArrLen, desc: Some(desc), args: vec![arr] });
                let more = self.new_reg(IrTy::Bool);
                self.emit(Inst::Cmp { dst: more, op: CmpOp::Lt, ty: CmpTy::Num(NumTy::I64), lhs: idx, rhs: len });
                // when the inner array is used up, go on with the next outer element
                self.terminate(Term::Branch { cond: more, then: body_b, els: cx.cont });
                self.switch_to(body_b);
                let w = self.new_reg(elem_ir);
                self.emit(Inst::ArrayGet { dst: w, arr, idx, idx_ty: NumTy::I64 });
                self.apply(rest, Exits { done: cx.done, cont: latch }, w, sink);
                self.jump(latch);
                self.switch_to(latch);
                let one = self.const_i64(1);
                self.emit(Inst::Arith { dst: idx, op: ArithOp::Add, ty: NumTy::I64, lhs: idx, rhs: one });
                self.jump(head);
                // the code after this step is unreachable straight-line code
                let dead = self.new_block();
                self.switch_to(dead);
            }
            Stage::Take { n, count } => {
                let ge = self.new_reg(IrTy::Bool);
                self.emit(Inst::Cmp { dst: ge, op: CmpOp::Ge, ty: CmpTy::Num(NumTy::I64), lhs: *count, rhs: *n });
                let ok = self.new_block();
                self.terminate(Term::Branch { cond: ge, then: cx.done, els: ok });
                self.switch_to(ok);
                let one = self.const_i64(1);
                self.emit(Inst::Arith { dst: *count, op: ArithOp::Add, ty: NumTy::I64, lhs: *count, rhs: one });
                // after this element's turn, stop at once when the quota is full (without pulling another)
                let after = self.new_block();
                self.apply(rest, Exits { done: cx.done, cont: after }, v, sink);
                self.jump(after);
                self.switch_to(after);
                let full = self.new_reg(IrTy::Bool);
                self.emit(Inst::Cmp { dst: full, op: CmpOp::Ge, ty: CmpTy::Num(NumTy::I64), lhs: *count, rhs: *n });
                self.terminate(Term::Branch { cond: full, then: cx.done, els: cx.cont });
                let dead = self.new_block();
                self.switch_to(dead);
            }
            Stage::Skip { n, count } => {
                let lt = self.new_reg(IrTy::Bool);
                self.emit(Inst::Cmp { dst: lt, op: CmpOp::Lt, ty: CmpTy::Num(NumTy::I64), lhs: *count, rhs: *n });
                let skip = self.new_block();
                let pass = self.new_block();
                self.terminate(Term::Branch { cond: lt, then: skip, els: pass });
                self.switch_to(skip);
                let one = self.const_i64(1);
                self.emit(Inst::Arith { dst: *count, op: ArithOp::Add, ty: NumTy::I64, lhs: *count, rhs: one });
                self.jump(cx.cont);
                self.switch_to(pass);
                self.apply(rest, cx, v, sink);
            }
            Stage::StepBy { n, count } => {
                let r = self.new_reg(IrTy::Num(NumTy::I64));
                self.emit(Inst::Arith { dst: r, op: ArithOp::Rem, ty: NumTy::I64, lhs: *count, rhs: *n });
                let zero = self.const_i64(0);
                let hit = self.new_reg(IrTy::Bool);
                self.emit(Inst::Cmp { dst: hit, op: CmpOp::Eq, ty: CmpTy::Num(NumTy::I64), lhs: r, rhs: zero });
                let one = self.const_i64(1);
                self.emit(Inst::Arith { dst: *count, op: ArithOp::Add, ty: NumTy::I64, lhs: *count, rhs: one });
                let ok = self.new_block();
                self.terminate(Term::Branch { cond: hit, then: ok, els: cx.cont });
                self.switch_to(ok);
                self.apply(rest, cx, v, sink);
            }
            Stage::Enumerate { count, pair } => {
                let i = self.new_reg(IrTy::Num(NumTy::I64));
                self.emit(Inst::Move { dst: i, src: *count });
                let one = self.const_i64(1);
                self.emit(Inst::Arith { dst: *count, op: ArithOp::Add, ty: NumTy::I64, lhs: *count, rhs: one });
                let t = self.new_reg(IrTy::Ref);
                self.emit(Inst::NewEnum { dst: t, desc: *pair, tag: 0, fields: vec![i, v] });
                self.apply(rest, cx, t, sink);
            }
            Stage::Zip { right, pair } => {
                let Some(w) = self.pull(right, cx.done) else { return };
                let t = self.new_reg(IrTy::Ref);
                self.emit(Inst::NewEnum { dst: t, desc: *pair, tag: 0, fields: vec![v, w] });
                self.apply(rest, cx, t, sink);
            }
        }
    }

    /// Takes the next element out of the right side of a `zip`; jumps to `done` when it is used up.
    fn pull(&mut self, p: &Pull, done: BlockId) -> Option<VReg> {
        let mut v = match p.origin {
            Origin::Array { arr, ty, .. } => {
                let desc = self.cx.desc(ty);
                let TyKind::Array(elem) = self.cx.prog.types.kind(ty).clone() else { unreachable!("an array source") };
                let elem_ir = self.cx.ir_ty(elem);
                let len = self.new_reg(IrTy::Num(NumTy::I64));
                self.emit(Inst::Intrinsic { dst: len, b: Builtin::ArrLen, desc: Some(desc), args: vec![arr] });
                let ge = self.new_reg(IrTy::Bool);
                self.emit(Inst::Cmp { dst: ge, op: CmpOp::Ge, ty: CmpTy::Num(NumTy::I64), lhs: p.cursor, rhs: len });
                let ok = self.new_block();
                self.terminate(Term::Branch { cond: ge, then: done, els: ok });
                self.switch_to(ok);
                let x = self.new_reg(elem_ir);
                self.emit(Inst::ArrayGet { dst: x, arr, idx: p.cursor, idx_ty: NumTy::I64 });
                let one = self.const_i64(1);
                self.emit(Inst::Arith { dst: p.cursor, op: ArithOp::Add, ty: NumTy::I64, lhs: p.cursor, rhs: one });
                x
            }
            Origin::Range { hi, inclusive, num, .. } => {
                let ir = IrTy::Num(num);
                let ok = self.new_block();
                let cont = self.new_block();
                if inclusive {
                    self.terminate(Term::Branch { cond: p.finished, then: done, els: cont });
                    self.switch_to(cont);
                    let le = self.new_reg(IrTy::Bool);
                    self.emit(Inst::Cmp { dst: le, op: CmpOp::Le, ty: CmpTy::Num(num), lhs: p.cursor, rhs: hi });
                    self.terminate(Term::Branch { cond: le, then: ok, els: done });
                } else {
                    let lt = self.new_reg(IrTy::Bool);
                    self.emit(Inst::Cmp { dst: lt, op: CmpOp::Lt, ty: CmpTy::Num(num), lhs: p.cursor, rhs: hi });
                    self.terminate(Term::Branch { cond: lt, then: ok, els: done });
                    self.switch_to(cont); // unused
                }
                self.switch_to(ok);
                let x = self.new_reg(ir);
                self.emit(Inst::Move { dst: x, src: p.cursor });
                if inclusive {
                    let last = self.new_reg(IrTy::Bool);
                    self.emit(Inst::Cmp { dst: last, op: CmpOp::Eq, ty: CmpTy::Num(num), lhs: p.cursor, rhs: hi });
                    self.emit(Inst::Move { dst: p.finished, src: last });
                    let keep = self.new_block();
                    let adv = self.new_block();
                    self.terminate(Term::Branch { cond: last, then: keep, els: adv });
                    self.switch_to(adv);
                    let one = self.const_bits(ir, 1);
                    self.emit(Inst::Arith { dst: p.cursor, op: ArithOp::Add, ty: num, lhs: p.cursor, rhs: one });
                    self.jump(keep);
                    self.switch_to(keep);
                } else {
                    let one = self.const_bits(ir, 1);
                    self.emit(Inst::Arith { dst: p.cursor, op: ArithOp::Add, ty: num, lhs: p.cursor, rhs: one });
                }
                x
            }
            Origin::Chain(..) => return None,
        };
        for (f, out) in &p.maps {
            v = self.call_fun(f, &[v], *out);
        }
        Some(v)
    }

    // ---- the last step ------------------------------------------------------------------------------------

    /// A method call that ends a chain: emits the loop and returns the value.
    pub(super) fn lazy_terminal(&mut self, id: NodeId, name: &str, receiver: NodeId, args: &[NodeId]) -> VReg {
        let out_ty = self.node_ty(id);
        if self.is_iter_type(out_ty) {
            self.iter_error(id, "an iterator cannot be used as a value here");
            return self.unit();
        }
        let Some(p) = self.pipeline(receiver) else { return self.unit() };
        let elem = p.elem;
        let elem_ir = self.cx.ir_ty(elem);
        match name {
            "for_each" => {
                let f = self.fun_arg(args[0]);
                let unit = self.cx.prog.types.unit();
                self.drive(&p, &mut |me, _cx, v| {
                    me.call_fun(&f, &[v], unit);
                });
                self.unit()
            }
            "fold" => {
                let init = self.expr(args[0]);
                let f = self.fun_arg(args[1]);
                let acc = self.new_reg(self.f.reg_ty(init));
                self.emit(Inst::Move { dst: acc, src: init });
                self.drive(&p, &mut |me, _cx, v| {
                    let r = me.call_fun(&f, &[acc, v], out_ty);
                    me.emit(Inst::Move { dst: acc, src: r });
                });
                acc
            }
            "sum" | "product" => {
                let IrTy::Num(num) = elem_ir else {
                    self.lazy_error(id, "sum and product need numbers");
                    return self.unit();
                };
                let one = name == "product";
                let bits = match (num, one) {
                    (NumTy::F64, false) => 0.0f64.to_bits(),
                    (NumTy::F64, true) => 1.0f64.to_bits(),
                    (NumTy::F32, false) => 0.0f32.to_bits() as u64,
                    (NumTy::F32, true) => 1.0f32.to_bits() as u64,
                    (_, o) => o as u64,
                };
                let acc = self.new_reg(elem_ir);
                let init = self.const_bits(elem_ir, bits);
                self.emit(Inst::Move { dst: acc, src: init });
                let op = if one { ArithOp::Mul } else { ArithOp::Add };
                self.drive(&p, &mut |me, _cx, v| {
                    me.emit(Inst::Arith { dst: acc, op, ty: num, lhs: acc, rhs: v });
                });
                acc
            }
            "count" => {
                let n = self.counter();
                let z = self.const_i64(0);
                self.emit(Inst::Move { dst: n, src: z });
                self.drive(&p, &mut |me, _cx, _v| {
                    let one = me.const_i64(1);
                    me.emit(Inst::Arith { dst: n, op: ArithOp::Add, ty: NumTy::I64, lhs: n, rhs: one });
                });
                n
            }
            "collect" => {
                let desc = self.cx.desc(out_ty);
                let arr = self.new_reg(IrTy::Ref);
                self.emit(Inst::NewArray { dst: arr, desc, elems: vec![] });
                self.drive(&p, &mut |me, _cx, v| {
                    let d = me.new_reg(IrTy::Unit);
                    me.emit(Inst::Intrinsic { dst: d, b: Builtin::ArrPush, desc: Some(desc), args: vec![arr, v] });
                });
                arr
            }
            "join" => {
                let sep = self.expr(args[0]);
                let arr_ty = self.cx.prog.types.array(elem);
                let desc = self.cx.desc(arr_ty);
                let arr = self.new_reg(IrTy::Ref);
                self.emit(Inst::NewArray { dst: arr, desc, elems: vec![] });
                self.drive(&p, &mut |me, _cx, v| {
                    let d = me.new_reg(IrTy::Unit);
                    me.emit(Inst::Intrinsic { dst: d, b: Builtin::ArrPush, desc: Some(desc), args: vec![arr, v] });
                });
                let out = self.new_reg(IrTy::Ref);
                self.emit(Inst::Intrinsic { dst: out, b: Builtin::ArrJoin, desc: Some(desc), args: vec![arr, sep] });
                out
            }
            "any" | "all" => {
                let f = self.fun_arg(args[0]);
                let is_any = name == "any";
                let res = self.new_reg(IrTy::Bool);
                let init = self.const_bool(!is_any);
                self.emit(Inst::Move { dst: res, src: init });
                let bool_ty = self.cx.prog.types.bool();
                self.drive(&p, &mut |me, cx, v| {
                    let t = me.call_fun(&f, &[v], bool_ty);
                    let hit = me.new_block();
                    let (then, els) = if is_any { (hit, cx.cont) } else { (cx.cont, hit) };
                    me.terminate(Term::Branch { cond: t, then, els });
                    me.switch_to(hit);
                    let r = me.const_bool(is_any);
                    me.emit(Inst::Move { dst: res, src: r });
                    me.jump(cx.done);
                    let dead = me.new_block();
                    me.switch_to(dead);
                });
                res
            }
            "find" | "position" | "first" | "nth" | "last" => self.lazy_search(id, name, &p, args, out_ty),
            "reduce" => {
                let f = self.fun_arg(args[0]);
                let has = self.new_reg(IrTy::Bool);
                let f0 = self.const_bool(false);
                self.emit(Inst::Move { dst: has, src: f0 });
                let acc = self.new_reg(elem_ir);
                let dummy = self.placeholder(elem_ir);
                self.emit(Inst::Move { dst: acc, src: dummy });
                self.drive(&p, &mut |me, _cx, v| {
                    let first = me.new_block();
                    let more = me.new_block();
                    let end = me.new_block();
                    me.terminate(Term::Branch { cond: has, then: more, els: first });
                    me.switch_to(first);
                    me.emit(Inst::Move { dst: acc, src: v });
                    let t = me.const_bool(true);
                    me.emit(Inst::Move { dst: has, src: t });
                    me.jump(end);
                    me.switch_to(more);
                    let r = me.call_fun(&f, &[acc, v], elem);
                    me.emit(Inst::Move { dst: acc, src: r });
                    me.jump(end);
                    me.switch_to(end);
                });
                self.option_from(out_ty, has, acc)
            }
            "max" | "min" | "max_by_key" | "min_by_key" => {
                let by_key = name.ends_with("_key");
                let want_max = name.starts_with("max");
                let key_fn = if by_key { Some(self.fun_arg(args[0])) } else { None };
                let key_ty = match &key_fn {
                    Some(Fun::Inline(node)) => {
                        let r = self.cx.prog.modules[self.module.index()].info.closures[node].ret;
                        self.subst(r)
                    }
                    Some(Fun::Value(_)) => {
                        self.lazy_error(id, "max_by_key and min_by_key need a closure written out as the argument");
                        return self.unit();
                    }
                    None => elem,
                };
                if !matches!(self.cx.prog.types.kind(key_ty), TyKind::Prim(_)) {
                    self.lazy_error(
                        id,
                        "max, min and the by_key forms compare numbers, characters, booleans or strings",
                    );
                    return self.unit();
                }
                let has = self.new_reg(IrTy::Bool);
                let f0 = self.const_bool(false);
                self.emit(Inst::Move { dst: has, src: f0 });
                let best = self.new_reg(elem_ir);
                let key_ir = self.cx.ir_ty(key_ty);
                let best_key = self.new_reg(key_ir);
                let (d1, d2) = (self.placeholder(elem_ir), self.placeholder(key_ir));
                self.emit(Inst::Move { dst: best, src: d1 });
                self.emit(Inst::Move { dst: best_key, src: d2 });
                self.drive(&p, &mut |me, _cx, v| {
                    let k = match &key_fn {
                        Some(f) => me.call_fun(f, &[v], key_ty),
                        None => v,
                    };
                    let first = me.new_block();
                    let compare = me.new_block();
                    let take = me.new_block();
                    let end = me.new_block();
                    me.terminate(Term::Branch { cond: has, then: compare, els: first });
                    me.switch_to(first);
                    let t = me.const_bool(true);
                    me.emit(Inst::Move { dst: has, src: t });
                    me.jump(take);
                    me.switch_to(compare);
                    // ties keep the first maximum / minimum... as Rust's `min`, and the last maximum
                    let better = if want_max {
                        me.builtin_cmp(CmpOp::Ge, key_ty, k, best_key)
                    } else {
                        me.builtin_cmp(CmpOp::Lt, key_ty, k, best_key)
                    };
                    me.terminate(Term::Branch { cond: better, then: take, els: end });
                    me.switch_to(take);
                    me.emit(Inst::Move { dst: best, src: v });
                    me.emit(Inst::Move { dst: best_key, src: k });
                    me.jump(end);
                    me.switch_to(end);
                });
                self.option_from(out_ty, has, best)
            }
            other => {
                self.cx.error(format!("internal error: unknown lazy method `{other}`"));
                self.unit()
            }
        }
    }

    /// `Some(value)` if `has`, else `None`, of the `Option` type `opt_ty`.
    fn option_from(&mut self, opt_ty: Ty, has: VReg, value: VReg) -> VReg {
        let desc = self.cx.desc(opt_ty);
        let res = self.new_reg(IrTy::Ref);
        let some = self.new_block();
        let none = self.new_block();
        let end = self.new_block();
        self.terminate(Term::Branch { cond: has, then: some, els: none });
        self.switch_to(some);
        self.emit(Inst::NewEnum { dst: res, desc, tag: SOME_TAG, fields: vec![value] });
        self.jump(end);
        self.switch_to(none);
        self.emit(Inst::NewEnum { dst: res, desc, tag: crate::runtime::value::NONE_TAG, fields: vec![] });
        self.jump(end);
        self.switch_to(end);
        res
    }

    /// `find`, `position`, `first`, `nth`, `last`: ends the loop early when the answer is known.
    fn lazy_search(&mut self, _id: NodeId, name: &str, p: &Pipeline, args: &[NodeId], out_ty: Ty) -> VReg {
        let desc = self.cx.desc(out_ty);
        let res = self.new_reg(IrTy::Ref);
        self.emit(Inst::NewEnum { dst: res, desc, tag: crate::runtime::value::NONE_TAG, fields: vec![] });
        let f = if name == "find" || name == "position" { Some(self.fun_arg(args[0])) } else { None };
        let n = if name == "nth" { Some(self.expr(args[0])) } else { None };
        let idx = self.counter();
        let z = self.const_i64(0);
        self.emit(Inst::Move { dst: idx, src: z });
        let bool_ty = self.cx.prog.types.bool();
        let by_position = name == "position";
        let last = name == "last";
        self.drive(p, &mut |me, cx, v| {
            // does this element answer the question?
            let hit = me.new_block();
            let miss = me.new_block();
            match (&f, n) {
                (Some(f), _) => {
                    let t = me.call_fun(f, &[v], bool_ty);
                    me.terminate(Term::Branch { cond: t, then: hit, els: miss });
                }
                (None, Some(n)) => {
                    let eq = me.new_reg(IrTy::Bool);
                    me.emit(Inst::Cmp { dst: eq, op: CmpOp::Eq, ty: CmpTy::Num(NumTy::I64), lhs: idx, rhs: n });
                    me.terminate(Term::Branch { cond: eq, then: hit, els: miss });
                }
                (None, None) => me.jump(hit),
            }
            me.switch_to(hit);
            let payload = if by_position { idx } else { v };
            me.emit(Inst::NewEnum { dst: res, desc, tag: SOME_TAG, fields: vec![payload] });
            if last {
                me.jump(miss);
            } else {
                me.jump(cx.done);
            }
            me.switch_to(miss);
            let one = me.const_i64(1);
            me.emit(Inst::Arith { dst: idx, op: ArithOp::Add, ty: NumTy::I64, lhs: idx, rhs: one });
        });
        res
    }

    // ---- `for` over a lazy value ---------------------------------------------------------------------------

    pub(super) fn for_lazy(&mut self, pat: NodeId, iter: NodeId, body: NodeId) -> VReg {
        let Some(p) = self.pipeline(iter) else { return self.unit() };
        self.drive(&p, &mut |me, cx, v| {
            me.push_scope();
            me.bind_irrefutable(pat, v);
            me.loops.push(LoopCtx {
                break_to: cx.done,
                continue_to: cx.cont,
                scope_depth: me.scopes.len() - 1,
                result: None,
            });
            me.expr(body);
            me.loops.pop();
            me.pop_scope();
        });
        self.unit()
    }

    /// `let it = <iterator chain>;`: remember the chain; it is expanded where `it` is consumed.
    pub(super) fn bind_iterator(&mut self, pat: NodeId, init: NodeId) -> bool {
        let ty = self.node_ty(init);
        if !self.is_iter_type(ty) {
            return false;
        }
        let Some(p) = self.pipeline(init) else { return true };
        if let Some(&l) = self.cx.prog.modules[self.module.index()].info.binding_locals.get(&pat) {
            self.iter_locals.insert(l, p);
        }
        true
    }
}
