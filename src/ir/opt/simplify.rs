//! Value forwarding and local simplifications.
//!
//! * **Value forwarding / CSE** over extended basic blocks (a block with a single predecessor
//!   continues the predecessor's knowledge): a pure expression computed twice is computed once;
//!   a field read of a struct or enum built in the same region is the field's register; a
//!   `BranchTag` on a freshly built enum becomes a jump. After inlining, constructors and
//!   accessors vanish this way and DCE then drops the unused allocation.
//! * **Algebraic identities** on integers (`x + 0`, `x * 1`, `x * 0`, ...). Floats are untouched
//!   because of `-0.0` and NaN.
//! * **`print(a + b)`** becomes `print(a); print(b)`: no temporary string.
//! * **Dead element loads**: `for x in xs` loads `xs[i]` right after proving `i < len(xs)`; when the
//!   element is unused, the load (and its bounds check) is removed.
//!
//! Every rewrite keeps run-time traps: a trapping operation is only replaced by an earlier
//! identical one (which traps first), never dropped.

use crate::fx::FxHashMap as HashMap;

use crate::builtin::Builtin;
use crate::ir::types::*;
use crate::runtime::numeric::{decode_int, ArithOp, CmpOp};
use crate::runtime::NumTy;

/// Runs all simplifications on `f`; returns how many rewrites happened.
pub fn simplify_func(f: &mut IrFunc) -> usize {
    forward_values(f) + algebra(f) + split_print_concat(f) + drop_inrange_loads(f)
}

// ---- helpers ------------------------------------------------------------------------------------

pub(super) fn def_counts(f: &IrFunc) -> Vec<u32> {
    let mut defs = vec![0u32; f.regs.len()];
    for c in defs.iter_mut().take(f.params as usize) {
        *c += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            if let Some(d) = i.def() {
                defs[d.index()] += 1;
            }
        }
    }
    defs
}

fn use_counts(f: &IrFunc) -> Vec<u32> {
    let mut uses = vec![0u32; f.regs.len()];
    for b in &f.blocks {
        for i in &b.insts {
            i.for_each_use(|r| uses[r.index()] += 1);
        }
        b.term.for_each_use(|r| uses[r.index()] += 1);
    }
    uses
}

/// Predecessor lists (an edge counts once per terminator slot).
pub(super) fn predecessors(f: &IrFunc) -> Vec<Vec<usize>> {
    let mut preds = vec![Vec::new(); f.blocks.len()];
    for (i, b) in f.blocks.iter().enumerate() {
        for s in b.term.successors() {
            preds[s.index()].push(i);
        }
    }
    preds
}

/// Whether executing `i` may write heap memory (or run arbitrary code that does).
pub(super) fn clobbers_memory(i: &Inst) -> bool {
    match i {
        Inst::SetField { .. }
        | Inst::ArraySet { .. }
        | Inst::MapSet { .. }
        | Inst::Call { .. }
        | Inst::CallClosure { .. } => true,
        Inst::Intrinsic { b, .. } => b.mutates_heap(),
        _ => false,
    }
}

// ---- value forwarding and CSE ---------------------------------------------------------------------

#[derive(Clone, PartialEq, Eq, Hash)]
enum Key {
    Arith(ArithOp, NumTy, VReg, VReg),
    Cmp(CmpOp, CmpTy, VReg, VReg),
    Neg(NumTy, VReg),
    BitNot(NumTy, VReg),
    BoolNot(VReg),
    Cast(NumTy, NumTy, VReg),
    GetField(VReg, u32),
    EnumTag(VReg),
    EnumField(VReg, u32),
    ArrayGet(VReg, VReg, NumTy),
    Intrinsic(Builtin, Option<DescId>, Vec<VReg>),
}

impl Key {
    fn operands(&self) -> Vec<VReg> {
        match self {
            Key::Arith(_, _, a, b) | Key::Cmp(_, _, a, b) | Key::ArrayGet(a, b, _) => vec![*a, *b],
            Key::Neg(_, a)
            | Key::BitNot(_, a)
            | Key::BoolNot(a)
            | Key::Cast(_, _, a)
            | Key::GetField(a, _)
            | Key::EnumTag(a)
            | Key::EnumField(a, _) => vec![*a],
            Key::Intrinsic(_, _, args) => args.clone(),
        }
    }

    /// Whether the value can change when memory is written.
    fn reads_mutable_memory(&self) -> bool {
        match self {
            Key::GetField(..) | Key::ArrayGet(..) => true,
            Key::Intrinsic(b, ..) => b.reads_memory_only(),
            _ => false,
        }
    }
}

#[derive(Clone)]
enum Agg {
    Struct(Vec<VReg>),
    Enum { tag: u32, fields: Vec<VReg> },
}

impl Agg {
    fn fields(&self) -> &[VReg] {
        match self {
            Agg::Struct(f) | Agg::Enum { fields: f, .. } => f,
        }
    }
}

#[derive(Clone, Default)]
struct Avail {
    exprs: HashMap<Key, VReg>,
    aggs: HashMap<VReg, Agg>,
}

impl Avail {
    fn kill_reg(&mut self, r: VReg) {
        self.exprs.retain(|k, v| *v != r && !k.operands().contains(&r));
        self.aggs.retain(|a, g| *a != r && !g.fields().contains(&r));
    }

    fn kill_memory(&mut self) {
        self.exprs.retain(|k, _| !k.reads_mutable_memory());
        // struct fields are mutable, enum payloads are not
        self.aggs.retain(|_, g| matches!(g, Agg::Enum { .. }));
    }
}

/// The expression key of a pure instruction, when every register involved is stable.
fn key_of(f: &IrFunc, inst: &Inst) -> Option<Key> {
    let ok = |rs: &[VReg]| rs.iter().all(|r| !f.pinned[r.index()]);
    let key = match inst {
        Inst::Arith { op, ty, lhs, rhs, .. } => Key::Arith(*op, *ty, *lhs, *rhs),
        Inst::Cmp { op, ty, lhs, rhs, .. } => Key::Cmp(*op, *ty, *lhs, *rhs),
        Inst::Neg { ty, src, .. } => Key::Neg(*ty, *src),
        Inst::BitNot { ty, src, .. } => Key::BitNot(*ty, *src),
        Inst::BoolNot { src, .. } => Key::BoolNot(*src),
        Inst::Cast { from, to, src, .. } => Key::Cast(*from, *to, *src),
        Inst::GetField { obj, idx, .. } => Key::GetField(*obj, *idx),
        Inst::EnumTag { obj, .. } => Key::EnumTag(*obj),
        Inst::EnumField { obj, idx, .. } => Key::EnumField(*obj, *idx),
        Inst::ArrayGet { arr, idx, idx_ty, .. } => Key::ArrayGet(*arr, *idx, *idx_ty),
        Inst::Intrinsic { b, desc, args, .. } if b.is_pure() || b.reads_memory_only() => {
            Key::Intrinsic(*b, *desc, args.clone())
        }
        _ => return None,
    };
    let dst = inst.def()?;
    (ok(&key.operands()) && ok(&[dst])).then_some(key)
}

/// Rewrites `inst` using what is known; returns `true` when it changed.
fn rewrite(f: &IrFunc, inst: &mut Inst, avail: &Avail) -> bool {
    let same_ty = |a: VReg, b: VReg| f.reg_ty(a) == f.reg_ty(b);
    let forwarded = match inst {
        Inst::GetField { dst, obj, idx } => match avail.aggs.get(obj) {
            Some(Agg::Struct(fields)) => fields.get(*idx as usize).map(|&s| Inst::Move { dst: *dst, src: s }),
            _ => None,
        },
        Inst::EnumTag { dst, obj } => match avail.aggs.get(obj) {
            Some(Agg::Enum { tag, .. }) => Some(Inst::Const { dst: *dst, val: Const::Bits(*tag as u64) }),
            _ => None,
        },
        Inst::EnumField { dst, obj, idx } => match avail.aggs.get(obj) {
            Some(Agg::Enum { fields, .. }) => fields.get(*idx as usize).map(|&s| Inst::Move { dst: *dst, src: s }),
            _ => None,
        },
        _ => None,
    };
    if let Some(new) = forwarded {
        let ok = match &new {
            Inst::Move { dst, src } => same_ty(*dst, *src) && !f.pinned[src.index()] && !f.pinned[dst.index()],
            Inst::Const { dst, .. } => matches!(f.reg_ty(*dst), IrTy::Num(_)) && !f.pinned[dst.index()],
            _ => false,
        };
        if ok {
            *inst = new;
            return true;
        }
    }
    if let (Some(key), Some(dst)) = (key_of(f, inst), inst.def()) {
        if let Some(&have) = avail.exprs.get(&key) {
            if have != dst && same_ty(have, dst) {
                *inst = Inst::Move { dst, src: have };
                return true;
            }
        }
    }
    false
}

fn record(f: &IrFunc, inst: &Inst, avail: &mut Avail) {
    let Some(dst) = inst.def() else { return };
    let stable = |rs: &[VReg]| rs.iter().all(|r| !f.pinned[r.index()]) && !f.pinned[dst.index()];
    match inst {
        Inst::NewStruct { fields, .. } if stable(fields) && !fields.contains(&dst) => {
            avail.aggs.insert(dst, Agg::Struct(fields.clone()));
        }
        Inst::NewEnum { tag, fields, .. } if stable(fields) && !fields.contains(&dst) => {
            avail.aggs.insert(dst, Agg::Enum { tag: *tag, fields: fields.clone() });
        }
        Inst::Move { src, .. } if *src != dst && stable(&[*src]) => {
            if let Some(a) = avail.aggs.get(src).cloned() {
                if !a.fields().contains(&dst) {
                    avail.aggs.insert(dst, a);
                }
            }
        }
        _ => {
            if let Some(key) = key_of(f, inst) {
                if !key.operands().contains(&dst) {
                    avail.exprs.insert(key, dst);
                }
            }
        }
    }
}

/// Table size beyond which the knowledge is dropped (keeps the pass linear in practice).
const MAX_FACTS: usize = 160;

fn forward_values(f: &mut IrFunc) -> usize {
    let n = f.blocks.len();
    let preds = predecessors(f);
    // reverse post-order, so a unique predecessor is processed first
    let mut order = Vec::with_capacity(n);
    let mut seen = vec![false; n];
    let mut stack = vec![(0usize, 0usize)];
    seen[0] = true;
    while let Some((b, i)) = stack.pop() {
        let succ = f.blocks[b].term.successors();
        if i < succ.len() {
            stack.push((b, i + 1));
            let s = succ[i].index();
            if !seen[s] {
                seen[s] = true;
                stack.push((s, 0));
            }
        } else {
            order.push(b);
        }
    }
    order.reverse();

    let mut ends: Vec<Option<Avail>> = vec![None; n];
    let mut changes = 0;
    for &b in &order {
        let mut avail = match preds[b].as_slice() {
            [p] if *p != b => ends[*p].clone().unwrap_or_default(),
            _ => Avail::default(),
        };
        let mut insts = std::mem::take(&mut f.blocks[b].insts);
        for inst in insts.iter_mut() {
            if rewrite(f, inst, &avail) {
                changes += 1;
            }
            if clobbers_memory(inst) {
                avail.kill_memory();
            }
            // facts are computed from the operands before the definition invalidates anything
            let mut next = Avail::default();
            record(f, inst, &mut next);
            if let Some(d) = inst.def() {
                avail.kill_reg(d);
            }
            avail.exprs.extend(next.exprs);
            avail.aggs.extend(next.aggs);
            if avail.exprs.len() + avail.aggs.len() > MAX_FACTS {
                avail = Avail::default();
            }
        }
        // a self-move is nothing
        let before = insts.len();
        insts.retain(|i| !matches!(i, Inst::Move { dst, src } if dst == src));
        changes += before - insts.len();
        f.blocks[b].insts = insts;

        if let Term::BranchTag { obj, tag, then, els } = f.blocks[b].term {
            if let Some(Agg::Enum { tag: known, .. }) = avail.aggs.get(&obj) {
                f.blocks[b].term = Term::Jump(if *known == tag { then } else { els });
                changes += 1;
            }
        }
        ends[b] = Some(avail);
    }
    changes
}

// ---- algebraic identities --------------------------------------------------------------------------

fn algebra(f: &mut IrFunc) -> usize {
    let defs = def_counts(f);
    let mut consts: HashMap<VReg, u64> = HashMap::default();
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Const { dst, val: Const::Bits(v) } = i {
                if defs[dst.index()] == 1 && !f.pinned[dst.index()] {
                    consts.insert(*dst, *v);
                }
            }
        }
    }
    let mut changes = 0;
    for bi in 0..f.blocks.len() {
        for ii in 0..f.blocks[bi].insts.len() {
            let Inst::Arith { dst, op, ty, lhs, rhs } = f.blocks[bi].insts[ii] else { continue };
            if !ty.is_int() || f.pinned[dst.index()] {
                continue;
            }
            let (l, r) = (consts.get(&lhs).copied(), consts.get(&rhs).copied());
            let movable = |s: VReg| !f.pinned[s.index()] && f.reg_ty(s) == f.reg_ty(dst);
            use ArithOp::*;
            let new = match (op, l, r) {
                (Add | Sub | Or | Xor | Shl | Shr, _, Some(0)) if movable(lhs) => Some(Inst::Move { dst, src: lhs }),
                (Add | Or | Xor, Some(0), _) if movable(rhs) => Some(Inst::Move { dst, src: rhs }),
                (Mul | Div, _, Some(1)) if movable(lhs) => Some(Inst::Move { dst, src: lhs }),
                (Mul, Some(1), _) if movable(rhs) => Some(Inst::Move { dst, src: rhs }),
                (Mul | And, _, Some(0)) | (Mul | And, Some(0), _) => Some(Inst::Const { dst, val: Const::Bits(0) }),
                (Rem, _, Some(1)) => Some(Inst::Const { dst, val: Const::Bits(0) }),
                _ => None,
            };
            if let Some(new) = new {
                f.blocks[bi].insts[ii] = new;
                changes += 1;
            }
        }
    }
    changes
}

// ---- print(a + b) --------------------------------------------------------------------------------------

fn split_print_concat(f: &mut IrFunc) -> usize {
    let uses = use_counts(f);
    let defs = def_counts(f);
    let mut changes = 0;
    for bi in 0..f.blocks.len() {
        let mut ii = 0;
        while ii < f.blocks[bi].insts.len() {
            let Inst::Intrinsic { b: pb @ (Builtin::Print | Builtin::Println), desc, args: pargs, .. } =
                f.blocks[bi].insts[ii].clone()
            else {
                ii += 1;
                continue;
            };
            let &[t] = pargs.as_slice() else {
                ii += 1;
                continue;
            };
            if uses[t.index()] != 1 || defs[t.index()] != 1 || f.pinned[t.index()] {
                ii += 1;
                continue;
            }
            // the concatenation that produced `t`, earlier in this block
            let Some(ci) = (0..ii).rev().find(|&j| f.blocks[bi].insts[j].def() == Some(t)) else {
                ii += 1;
                continue;
            };
            let Inst::Intrinsic { b: Builtin::StrConcat, args: cargs, .. } = f.blocks[bi].insts[ci].clone() else {
                ii += 1;
                continue;
            };
            let &[a, b] = cargs.as_slice() else {
                ii += 1;
                continue;
            };
            // the pieces must still hold the same strings at the print
            let stable = f.blocks[bi].insts[ci + 1..ii].iter().all(|i| {
                i.def().map_or(true, |d| d != a && d != b) && !matches!(i, Inst::Call { .. } | Inst::CallClosure { .. })
            });
            if !stable || f.pinned[a.index()] && ii != ci + 1 || f.pinned[b.index()] && ii != ci + 1 {
                ii += 1;
                continue;
            }
            let unit = f.new_reg(IrTy::Unit);
            let first = Inst::Intrinsic { dst: unit, b: Builtin::Print, desc, args: vec![a] };
            let Inst::Intrinsic { args, .. } = &mut f.blocks[bi].insts[ii] else { unreachable!() };
            *args = vec![b];
            f.blocks[bi].insts.insert(ii, first);
            f.blocks[bi].insts.remove(ci);
            let _ = pb;
            changes += 1;
            // re-examine from the start of the pair: `a` may itself be a concatenation
            ii = ci;
        }
    }
    changes
}

// ---- dead element loads --------------------------------------------------------------------------------

/// Whether `idx` can never be negative: every definition is a non-negative constant or a
/// self-increment by a positive constant (overflow traps, so it cannot wrap around).
fn never_negative(f: &IrFunc, defs: &[u32], idx: VReg, ty: NumTy) -> bool {
    if !ty.is_signed() {
        return true;
    }
    if idx.index() < f.params as usize || f.pinned[idx.index()] {
        return false;
    }
    let single_const = |r: VReg| -> Option<i128> {
        if defs[r.index()] != 1 || f.pinned[r.index()] {
            return None;
        }
        f.blocks.iter().flat_map(|b| &b.insts).find_map(|i| match i {
            Inst::Const { dst, val: Const::Bits(v) } if *dst == r => Some(decode_int(ty, *v)),
            _ => None,
        })
    };
    let mut any = false;
    for i in f.blocks.iter().flat_map(|b| &b.insts) {
        if i.def() != Some(idx) {
            continue;
        }
        any = true;
        let ok = match i {
            Inst::Const { val: Const::Bits(v), .. } => decode_int(ty, *v) >= 0,
            Inst::Arith { op: ArithOp::Add, ty: aty, lhs, rhs, .. } if *aty == ty => {
                (*lhs == idx && single_const(*rhs).is_some_and(|c| c > 0))
                    || (*rhs == idx && single_const(*lhs).is_some_and(|c| c > 0))
            }
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    any
}

fn drop_inrange_loads(f: &mut IrFunc) -> usize {
    let preds = predecessors(f);
    let uses = use_counts(f);
    let defs = def_counts(f);
    let mut changes = 0;
    for bi in 0..f.blocks.len() {
        let [p] = preds[bi].as_slice() else { continue };
        let p = *p;
        let Term::Branch { cond, then, els } = f.blocks[p].term else { continue };
        if then.index() != bi || els.index() == bi || p == bi {
            continue;
        }
        // `cond = lt idx, len` and `len = ArrLen(arr)` in the predecessor, nothing disturbing in between
        let pb = &f.blocks[p].insts;
        let Some(ci) = pb.iter().rposition(|i| i.def() == Some(cond)) else { continue };
        let Inst::Cmp { op: CmpOp::Lt, ty: CmpTy::Num(nt), lhs: idx, rhs: len, .. } = pb[ci] else { continue };
        let Some(li) = pb[..ci].iter().rposition(|i| i.def() == Some(len)) else { continue };
        let Inst::Intrinsic { b: Builtin::ArrLen, args: ref largs, .. } = pb[li] else { continue };
        let &[arr] = largs.as_slice() else { continue };
        let disturbed = |insts: &[Inst], watched: &[VReg]| {
            insts.iter().any(|i| clobbers_memory(i) || i.def().is_some_and(|d| watched.contains(&d)))
        };
        if disturbed(&pb[li + 1..ci], &[arr, idx, len]) || disturbed(&pb[ci + 1..], &[arr, idx, len, cond]) {
            continue;
        }
        if f.pinned[arr.index()] || f.pinned[idx.index()] || !nt.is_int() || !never_negative(f, &defs, idx, nt) {
            continue;
        }
        // the first instructions of this block, up to the dead load
        let mut j = 0;
        while j < f.blocks[bi].insts.len() {
            let i = &f.blocks[bi].insts[j];
            if let Inst::ArrayGet { dst, arr: a, idx: x, idx_ty } = *i {
                if a == arr && x == idx && idx_ty == nt && uses[dst.index()] == 0 && !f.pinned[dst.index()] {
                    f.blocks[bi].insts.remove(j);
                    changes += 1;
                    continue;
                }
            }
            if clobbers_memory(i) || i.def().is_some_and(|d| d == arr || d == idx) {
                break;
            }
            j += 1;
        }
    }
    changes
}
