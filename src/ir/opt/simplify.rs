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
use crate::ir::effects::FuncEffects;
use crate::ir::types::*;
use crate::runtime::numeric::{decode_int, ArithOp, CmpOp};
use crate::runtime::NumTy;

/// Runs all simplifications on `f`; returns how many rewrites happened.
pub fn simplify_func(f: &mut IrFunc, fx: &FuncEffects) -> usize {
    let mut n = super::copies::copy_propagate(f);
    // the escape analysis is only needed where something is allocated, and only again after a
    // pass has changed the function
    if f.blocks.iter().flat_map(|b| &b.insts).any(|i| {
        matches!(
            i,
            Inst::NewStruct { .. }
                | Inst::NewEnum { .. }
                | Inst::NewArray { .. }
                | Inst::NewMap { .. }
                | Inst::NewSet { .. }
        )
    }) {
        let mut classes = crate::ir::escape::classify(f, fx.escapes());
        for step in 0..3 {
            let done = match step {
                0 => super::sroa::sroa_func(f, &classes),
                1 => super::sroa::sroa_classes(f, &classes),
                _ => super::sroa::dead_objects(f, &classes),
            };
            n += done;
            if done > 0 && step < 2 {
                classes = crate::ir::escape::classify(f, fx.escapes());
            }
        }
    }
    n + forward_values(f, fx) + algebra(f) + branch_on_negation(f) + split_print_concat(f) + drop_inrange_loads(f, fx)
}

// ---- helpers ------------------------------------------------------------------------------------

/// A map keyed by register number, stored densely (registers are small consecutive integers, so a
/// vector beats a hash table both in speed and in allocations).
#[derive(Debug, Clone)]
pub(super) struct RegMap<T> {
    slots: Vec<Option<T>>,
}

impl<T> RegMap<T> {
    pub(super) fn new(regs: usize) -> RegMap<T> {
        RegMap { slots: (0..regs).map(|_| None).collect() }
    }

    pub(super) fn insert(&mut self, r: VReg, v: T) {
        self.slots[r.index()] = Some(v);
    }

    pub(super) fn get(&self, r: &VReg) -> Option<&T> {
        self.slots.get(r.index()).and_then(|s| s.as_ref())
    }
}

/// The value of every register whose only definition is a numeric constant and which no closure can
/// change.
pub(super) fn single_def_bits(f: &IrFunc, defs: &[u32]) -> RegMap<u64> {
    let mut out = RegMap::new(f.regs.len());
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Const { dst, val: Const::Bits(v) } = i {
                if defs[dst.index()] == 1 && !f.pinned[dst.index()] {
                    out.insert(*dst, *v);
                }
            }
        }
    }
    out
}

/// Moves `d = t` that finish an assignment: `t` is computed earlier in the block by an instruction
/// that is not a move, has no other use and nothing in between touches `d`. The move coalescing
/// turns the pair into `d = op ..`; reading `t` in place of `d` later would keep `t` alive and
/// prevent that, so copy propagation leaves these alone.
pub(super) fn result_moves(f: &IrFunc) -> crate::fx::FxHashSet<(usize, usize)> {
    let defs = def_counts(f);
    let uses = use_counts(f);
    let mut out: crate::fx::FxHashSet<(usize, usize)> = Default::default();
    for (bi, b) in f.blocks.iter().enumerate() {
        for (ii, i) in b.insts.iter().enumerate() {
            let Inst::Move { dst, src } = i else { continue };
            // follow `t = op; u = t; d = u` back to the instruction that computes the value
            let mut cur = *src;
            let mut upto = ii;
            let found = loop {
                if cur == *dst || defs[cur.index()] != 1 || uses[cur.index()] != 1 {
                    break false;
                }
                let mut next = None;
                for k in (0..upto).rev() {
                    let prev = &b.insts[k];
                    if prev.def() == Some(cur) {
                        match prev {
                            Inst::Move { src: s2, .. } => next = Some((*s2, k)),
                            _ => next = Some((cur, usize::MAX)),
                        }
                        break;
                    }
                    let mut touches = prev.def() == Some(*dst);
                    prev.for_each_use(|r| touches |= r == *dst);
                    if touches {
                        break;
                    }
                }
                match next {
                    Some((_, usize::MAX)) => break true,
                    Some((s2, k)) => {
                        cur = s2;
                        upto = k;
                    }
                    None => break false,
                }
            };
            if found {
                out.insert((bi, ii));
            }
        }
    }
    out
}

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
        let (slots, n) = b.term.successor_slots();
        for s in &slots[..n] {
            preds[s.index()].push(i);
        }
    }
    preds
}

/// Whether executing `i` may write memory other code can read (calls count as what their callee does).
pub(super) fn clobbers_memory(i: &Inst, fx: &FuncEffects) -> bool {
    i.writes_memory(fx)
}

/// `len`-like builtins: a store to a field or an element cannot change what they return.
fn length_only(b: Builtin) -> bool {
    use Builtin::*;
    matches!(b, ArrLen | ArrIsEmpty | MapLen | MapIsEmpty | SetLen | SetIsEmpty)
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
    /// A map lookup; it traps when the key is absent, so a second one with nothing in between is safe.
    MapGet(VReg, VReg),
    Intrinsic(Builtin, Option<DescId>, Vec<VReg>),
    /// A call to a function without writes, I/O or allocation; the flag: it reads memory.
    Call(FuncId, Vec<VReg>, bool),
    /// The current value of a global variable.
    Global(GlobalSlot),
}

impl Key {
    fn operands(&self) -> Vec<VReg> {
        match self {
            Key::Arith(_, _, a, b) | Key::Cmp(_, _, a, b) | Key::ArrayGet(a, b, _) => vec![*a, *b],
            Key::MapGet(a, b) => vec![*a, *b],
            Key::Neg(_, a)
            | Key::BitNot(_, a)
            | Key::BoolNot(a)
            | Key::Cast(_, _, a)
            | Key::GetField(a, _)
            | Key::EnumTag(a)
            | Key::EnumField(a, _) => vec![*a],
            Key::Intrinsic(_, _, args) | Key::Call(_, args, _) => args.clone(),
            Key::Global(_) => Vec::new(),
        }
    }

    fn visit_operands(&self, mut f: impl FnMut(VReg)) {
        match self {
            Key::Arith(_, _, a, b) | Key::Cmp(_, _, a, b) | Key::ArrayGet(a, b, _) | Key::MapGet(a, b) => {
                f(*a);
                f(*b);
            }
            Key::Neg(_, a)
            | Key::BitNot(_, a)
            | Key::BoolNot(a)
            | Key::Cast(_, _, a)
            | Key::GetField(a, _)
            | Key::EnumTag(a)
            | Key::EnumField(a, _) => f(*a),
            Key::Intrinsic(_, _, args) | Key::Call(_, args, _) => args.iter().for_each(|&r| f(r)),
            Key::Global(_) => {}
        }
    }

    /// Whether `r` is one of the operands (without building the list).
    fn mentions(&self, r: VReg) -> bool {
        match self {
            Key::Arith(_, _, a, b) | Key::Cmp(_, _, a, b) | Key::ArrayGet(a, b, _) | Key::MapGet(a, b) => {
                *a == r || *b == r
            }
            Key::Neg(_, a)
            | Key::BitNot(_, a)
            | Key::BoolNot(a)
            | Key::Cast(_, _, a)
            | Key::GetField(a, _)
            | Key::EnumTag(a)
            | Key::EnumField(a, _) => *a == r,
            Key::Intrinsic(_, _, args) | Key::Call(_, args, _) => args.contains(&r),
            Key::Global(_) => false,
        }
    }

    /// Whether the value can change when memory is written.
    fn reads_mutable_memory(&self) -> bool {
        match self {
            Key::GetField(..) | Key::ArrayGet(..) | Key::MapGet(..) | Key::Global(_) => true,
            Key::Intrinsic(b, ..) => b.effects().reads(),
            Key::Call(_, _, reads) => *reads,
            _ => false,
        }
    }
}

#[derive(Clone, PartialEq)]
enum Agg {
    Struct(Vec<VReg>),
    Enum {
        tag: u32,
        fields: Vec<VReg>,
    },
    /// An array literal that nothing has written to since.
    Array(Vec<VReg>),
}

impl Agg {
    fn fields(&self) -> &[VReg] {
        match self {
            Agg::Struct(f) | Agg::Array(f) | Agg::Enum { fields: f, .. } => f,
        }
    }
}

#[derive(Clone, Default)]
struct Avail {
    exprs: HashMap<Key, VReg>,
    aggs: HashMap<VReg, Agg>,
    /// `dst -> src` for registers that are currently plain copies of another register.
    copies: HashMap<VReg, VReg>,
    /// Registers that appear in some fact (once set a bit stays, so this may over-approximate): a
    /// definition of any other register cannot invalidate anything and skips the scan.
    mentioned: Vec<u64>,
}

impl Avail {
    fn note(&mut self, r: VReg) {
        let (w, bit) = (r.index() / 64, r.index() % 64);
        if self.mentioned.len() <= w {
            self.mentioned.resize(w + 1, 0);
        }
        self.mentioned[w] |= 1 << bit;
    }

    fn note_key(&mut self, k: &Key, value: VReg) {
        self.note(value);
        k.visit_operands(|r| self.note(r));
    }

    fn is_mentioned(&self, r: VReg) -> bool {
        self.mentioned.get(r.index() / 64).is_some_and(|w| w & (1 << (r.index() % 64)) != 0)
    }

    fn kill_reg(&mut self, r: VReg) {
        if !self.is_mentioned(r) {
            return;
        }
        self.exprs.retain(|k, v| *v != r && !k.mentions(r));
        self.aggs.retain(|a, g| *a != r && !g.fields().contains(&r));
        self.copies.retain(|d, c| *d != r && *c != r);
    }

    /// What holds on every path into a join: the facts both sides agree on.
    fn meet(mut self, other: &Avail) -> Avail {
        self.exprs.retain(|k, v| other.exprs.get(k) == Some(v));
        self.aggs.retain(|r, g| other.aggs.get(r) == Some(g));
        self.copies.retain(|d, c| other.copies.get(d) == Some(c));
        self
    }

    fn kill_memory(&mut self) {
        self.exprs.retain(|k, _| !k.reads_mutable_memory());
        // struct fields are mutable, enum payloads are not
        self.aggs.retain(|_, g| matches!(g, Agg::Enum { .. }));
    }
}

/// The expression key of a pure instruction, when every register involved is stable.
fn key_of(f: &IrFunc, inst: &Inst, canon: &RegMap<VReg>, fx: &FuncEffects) -> Option<Key> {
    let ok = |rs: &[VReg]| rs.iter().all(|r| !f.pinned[r.index()]);
    let c = |r: &VReg| canon.get(r).copied().unwrap_or(*r);
    let key = match inst {
        Inst::Arith { op, ty, lhs, rhs, .. } => Key::Arith(*op, *ty, c(lhs), c(rhs)),
        Inst::Cmp { op, ty, lhs, rhs, .. } => Key::Cmp(*op, *ty, c(lhs), c(rhs)),
        Inst::Neg { ty, src, .. } => Key::Neg(*ty, c(src)),
        Inst::BitNot { ty, src, .. } => Key::BitNot(*ty, c(src)),
        Inst::BoolNot { src, .. } => Key::BoolNot(c(src)),
        Inst::Cast { from, to, src, .. } => Key::Cast(*from, *to, c(src)),
        Inst::GetField { obj, idx, .. } => Key::GetField(c(obj), *idx),
        Inst::EnumTag { obj, .. } => Key::EnumTag(c(obj)),
        Inst::EnumField { obj, idx, .. } => Key::EnumField(c(obj), *idx),
        Inst::ArrayGet { arr, idx, idx_ty, .. } => Key::ArrayGet(c(arr), c(idx), *idx_ty),
        Inst::MapGet { map, key, .. } => Key::MapGet(c(map), c(key)),
        Inst::Intrinsic { b, desc, args, .. } if b.effects().reusable() => {
            Key::Intrinsic(*b, *desc, args.iter().map(c).collect())
        }
        Inst::GetGlobal { slot, .. } => Key::Global(*slot),
        Inst::Call { func, args, .. } if fx.of(*func).reusable() => {
            Key::Call(*func, args.iter().map(c).collect(), fx.of(*func).reads())
        }
        _ => return None,
    };
    let dst = inst.def()?;
    (ok(&key.operands()) && ok(&[dst])).then_some(key)
}

/// Rewrites `inst` using what is known; returns `true` when it changed.
fn rewrite(
    f: &IrFunc,
    inst: &mut Inst,
    avail: &Avail,
    canon: &RegMap<VReg>,
    consts: &RegMap<u64>,
    fx: &FuncEffects,
) -> bool {
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
        // an array literal that nothing wrote to: its length and elements are known
        Inst::Intrinsic { dst, b: b @ (Builtin::ArrLen | Builtin::ArrIsEmpty), args, .. } => match args.as_slice() {
            [arr] => match avail.aggs.get(arr) {
                Some(Agg::Array(elems)) => {
                    let n = if *b == Builtin::ArrLen { elems.len() } else { elems.is_empty() as usize };
                    Some(Inst::Const { dst: *dst, val: Const::Bits(n as u64) })
                }
                _ => None,
            },
            _ => None,
        },
        Inst::ArrayGet { dst, arr, idx, idx_ty } => match (avail.aggs.get(arr), consts.get(idx)) {
            (Some(Agg::Array(elems)), Some(&bits)) => {
                let i = decode_int(*idx_ty, bits);
                (i >= 0 && (i as usize) < elems.len()).then(|| Inst::Move { dst: *dst, src: elems[i as usize] })
            }
            _ => None,
        },
        _ => None,
    };
    if let Some(new) = forwarded {
        let ok = match &new {
            Inst::Move { dst, src } => same_ty(*dst, *src) && !f.pinned[src.index()] && !f.pinned[dst.index()],
            Inst::Const { dst, .. } => matches!(f.reg_ty(*dst), IrTy::Num(_) | IrTy::Bool) && !f.pinned[dst.index()],
            _ => false,
        };
        if ok {
            *inst = new;
            return true;
        }
    }
    if let (Some(key), Some(dst)) = (key_of(f, inst, canon, fx), inst.def()) {
        if let Some(&have) = avail.exprs.get(&key) {
            if have != dst && same_ty(have, dst) {
                *inst = Inst::Move { dst, src: have };
                return true;
            }
        }
    }
    false
}

fn record(f: &IrFunc, inst: &Inst, avail: &mut Avail, canon: &RegMap<VReg>, fx: &FuncEffects) {
    let Some(dst) = inst.def() else { return };
    let stable = |rs: &[VReg]| rs.iter().all(|r| !f.pinned[r.index()]) && !f.pinned[dst.index()];
    match inst {
        Inst::NewStruct { fields, .. } if stable(fields) && !fields.contains(&dst) => {
            avail.aggs.insert(dst, Agg::Struct(fields.clone()));
        }
        Inst::NewArray { elems, .. } if stable(elems) && !elems.contains(&dst) => {
            avail.aggs.insert(dst, Agg::Array(elems.clone()));
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
            if let Some(key) = key_of(f, inst, canon, fx) {
                if !key.operands().contains(&dst) {
                    avail.exprs.insert(key, dst);
                }
            }
        }
    }
}

/// Table size beyond which the knowledge is dropped (keeps the pass linear in practice).
const MAX_FACTS: usize = 160;

fn forward_values(f: &mut IrFunc, fx: &FuncEffects) -> usize {
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

    // registers that hold the same constant all over the function count as one when comparing keys
    let defs = def_counts(f);
    let mut canon: RegMap<VReg> = RegMap::new(f.regs.len());
    let const_vals = single_def_bits(f, &defs);
    let mut first_const: HashMap<(IrTy, u64), VReg> = HashMap::default();
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Const { dst, .. } = i {
                if let Some(v) = const_vals.get(dst) {
                    let rep = *first_const.entry((f.reg_ty(*dst), *v)).or_insert(*dst);
                    if rep != *dst {
                        canon.insert(*dst, rep);
                    }
                }
            }
        }
    }

    let result_moves = result_moves(f);
    let mut ends: Vec<Option<Avail>> = vec![None; n];
    let mut changes = 0;
    for &b in &order {
        // what is known at the start of the block: the agreement of all predecessors, provided every
        // one of them has been processed (a back edge has not, so loop headers start empty)
        let incoming: Vec<usize> = preds[b].iter().copied().filter(|&p| seen[p]).collect();
        // a predecessor that only leads here hands its state over instead of copying it
        let sole = match incoming.as_slice() {
            [p] if *p != b && f.blocks[*p].term.successors().len() == 1 => ends[*p].take(),
            _ => None,
        };
        let mut avail = match incoming.split_first() {
            _ if sole.is_some() => sole.unwrap_or_default(),
            Some((&first, rest)) if first != b && ends[first].is_some() => rest
                .iter()
                .try_fold(ends[first].clone().unwrap_or_default(), |acc, &p| {
                    ends[p].as_ref().filter(|_| p != b).map(|e| acc.meet(e))
                })
                .unwrap_or_default(),
            _ => Avail::default(),
        };
        let mut insts = std::mem::take(&mut f.blocks[b].insts);
        for (ii, inst) in insts.iter_mut().enumerate() {
            // read through plain copies: the original register is just as good
            if !avail.copies.is_empty() {
                inst.for_each_use_mut(|r| {
                    if let Some(&c) = avail.copies.get(r) {
                        *r = c;
                        changes += 1;
                    }
                });
            }
            if rewrite(f, inst, &avail, &canon, &const_vals, fx) {
                changes += 1;
            }
            // a store makes the stored value the known content of the location; other stores
            // and calls forget what they might change
            let mut stored: Option<(Key, VReg)> = None;
            match &*inst {
                Inst::SetField { obj, idx, src } => {
                    avail.exprs.retain(|k, _| match k {
                        Key::GetField(_, i) => i != idx,
                        Key::Call(_, _, reads) => !reads,
                        Key::Intrinsic(b, ..) => length_only(*b) || !b.effects().reads(),
                        _ => true,
                    });
                    avail.aggs.retain(|_, g| matches!(g, Agg::Enum { .. }));
                    if !f.pinned[obj.index()] && !f.pinned[src.index()] {
                        stored = Some((Key::GetField(canon.get(obj).copied().unwrap_or(*obj), *idx), *src));
                    }
                }
                Inst::ArraySet { arr, idx, idx_ty, src } => {
                    avail.exprs.retain(|k, _| match k {
                        Key::ArrayGet(..) => false,
                        Key::Call(_, _, reads) => !reads,
                        Key::Intrinsic(b, ..) => length_only(*b) || !b.effects().reads(),
                        _ => true,
                    });
                    avail.aggs.retain(|_, g| !matches!(g, Agg::Array(_)));
                    if !f.pinned[arr.index()] && !f.pinned[idx.index()] && !f.pinned[src.index()] {
                        stored = Some((
                            Key::ArrayGet(
                                canon.get(arr).copied().unwrap_or(*arr),
                                canon.get(idx).copied().unwrap_or(*idx),
                                *idx_ty,
                            ),
                            *src,
                        ));
                    }
                }
                Inst::SetGlobal { slot, src } => {
                    avail.exprs.retain(|k, _| match k {
                        Key::Global(g) => g != slot,
                        Key::Call(_, _, reads) => !reads,
                        _ => true,
                    });
                    if !f.pinned[src.index()] {
                        stored = Some((Key::Global(*slot), *src));
                    }
                }
                i if clobbers_memory(i, fx) => avail.kill_memory(),
                _ => {}
            }
            // facts are computed from the operands before the definition invalidates anything
            let mut next = Avail::default();
            record(f, inst, &mut next, &canon, fx);
            if let Some(d) = inst.def() {
                avail.kill_reg(d);
            }
            for (k, v) in &next.exprs {
                avail.note_key(k, *v);
            }
            for (r, g) in &next.aggs {
                avail.note(*r);
                g.fields().iter().for_each(|&x| avail.note(x));
            }
            avail.exprs.extend(next.exprs);
            avail.aggs.extend(next.aggs);
            if let Inst::Move { dst, src } = *inst {
                if dst != src
                    && !result_moves.contains(&(b, ii))
                    && !f.pinned[dst.index()]
                    && !f.pinned[src.index()]
                    && f.reg_ty(dst) == f.reg_ty(src)
                {
                    avail.note(dst);
                    avail.note(src);
                    avail.copies.insert(dst, src);
                }
            }
            if let Some((key, value)) = stored {
                avail.note_key(&key, value);
                avail.exprs.insert(key, value);
            }
            if avail.exprs.len() + avail.aggs.len() + avail.copies.len() > MAX_FACTS {
                avail = Avail::default();
            }
        }
        // a self-move is nothing
        let before = insts.len();
        insts.retain(|i| !matches!(i, Inst::Move { dst, src } if dst == src));
        changes += before - insts.len();
        f.blocks[b].insts = insts;

        match &mut f.blocks[b].term {
            Term::Branch { cond: r, .. } | Term::BranchTag { obj: r, .. } | Term::Return(r) => {
                if let Some(&c) = avail.copies.get(r) {
                    *r = c;
                    changes += 1;
                }
            }
            _ => {}
        }
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
    let consts = single_def_bits(f, &defs);
    let mut changes = 0;
    for bi in 0..f.blocks.len() {
        let mut ii = 0;
        while ii < f.blocks[bi].insts.len() {
            let inst = f.blocks[bi].insts[ii].clone();
            let new: Option<Inst> = match inst {
                Inst::Arith { dst, op, ty, lhs, rhs } if ty.is_int() && !f.pinned[dst.index()] => {
                    let (l, r) = (consts.get(&lhs).copied(), consts.get(&rhs).copied());
                    let movable = |s: VReg| !f.pinned[s.index()] && f.reg_ty(s) == f.reg_ty(dst);
                    use ArithOp::*;
                    match (op, l, r) {
                        (Add | Sub | Or | Xor | Shl | Shr, _, Some(0)) if movable(lhs) => {
                            Some(Inst::Move { dst, src: lhs })
                        }
                        (Add | Or | Xor, Some(0), _) if movable(rhs) => Some(Inst::Move { dst, src: rhs }),
                        (Mul | Div, _, Some(1)) if movable(lhs) => Some(Inst::Move { dst, src: lhs }),
                        (Mul, Some(1), _) if movable(rhs) => Some(Inst::Move { dst, src: rhs }),
                        (Mul | And, _, Some(0)) | (Mul | And, Some(0), _) => {
                            Some(Inst::Const { dst, val: Const::Bits(0) })
                        }
                        (Rem, _, Some(1)) => Some(Inst::Const { dst, val: Const::Bits(0) }),
                        // `x - x`, `x ^ x` are 0 whatever x is; `x & x`, `x | x` are x
                        (Sub | Xor, ..) if lhs == rhs => Some(Inst::Const { dst, val: Const::Bits(0) }),
                        (And | Or, ..) if lhs == rhs && movable(lhs) => Some(Inst::Move { dst, src: lhs }),
                        // unsigned division and remainder by a power of two: no zero check, no divide
                        (Div | Rem, _, Some(c)) if !ty.is_signed() && c > 1 && c.is_power_of_two() => {
                            let (op, amount_ty, amount) =
                                if op == Div { (Shr, NumTy::I64, c.trailing_zeros() as u64) } else { (And, ty, c - 1) };
                            let k = f.new_reg(IrTy::Num(amount_ty));
                            f.blocks[bi].insts.insert(ii, Inst::Const { dst: k, val: Const::Bits(amount) });
                            ii += 1;
                            Some(Inst::Arith { dst, op, ty, lhs, rhs: k })
                        }
                        _ => None,
                    }
                }
                // nothing unsigned is below zero: `0 <= x` and `x >= 0` hold, `x < 0` and `0 > x` do not
                Inst::Cmp { dst, op, ty: CmpTy::Num(n), lhs, rhs }
                    if n.is_int()
                        && !n.is_signed()
                        && !f.pinned[dst.index()]
                        && match op {
                            CmpOp::Le | CmpOp::Gt => consts.get(&lhs) == Some(&0),
                            CmpOp::Ge | CmpOp::Lt => consts.get(&rhs) == Some(&0),
                            _ => false,
                        } =>
                {
                    Some(Inst::Const { dst, val: Const::Bits(matches!(op, CmpOp::Le | CmpOp::Ge) as u64) })
                }
                // comparing a value with itself (not floats: NaN)
                Inst::Cmp { dst, op, ty, lhs, rhs }
                    if lhs == rhs && !f.pinned[dst.index()] && matches!(ty, CmpTy::Str | CmpTy::Num(_)) =>
                {
                    let int_like = match ty {
                        CmpTy::Str => true,
                        CmpTy::Num(n) => n.is_int(),
                    };
                    int_like.then_some(Inst::Const {
                        dst,
                        val: Const::Bits(matches!(op, CmpOp::Eq | CmpOp::Le | CmpOp::Ge) as u64),
                    })
                }
                Inst::Cast { dst, from, to, src }
                    if from == to
                        && !f.pinned[dst.index()]
                        && !f.pinned[src.index()]
                        && f.reg_ty(dst) == f.reg_ty(src) =>
                {
                    Some(Inst::Move { dst, src })
                }
                _ => None,
            };
            if let Some(new) = new {
                f.blocks[bi].insts[ii] = new;
                changes += 1;
            }
            ii += 1;
        }
    }
    changes
}

/// `v = !c; branch v ? A : B` is `branch c ? B : A` when `v` has no other use.
fn branch_on_negation(f: &mut IrFunc) -> usize {
    let uses = use_counts(f);
    let defs = def_counts(f);
    let mut changes = 0;
    for b in &mut f.blocks {
        let Term::Branch { cond, then, els } = b.term else { continue };
        if uses[cond.index()] != 1 || defs[cond.index()] != 1 || f.pinned[cond.index()] {
            continue;
        }
        let Some(pos) = b.insts.iter().rposition(|i| i.def() == Some(cond)) else { continue };
        let Inst::BoolNot { src, .. } = b.insts[pos] else { continue };
        if f.pinned[src.index()] || b.insts[pos + 1..].iter().any(|i| i.def() == Some(src)) {
            continue;
        }
        b.term = Term::Branch { cond: src, then: els, els: then };
        changes += 1;
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
                i.def().is_none_or(|d| d != a && d != b) && !matches!(i, Inst::Call { .. } | Inst::CallClosure { .. })
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

fn drop_inrange_loads(f: &mut IrFunc, fx: &FuncEffects) -> usize {
    let preds = predecessors(f);
    let uses = use_counts(f);
    let defs = def_counts(f);
    let mut changes = 0;
    for (bi, pred) in preds.iter().enumerate() {
        let [p] = pred.as_slice() else { continue };
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
            insts.iter().any(|i| clobbers_memory(i, fx) || i.def().is_some_and(|d| watched.contains(&d)))
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
            if clobbers_memory(i, fx) || i.def().is_some_and(|d| d == arr || d == idx) {
                break;
            }
            j += 1;
        }
    }
    changes
}
