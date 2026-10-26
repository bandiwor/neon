//! Merging of equal constants.
//!
//! Hoisting and inlining leave several registers holding the same constant (`1` three times in a
//! loop nest). Each one costs a frame slot and a load. When one definition dominates another with the
//! same value and type, uses of the second read the first and the second definition disappears
//! (dead-code elimination removes it).

use super::licm::{dominators, immediate_operands, reverse_post_order};
use super::simplify::{def_counts, predecessors};
use crate::fx::FxHashMap as HashMap;
use crate::ir::types::*;

/// Functions with more distinct repeated constants than this keep them as they are.
const MAX_SHARED: usize = 24;

pub fn dedupe_consts(f: &mut IrFunc) -> usize {
    if f.blocks.len() > 400 {
        return 0;
    }
    let defs = def_counts(f);
    // a small constant that only feeds additions becomes an immediate operand: leave it alone
    let immediates = immediate_operands(f);
    // every single-definition constant, in program order within a block
    type Group = Vec<(usize, usize, VReg)>;
    let mut groups: HashMap<(IrTy, Const), Group> = HashMap::default();
    for (b, blk) in f.blocks.iter().enumerate() {
        for (i, inst) in blk.insts.iter().enumerate() {
            if let Inst::Const { dst, val } = inst {
                if defs[dst.index()] == 1
                    && !f.pinned[dst.index()]
                    && !matches!(val, Const::Unit)
                    && !(immediates[dst.index()]
                        && matches!(val, Const::Bits(v) if (-255..=127).contains(&(*v as i64))))
                {
                    groups.entry((f.reg_ty(*dst), *val)).or_default().push((b, i, *dst));
                }
            }
        }
    }
    // a few constants shared by a whole function are cheap to keep in registers; hundreds (long
    // literals) would stay live for the whole function and use up the frame
    let repeated = groups.values().filter(|g| g.len() >= 2).count();
    if repeated == 0 || repeated > MAX_SHARED {
        return 0;
    }
    let preds = predecessors(f);
    let rpo = reverse_post_order(f);
    let idom = dominators(f, &rpo, &preds);
    let dominates = |a: usize, mut b: usize| loop {
        if a == b {
            return true;
        }
        if b == 0 || idom[b] == usize::MAX {
            return false;
        }
        b = idom[b];
    };
    let mut replace: HashMap<VReg, VReg> = HashMap::default();
    for group in groups.values() {
        for (k, &(bb, bi, b)) in group.iter().enumerate() {
            // an earlier definition of the same constant that dominates this one
            let rep = group[..k]
                .iter()
                .find(|&&(ab, ai, a)| !replace.contains_key(&a) && if ab == bb { ai < bi } else { dominates(ab, bb) });
            if let Some(&(_, _, a)) = rep {
                replace.insert(b, a);
            }
        }
    }
    if replace.is_empty() {
        return 0;
    }
    for blk in &mut f.blocks {
        for inst in &mut blk.insts {
            inst.for_each_use_mut(|r| {
                if let Some(&a) = replace.get(r) {
                    *r = a;
                }
            });
        }
        match &mut blk.term {
            Term::Branch { cond: r, .. } | Term::BranchTag { obj: r, .. } | Term::Return(r) => {
                if let Some(&a) = replace.get(r) {
                    *r = a;
                }
            }
            _ => {}
        }
    }
    replace.len()
}
