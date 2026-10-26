//! Control-flow shaping passes.
//!
//! * [`self_tail`]: `return f(args)` inside `f` becomes "set the parameters, jump to the start", so
//!   the recursion runs in constant stack.
//! * [`dup_returns`]: a jump to a tiny block that only returns is the return.
//! * [`thread_known_branches`]: a jump to a block that only branches on a condition the jumping
//!   block has just set to a constant goes straight to the right target.
//! * [`merge_bool_diamonds`]: `if c { v = true } else { v = false }` is `v = c`.

use crate::ir::types::*;

/// Turns tail calls of a function to itself into jumps. Returns how many were rewritten.
pub fn self_tail(f: &mut IrFunc, me: FuncId) -> usize {
    // variables captured by closures need a fresh cell per call, so recursion cannot reuse the frame
    if f.is_closure || !f.upvals.is_empty() || f.pinned.iter().any(|&p| p) {
        return 0;
    }
    let nparams = f.params as usize;
    let is_site = move |b: &Block| -> bool {
        match (b.insts.last(), &b.term) {
            (Some(Inst::Call { dst, func, args }), Term::Return(r)) => *func == me && dst == r && args.len() == nparams,
            _ => false,
        }
    };
    if !f.blocks.iter().any(is_site) {
        return 0;
    }
    // the loop head: the old entry block moves, so that the entry itself has no predecessors
    let head = f.new_block();
    let entry = std::mem::replace(&mut f.blocks[0], Block { insts: Vec::new(), term: Term::Jump(head) });
    f.blocks[head.index()] = entry;

    let params = f.params as usize;
    let mut done = 0;
    for bi in 0..f.blocks.len() {
        if !is_site(&f.blocks[bi]) || bi == 0 {
            continue;
        }
        let Some(Inst::Call { args, .. }) = f.blocks[bi].insts.pop() else { unreachable!() };
        // arguments that are parameters themselves must be read before any parameter changes
        let clash = args.iter().any(|a| a.index() < params);
        let mut moves = Vec::new();
        let mut finals = Vec::new();
        for (i, &a) in args.iter().enumerate() {
            let p = VReg(i as u32);
            if a == p {
                continue;
            }
            if clash {
                let t = f.new_reg(f.reg_ty(p));
                moves.push(Inst::Move { dst: t, src: a });
                finals.push(Inst::Move { dst: p, src: t });
            } else {
                finals.push(Inst::Move { dst: p, src: a });
            }
        }
        f.blocks[bi].insts.extend(moves);
        f.blocks[bi].insts.extend(finals);
        f.blocks[bi].term = Term::Jump(head);
        done += 1;
    }
    done
}

/// Replaces `jump B` by `return r` when `B` is just (a couple of pure instructions and) that return.
pub fn dup_returns(f: &mut IrFunc) -> usize {
    let small: Vec<Option<(Vec<Inst>, VReg)>> = f
        .blocks
        .iter()
        .enumerate()
        .map(|(i, b)| match b.term {
            Term::Return(r)
                if i != 0
                    && b.insts.len() <= 2
                    && b.insts.iter().all(|x| matches!(x, Inst::Const { .. } | Inst::Move { .. })) =>
            {
                Some((b.insts.clone(), r))
            }
            _ => None,
        })
        .collect();
    let mut done = 0;
    for (bi, b) in f.blocks.iter_mut().enumerate() {
        if let Term::Jump(t) = b.term {
            if t.index() != bi {
                if let Some((insts, r)) = &small[t.index()] {
                    b.insts.extend(insts.iter().cloned());
                    b.term = Term::Return(*r);
                    done += 1;
                }
            }
        }
    }
    done
}

/// `p: ..; cond = const k; jump B` with `B: branch cond ? X : Y` and nothing else in `B`.
pub fn thread_known_branches(f: &mut IrFunc) -> usize {
    let mut done = 0;
    for pi in 0..f.blocks.len() {
        let Term::Jump(b) = f.blocks[pi].term else { continue };
        if b.index() == pi || !f.blocks[b.index()].insts.is_empty() {
            continue;
        }
        let Term::Branch { cond, then, els } = f.blocks[b.index()].term else { continue };
        if f.pinned[cond.index()] {
            continue;
        }
        let known = f.blocks[pi].insts.iter().rev().find(|i| i.def() == Some(cond)).and_then(|i| match i {
            Inst::Const { val: Const::Bits(v), .. } => Some(*v != 0),
            _ => None,
        });
        if let Some(taken) = known {
            f.blocks[pi].term = Term::Jump(if taken { then } else { els });
            done += 1;
        }
    }
    done
}

/// How a boolean arm ends.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Exit {
    Jump(BlockId),
    Return(VReg),
}

/// Whether `b` is exactly `[d = const bits]` followed by a jump or a return of `d`, with a single
/// predecessor.
fn bool_arm(f: &IrFunc, b: BlockId, preds: &[u32]) -> Option<(VReg, u64, Exit)> {
    let blk = &f.blocks[b.index()];
    match (&blk.insts[..], &blk.term) {
        ([Inst::Const { dst, val: Const::Bits(v) }], Term::Jump(j)) if preds[b.index()] == 1 && *v <= 1 => {
            Some((*dst, *v, Exit::Jump(*j)))
        }
        ([Inst::Const { dst, val: Const::Bits(v) }], Term::Return(r))
            if preds[b.index()] == 1 && *v <= 1 && r == dst =>
        {
            Some((*dst, *v, Exit::Return(*r)))
        }
        _ => None,
    }
}

/// `if c { v = true } else { v = false }` becomes `v = c` (or `v = !c`).
pub fn merge_bool_diamonds(f: &mut IrFunc) -> usize {
    let mut preds = vec![0u32; f.blocks.len()];
    for b in &f.blocks {
        for s in b.term.successors() {
            preds[s.index()] += 1;
        }
    }
    let mut done = 0;
    for pi in 0..f.blocks.len() {
        let Term::Branch { cond, then, els } = f.blocks[pi].term else { continue };
        if then == els || then.index() == pi || els.index() == pi {
            continue;
        }
        let (Some((d1, v1, j1)), Some((d2, v2, j2))) = (bool_arm(f, then, &preds), bool_arm(f, els, &preds)) else {
            continue;
        };
        if d1 != d2
            || j1 != j2
            || v1 == v2
            || f.reg_ty(d1) != IrTy::Bool
            || f.pinned[d1.index()]
            || f.reg_ty(cond) != IrTy::Bool
        {
            continue;
        }
        let set = if v1 == 1 { Inst::Move { dst: d1, src: cond } } else { Inst::BoolNot { dst: d1, src: cond } };
        // `d = cond` must not read `d` after writing it, and `cond` must still be intact: it is
        // read at the end of this block, exactly as the branch did
        f.blocks[pi].insts.push(set);
        f.blocks[pi].term = match j1 {
            Exit::Jump(j) => Term::Jump(j),
            Exit::Return(r) => Term::Return(r),
        };
        done += 1;
    }
    done
}
