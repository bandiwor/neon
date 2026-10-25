//! Stack maps: for every instruction at which the collector may run, the
//! registers that hold live heap references.
//!
//! A register is a root at a safe-point if it is *live* there (bytecode
//! liveness) and *holds a reference* (a forward dataflow over the `ref_result`
//! flag of the instruction that last wrote it).

use crate::bytecode::analysis::{build_cfg, liveness, transfer};
use crate::bytecode::{Insn, Item};
use crate::gc::BitSet;

/// For each safe-point item index, its root set.
pub fn compute(items: &[Item], pinned: &BitSet, param_refs: &[bool]) -> Vec<(usize, BitSet)> {
    let n = items.len();
    let live_out = liveness(items, pinned);
    let ref_in = reference_slots(items, param_refs);
    let mut maps = Vec::new();
    for i in 0..n {
        let insn = &items[i].insn;
        if !insn.is_safe_point() {
            continue;
        }
        let mut live = match insn {
            // while a callee runs, everything from the window up belongs to it:
            // only what survives the call (and is below the window) is ours
            Insn::Call { .. } | Insn::CallClosure { .. } => {
                let mut s = live_out[i];
                if let Some(d) = insn.def() {
                    s.remove(d);
                }
                if let Some(from) = insn.clobbers_from() {
                    s.remove_from(from);
                }
                s
            }
            _ => transfer(insn, &live_out[i], pinned),
        };
        live.intersect_with(&ref_in[i]);
        maps.push((i, live));
    }
    maps
}

/// Registers that may hold a reference just before each item runs.
fn reference_slots(items: &[Item], param_refs: &[bool]) -> Vec<BitSet> {
    let cfg = build_cfg(items);
    let n = items.len();
    let mut state_in = vec![BitSet::new(); n];
    for (i, &r) in param_refs.iter().enumerate() {
        if r {
            state_in[0].insert(i as u8);
        }
    }
    let mut work: Vec<usize> = (0..n).rev().collect();
    let mut queued = vec![true; n];
    while let Some(i) = work.pop() {
        queued[i] = false;
        let mut out = state_in[i];
        let it = &items[i];
        if let Some(from) = it.insn.clobbers_from() {
            out.remove_from(from);
        }
        if let Some(d) = it.insn.def() {
            if it.ref_result {
                out.insert(d);
            } else {
                out.remove(d);
            }
        }
        for &s in &cfg.succ[i] {
            if state_in[s].union_with(&out) && !queued[s] {
                queued[s] = true;
                work.push(s);
            }
        }
    }
    state_in
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::Label;
    use crate::runtime::numeric::{ArithOp, NumTy};

    fn it(insn: Insn, r: bool) -> Item {
        Item::with_ref(insn, r)
    }

    #[test]
    fn roots_are_live_references_only() {
        let items = vec![
            it(Insn::NewArray { dst: 1, base: 0, n: 0, desc: 0 }, true),
            it(Insn::LoadI { dst: 2, imm: 7 }, false),
            it(Insn::NewArray { dst: 3, base: 0, n: 0, desc: 0 }, true),
            // r1 (ref) and r2 (int) are live after; only r1 is a root
            it(Insn::Arith { op: ArithOp::Add, ty: NumTy::I64, dst: 4, a: 2, b: 2 }, false),
            it(Insn::Intrinsic { dst: 5, base: 1, nargs: 1, b: crate::builtin::Builtin::ArrLen, desc: None }, false),
            it(Insn::Ret { src: 4 }, false),
        ];
        let maps = compute(&items, &BitSet::new(), &[]);
        let find = |i: usize| maps.iter().find(|m| m.0 == i).map(|m| m.1.iter().collect::<Vec<_>>());
        assert_eq!(find(0), Some(vec![]));
        assert_eq!(find(2), Some(vec![1]), "r1 is live across the second allocation");
        assert_eq!(find(4), Some(vec![1]), "the intrinsic's own argument window is rooted");
        assert_eq!(find(1), None, "loadi is not a safe-point");
    }

    #[test]
    fn calls_root_only_what_survives_below_the_window() {
        let items = vec![
            it(Insn::NewArray { dst: 0, base: 0, n: 0, desc: 0 }, true), // survives the call
            it(Insn::NewArray { dst: 1, base: 0, n: 0, desc: 0 }, true), // becomes the argument
            it(Insn::Call { base: 1, func: 0, nargs: 1 }, true),
            it(
                Insn::Intrinsic { dst: 0, base: 0, nargs: 2, b: crate::builtin::Builtin::ArrContains, desc: None },
                false,
            ),
            it(Insn::Ret { src: 0 }, false),
        ];
        let maps = compute(&items, &BitSet::new(), &[]);
        let at_call = maps.iter().find(|m| m.0 == 2).unwrap();
        assert_eq!(at_call.1.iter().collect::<Vec<_>>(), [0], "the argument belongs to the callee's frame");
    }

    #[test]
    fn parameters_and_merges() {
        let (l, end) = (Label(0), Label(1));
        let items = vec![
            it(Insn::JmpIf { cond: 0, to: l }, false),
            it(Insn::LoadI { dst: 1, imm: 1 }, false),
            it(Insn::Jmp { to: end }, false),
            it(Insn::Label(l), false),
            it(Insn::NewArray { dst: 1, base: 0, n: 0, desc: 0 }, true),
            it(Insn::Label(end), false),
            it(Insn::Intrinsic { dst: 2, base: 0, nargs: 0, b: crate::builtin::Builtin::Input, desc: None }, true),
            it(Insn::Ret { src: 2 }, false),
        ];
        // param r0 is a reference; r1 is only a reference on one path and dead after the merge
        let maps = compute(&items, &BitSet::new(), &[true]);
        let at_input = maps.iter().find(|m| m.0 == 6).unwrap();
        assert!(at_input.1.is_empty(), "{:?}", at_input.1);
        let at_new = maps.iter().find(|m| m.0 == 4).unwrap();
        assert!(at_new.1.is_empty(), "r0 is dead there");
    }

    #[test]
    fn pinned_registers_stay_rooted() {
        let items = vec![
            it(Insn::NewArray { dst: 3, base: 0, n: 0, desc: 0 }, true),
            it(Insn::NewArray { dst: 4, base: 0, n: 0, desc: 0 }, true),
            it(Insn::Ret { src: 4 }, false),
        ];
        let pinned: BitSet = [3u8].into_iter().collect();
        let maps = compute(&items, &pinned, &[]);
        assert_eq!(maps[1].1.iter().collect::<Vec<_>>(), [3]);
    }
}
