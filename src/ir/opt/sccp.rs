//! Sparse-conditional-constant-style propagation over the whole CFG.
//!
//! `fold` only knows registers with a single definition and, inside one block, the constants set
//! earlier in it. This pass runs the classic optimistic dataflow: every register is `Top` (nothing
//! known yet), a constant, or `Bottom` (varies); states merge at joins; a conditional branch whose
//! condition is a known constant only sends the state along the edge it takes, so code behind the
//! other edge never contributes (and is cut off at the end). Arithmetic is evaluated with the run-time
//! semantics in `runtime::numeric`; an operation that would trap stays as it is.
//!
//! Registers captured by closures (pinned) can change behind the analysis and are always `Bottom`.

use crate::ir::types::*;
use crate::runtime::numeric;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Lat {
    Top,
    Const(u64),
    Bottom,
}

fn meet(a: Lat, b: Lat) -> Lat {
    match (a, b) {
        (Lat::Top, x) | (x, Lat::Top) => x,
        (Lat::Const(x), Lat::Const(y)) if x == y => Lat::Const(x),
        _ => Lat::Bottom,
    }
}

fn unary(a: Lat, f: impl FnOnce(u64) -> Option<u64>) -> Lat {
    match a {
        Lat::Const(x) => f(x).map_or(Lat::Bottom, Lat::Const),
        Lat::Top => Lat::Top,
        Lat::Bottom => Lat::Bottom,
    }
}

fn binary(a: Lat, b: Lat, f: impl FnOnce(u64, u64) -> Option<u64>) -> Lat {
    match (a, b) {
        (Lat::Const(x), Lat::Const(y)) => f(x, y).map_or(Lat::Bottom, Lat::Const),
        (Lat::Bottom, _) | (_, Lat::Bottom) => Lat::Bottom,
        _ => Lat::Top,
    }
}

/// The value `inst` leaves in its destination, given the state before it.
fn value_of(inst: &Inst, st: &[Lat]) -> Lat {
    match inst {
        Inst::Const { val: Const::Bits(v), .. } => Lat::Const(*v),
        Inst::Move { src, .. } => st[src.index()],
        Inst::Arith { op, ty, lhs, rhs, .. } => {
            binary(st[lhs.index()], st[rhs.index()], |a, b| numeric::arith(*op, *ty, a, b).ok())
        }
        Inst::Cmp { op, ty: CmpTy::Num(n), lhs, rhs, .. } => {
            binary(st[lhs.index()], st[rhs.index()], |a, b| Some(numeric::compare(*op, *n, a, b) as u64))
        }
        Inst::Neg { ty, src, .. } => unary(st[src.index()], |a| numeric::neg(*ty, a).ok()),
        Inst::BitNot { ty, src, .. } => unary(st[src.index()], |a| Some(numeric::bit_not(*ty, a))),
        Inst::BoolNot { src, .. } => unary(st[src.index()], |a| Some((a == 0) as u64)),
        Inst::Cast { from, to, src, .. } => unary(st[src.index()], |a| Some(numeric::cast(*from, *to, a))),
        _ => Lat::Bottom,
    }
}

fn foldable(inst: &Inst) -> bool {
    matches!(
        inst,
        Inst::Move { .. }
            | Inst::Arith { .. }
            | Inst::Cmp { ty: CmpTy::Num(_), .. }
            | Inst::Neg { .. }
            | Inst::BitNot { .. }
            | Inst::BoolNot { .. }
            | Inst::Cast { .. }
    )
}

fn transfer(pinned: &[bool], inst: &Inst, st: &mut [Lat]) {
    if let Some(d) = inst.def() {
        st[d.index()] = if pinned[d.index()] { Lat::Bottom } else { value_of(inst, st) };
    }
}

/// The successors a terminator can take given the state at its end.
fn live_edges(term: &Term, st: &[Lat]) -> Vec<BlockId> {
    match term {
        Term::Branch { cond, then, els } => match st[cond.index()] {
            Lat::Const(c) => vec![if c != 0 { *then } else { *els }],
            Lat::Top => vec![],
            Lat::Bottom => vec![*then, *els],
        },
        other => other.successors(),
    }
}

pub fn sccp_func(f: &mut IrFunc) -> usize {
    let n = f.blocks.len();
    let regs = f.regs.len();
    if n == 0 || regs == 0 || n * regs > 2_000_000 {
        return 0;
    }
    let mut input: Vec<Option<Vec<Lat>>> = vec![None; n];
    let mut entry = vec![Lat::Top; regs];
    for (r, slot) in entry.iter_mut().enumerate() {
        if r < f.params as usize || f.pinned[r] {
            *slot = Lat::Bottom;
        }
    }
    input[0] = Some(entry);
    let mut work = vec![0usize];
    let mut rounds = 0usize;
    while let Some(b) = work.pop() {
        rounds += 1;
        if rounds > n * 8 + 64 {
            return 0; // not settling: leave the function alone
        }
        let Some(mut st) = input[b].clone() else { continue };
        for i in &f.blocks[b].insts {
            transfer(&f.pinned, i, &mut st);
        }
        for s in live_edges(&f.blocks[b].term, &st) {
            let s = s.index();
            let merged = match &input[s] {
                None => st.clone(),
                Some(old) => old.iter().zip(&st).map(|(a, b)| meet(*a, *b)).collect(),
            };
            if input[s].as_ref() != Some(&merged) {
                input[s] = Some(merged);
                work.push(s);
            }
        }
    }

    let mut changes = 0;
    for (b, entry) in input.iter().enumerate() {
        let Some(mut st) = entry.clone() else { continue };
        for inst in f.blocks[b].insts.iter_mut() {
            let was_const = matches!(inst, Inst::Const { .. });
            transfer(&f.pinned, inst, &mut st);
            if let (Some(d), false) = (inst.def(), was_const) {
                if foldable(inst) && !f.pinned[d.index()] {
                    if let Lat::Const(v) = st[d.index()] {
                        *inst = Inst::Const { dst: d, val: Const::Bits(v) };
                        changes += 1;
                    }
                }
            }
        }
        if let Term::Branch { cond, then, els } = f.blocks[b].term {
            if let Lat::Const(c) = st[cond.index()] {
                f.blocks[b].term = Term::Jump(if c != 0 { then } else { els });
                changes += 1;
            }
        }
    }
    changes
}
