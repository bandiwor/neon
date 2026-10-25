//! Constant folding and propagation, and copy propagation.
//!
//! A register with a single definition that is a constant is a known
//! constant everywhere (the definition dominates every use, because lowering
//! initializes variables where they are declared). Registers that closures
//! can modify ("pinned") are never treated as constant or copied.
//! Folding uses exactly the run-time arithmetic of `runtime::numeric`; an
//! operation that traps is left in place so the trap still happens.

use std::collections::HashMap;

use crate::ir::types::*;
use crate::runtime::numeric::{self, CmpOp};

/// Folds one function; returns the number of instructions/terminators rewritten.
pub fn fold_func(f: &mut IrFunc) -> usize {
    let mut total = 0;
    loop {
        let n = fold_once(f);
        total += n;
        if n == 0 {
            return total;
        }
    }
}

fn def_counts(f: &IrFunc) -> Vec<u32> {
    let mut counts = vec![0u32; f.regs.len()];
    for c in counts.iter_mut().take(f.params as usize) {
        *c += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            if let Some(d) = i.def() {
                counts[d.index()] += 1;
            }
        }
    }
    counts
}

fn fold_once(f: &mut IrFunc) -> usize {
    let counts = def_counts(f);

    // single-definition constants
    let mut consts: HashMap<VReg, u64> = HashMap::new();
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Const { dst, val: Const::Bits(v) } = i {
                if counts[dst.index()] == 1 && !f.pinned[dst.index()] {
                    consts.insert(*dst, *v);
                }
            }
        }
    }

    // single-definition copies of registers that never change
    let stable = |r: VReg| counts[r.index()] == 1 && !f.pinned[r.index()];
    let mut copies: HashMap<VReg, VReg> = HashMap::new();
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Move { dst, src } = i {
                if stable(*dst) && stable(*src) && dst != src {
                    copies.insert(*dst, *src);
                }
            }
        }
    }
    let resolve = |mut r: VReg| {
        let mut hops = 0;
        while let Some(&n) = copies.get(&r) {
            r = n;
            hops += 1;
            if hops > 64 {
                break;
            }
        }
        r
    };

    let mut changes = 0;
    for bi in 0..f.blocks.len() {
        // block-local knowledge about multiply-defined registers
        let mut local: HashMap<VReg, u64> = HashMap::new();
        let mut insts = std::mem::take(&mut f.blocks[bi].insts);
        for inst in insts.iter_mut() {
            // propagate copies into operands
            let mut renamed = false;
            inst.for_each_use_mut(|r| {
                let n = resolve(*r);
                if n != *r {
                    *r = n;
                    renamed = true;
                }
            });
            if renamed {
                changes += 1;
            }

            let known = |r: VReg| -> Option<u64> { consts.get(&r).or_else(|| local.get(&r)).copied() };
            let folded: Option<(VReg, u64)> = match inst {
                Inst::Arith { dst, op, ty, lhs, rhs } => match (known(*lhs), known(*rhs)) {
                    (Some(a), Some(b)) => numeric::arith(*op, *ty, a, b).ok().map(|v| (*dst, v)),
                    _ => None,
                },
                Inst::Cmp { dst, op, ty: CmpTy::Num(n), lhs, rhs } => match (known(*lhs), known(*rhs)) {
                    (Some(a), Some(b)) => Some((*dst, numeric::compare(*op, *n, a, b) as u64)),
                    _ => None,
                },
                Inst::Neg { dst, ty, src } => known(*src).and_then(|a| numeric::neg(*ty, a).ok()).map(|v| (*dst, v)),
                Inst::BitNot { dst, ty, src } => known(*src).map(|a| (*dst, numeric::bit_not(*ty, a))),
                Inst::BoolNot { dst, src } => known(*src).map(|a| (*dst, (a == 0) as u64)),
                Inst::Cast { dst, from, to, src } => known(*src).map(|a| (*dst, numeric::cast(*from, *to, a))),
                Inst::Move { dst, src } => known(*src).map(|a| (*dst, a)),
                _ => None,
            };
            if let Some((dst, v)) = folded {
                let already_const = matches!(inst, Inst::Const { .. });
                if !already_const {
                    *inst = Inst::Const { dst, val: Const::Bits(v) };
                    changes += 1;
                }
            }

            // update the block-local table
            if let Some(d) = inst.def() {
                match inst {
                    Inst::Const { val: Const::Bits(v), dst } if !f.pinned[dst.index()] => {
                        local.insert(*dst, *v);
                    }
                    _ => {
                        local.remove(&d);
                    }
                }
            }
            // anything a call or closure might change is not tracked across it
            if matches!(inst, Inst::Call { .. } | Inst::CallClosure { .. } | Inst::Intrinsic { .. }) {
                local.retain(|r, _| !f.pinned[r.index()]);
            }
        }
        f.blocks[bi].insts = insts;

        // terminator
        let known = |r: VReg| -> Option<u64> { consts.get(&r).or_else(|| local.get(&r)).copied() };
        let term = &mut f.blocks[bi].term;
        match term {
            Term::Branch { cond, then, els } => {
                let r = resolve(*cond);
                if r != *cond {
                    *cond = r;
                    changes += 1;
                }
                if let Some(c) = known(*cond) {
                    *term = Term::Jump(if c != 0 { *then } else { *els });
                    changes += 1;
                } else if then == els {
                    *term = Term::Jump(*then);
                    changes += 1;
                }
            }
            Term::BranchTag { obj, .. } => {
                let r = resolve(*obj);
                if r != *obj {
                    *obj = r;
                    changes += 1;
                }
            }
            Term::Return(r) => {
                let n = resolve(*r);
                if n != *r {
                    *r = n;
                    changes += 1;
                }
            }
            _ => {}
        }
    }
    let _ = CmpOp::Eq;
    changes
}
