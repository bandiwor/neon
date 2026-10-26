//! Loop rotation: `while c { body }` becomes `if c { do { body } while c }`.
//!
//! A `while` loop tests its condition in the header, so every iteration ends with a jump back to
//! the test. The header is copied behind the loop body instead: the back edge goes through the copy
//! and branches straight to the body again (one conditional jump per iteration, no unconditional one),
//! while the original header remains as the guard that decides whether the loop is entered at all.
//!
//! The guard has just run when the first iteration starts, so the part of the copied header that only
//! depends on values the loop never changes (`n - i`, a constant, `len` of an array nobody resizes...)
//! is already computed; those instructions are dropped from the copy. That moves even *trapping*
//! work out of the loop (`n - i` may overflow, but the guard already did that check on the same
//! operands), which the loop-invariant code motion cannot do for a loop that might not run.
//!
//! Only small headers are copied, and only when the back edge is an unconditional jump.

use super::licm::{dominators, find_loops, reverse_post_order};
use super::simplify::predecessors;
use crate::fx::FxHashSet as HashSet;
use crate::ir::effects::FuncEffects;
use crate::ir::types::*;

/// Headers with more instructions than this are not copied.
const MAX_HEADER: usize = 6;

pub fn rotate_loops(f: &mut IrFunc, fx: &FuncEffects) -> usize {
    let mut total = 0;
    for _ in 0..8 {
        let n = rotate_one(f, fx);
        if n == 0 {
            break;
        }
        total += n;
    }
    total
}

fn rotate_one(f: &mut IrFunc, fx: &FuncEffects) -> usize {
    let preds = predecessors(f);
    let rpo = reverse_post_order(f);
    let idom = dominators(f, &rpo, &preds);
    for lp in find_loops(f, &idom, &preds) {
        let h = lp.header;
        if h == 0 || f.blocks[h].insts.len() > MAX_HEADER {
            continue;
        }
        let Term::Branch { then, els, .. } = f.blocks[h].term else { continue };
        // exactly one side stays in the loop
        if lp.body.contains(&then.index()) == lp.body.contains(&els.index()) {
            continue;
        }
        let mut latches: Vec<usize> =
            lp.body.iter().copied().filter(|&b| matches!(f.blocks[b].term, Term::Jump(t) if t.index() == h)).collect();
        latches.sort_unstable();
        if latches.is_empty() || latches.len() > 2 {
            continue;
        }
        let outside = preds[h].iter().any(|p| !lp.body.contains(p) && idom[*p] != usize::MAX);
        if !outside {
            continue;
        }

        let copy_id = f.new_block();
        f.blocks[copy_id.index()].term = f.blocks[h].term.clone();
        let insts = drop_invariant(f, &lp.body, h, f.blocks[h].insts.clone(), fx);
        f.blocks[copy_id.index()].insts = insts;
        for l in latches {
            f.blocks[l].term = Term::Jump(copy_id);
        }
        return 1;
    }
    0
}

/// The instructions of the header copy that must run every iteration.
fn drop_invariant(f: &IrFunc, body: &HashSet<usize>, header: usize, insts: Vec<Inst>, fx: &FuncEffects) -> Vec<Inst> {
    // registers some other block of the loop assigns
    let mut variant: HashSet<VReg> = HashSet::default();
    for &b in body {
        if b != header {
            for i in &f.blocks[b].insts {
                if let Some(d) = i.def() {
                    variant.insert(d);
                }
            }
        }
    }
    let mut def_count: std::collections::HashMap<VReg, usize> = std::collections::HashMap::new();
    for i in &insts {
        if let Some(d) = i.def() {
            *def_count.entry(d).or_insert(0) += 1;
        }
    }
    // candidates: pure register computations assigning a register once
    let mut dropped = vec![false; insts.len()];
    for (k, i) in insts.iter().enumerate() {
        let pure = i.effects(fx).without(crate::effects::Effects::TRAPS) == crate::effects::Effects::NONE;
        dropped[k] = pure && i.def().is_some_and(|d| def_count[&d] == 1 && !f.pinned[d.index()]);
    }
    // whatever is not a candidate runs every iteration, so what it assigns changes
    for (k, i) in insts.iter().enumerate() {
        if let (false, Some(d)) = (dropped[k], i.def()) {
            variant.insert(d);
        }
    }
    loop {
        let mut changed = false;
        for (k, i) in insts.iter().enumerate() {
            if !dropped[k] {
                continue;
            }
            let d = i.def().expect("candidates define a register");
            let mut ok = !variant.contains(&d);
            i.for_each_use(|r| {
                // reads something the loop changes, or one the header assigns at this point or
                // later (the guard saw the old value, the copy would see the new one)
                let later = insts.iter().enumerate().any(|(j, o)| j >= k && o.def() == Some(r));
                if variant.contains(&r) || later || f.pinned[r.index()] {
                    ok = false;
                }
            });
            if !ok {
                dropped[k] = false;
                variant.insert(d);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    insts.into_iter().zip(dropped).filter(|(_, d)| !d).map(|(i, _)| i).collect()
}
