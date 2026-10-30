//! Global copy propagation: available copies across the whole control-flow graph.
//!
//! A *copy* `d = move s` is available at a point when on every path to it the move happened and
//! neither register has been written since. Reads of `d` there may read `s` instead. This is the
//! classic forward "must" analysis: the meet at a join is the intersection, loops start from the
//! optimistic "everything available" and shrink to the fixed point. It sees across blocks and around
//! loops, which the block-local forwarding of [`super::simplify`] cannot.

use crate::fx::FxHashMap as HashMap;
use crate::ir::types::*;

type Copies = HashMap<VReg, VReg>;

fn resolve(state: &Copies, mut r: VReg) -> VReg {
    // copies are kept flat (their sources are resolved when recorded), one hop is enough
    if let Some(&s) = state.get(&r) {
        r = s;
    }
    r
}

/// The effect of `inst` on the set of available copies.
fn step(f: &IrFunc, state: &mut Copies, inst: &Inst, record: bool, sources: &[bool]) {
    if let Some(d) = inst.def() {
        state.remove(&d);
        // only a register that is the source of some move can be the target of a copy
        if sources[d.index()] {
            state.retain(|_, v| *v != d);
        }
        if let Inst::Move { dst, src } = inst {
            let src = resolve(state, *src);
            if record
                && *dst != src
                && !f.pinned[dst.index()]
                && !f.pinned[src.index()]
                && f.reg_ty(*dst) == f.reg_ty(src)
            {
                state.insert(*dst, src);
            }
        }
    }
}

fn meet(maps: impl Iterator<Item = Copies>) -> Option<Copies> {
    let mut acc: Option<Copies> = None;
    for m in maps {
        acc = Some(match acc {
            None => m,
            Some(a) => a.into_iter().filter(|(k, v)| m.get(k) == Some(v)).collect(),
        });
    }
    acc
}

pub fn copy_propagate(f: &mut IrFunc) -> usize {
    let n = f.blocks.len();
    if n == 0 || !f.blocks.iter().flat_map(|b| &b.insts).any(|i| matches!(i, Inst::Move { .. })) {
        return 0;
    }
    let mut preds = vec![Vec::new(); n];
    for (i, b) in f.blocks.iter().enumerate() {
        for s in b.term.successors() {
            preds[s.index()].push(i);
        }
    }
    // `t = op ..; d = t` where `t` has no other use is an assignment `d = op ..` that the move
    // coalescing will merge; reading `t` in place of `d` later would keep `t` alive and prevent that
    let result_moves = super::simplify::result_moves(f);
    let mut sources = vec![false; f.regs.len()];
    for i in f.blocks.iter().flat_map(|b| &b.insts) {
        if let Inst::Move { src, .. } = i {
            sources[src.index()] = true;
        }
    }
    // reverse post-order
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

    // None = not reached yet ("everything is available")
    let mut out: Vec<Option<Copies>> = vec![None; n];
    let mut input: Vec<Copies> = vec![Copies::default(); n];
    let mut changed = true;
    let mut rounds = 0;
    while changed && rounds < 50 {
        changed = false;
        rounds += 1;
        for &b in &order {
            let state_in = if b == 0 {
                Copies::default()
            } else {
                match meet(preds[b].iter().filter_map(|&p| out[p].clone())) {
                    Some(m) => m,
                    None => continue,
                }
            };
            let mut state = state_in.clone();
            for (ii, i) in f.blocks[b].insts.iter().enumerate() {
                step(f, &mut state, i, !result_moves.contains(&(b, ii)), &sources);
            }
            input[b] = state_in;
            if out[b].as_ref() != Some(&state) {
                out[b] = Some(state);
                changed = true;
            }
        }
    }
    if changed {
        return 0; // did not settle: be safe
    }

    let mut renamed = 0;
    for &b in &order {
        if out[b].is_none() {
            continue;
        }
        let mut state = input[b].clone();
        let mut insts = std::mem::take(&mut f.blocks[b].insts);
        for (ii, inst) in insts.iter_mut().enumerate() {
            if !state.is_empty() {
                inst.for_each_use_mut(|r| {
                    let n = resolve(&state, *r);
                    if n != *r {
                        *r = n;
                        renamed += 1;
                    }
                });
            }
            step(f, &mut state, inst, !result_moves.contains(&(b, ii)), &sources);
        }
        f.blocks[b].insts = insts;
        if !state.is_empty() {
            match &mut f.blocks[b].term {
                Term::Branch { cond: r, .. } | Term::BranchTag { obj: r, .. } | Term::Return(r) => {
                    let n = resolve(&state, *r);
                    if n != *r {
                        *r = n;
                        renamed += 1;
                    }
                }
                _ => {}
            }
        }
    }
    renamed
}
