//! Dead code elimination and control-flow cleanup.

use crate::fx::FxHashSet as HashSet;

use crate::ir::effects::FuncEffects;
use crate::ir::types::*;

/// Simplifies one function's CFG and removes dead instructions.
/// Returns `(instructions removed, blocks removed)`.
pub fn dce_func(f: &mut IrFunc, fx: &FuncEffects) -> (usize, usize) {
    let mut insts_removed = 0;
    let mut blocks_removed = 0;
    loop {
        let before = (insts_removed, blocks_removed);
        blocks_removed += remove_unreachable(f);
        super::tail::thread_known_branches(f);
        super::tail::merge_bool_diamonds(f);
        super::tail::dup_returns(f);
        thread_jumps(f);
        blocks_removed += remove_unreachable(f);
        blocks_removed += merge_blocks(f);
        insts_removed += coalesce_moves(f);
        insts_removed += remove_dead_insts(f, fx);
        if (insts_removed, blocks_removed) == before {
            return (insts_removed, blocks_removed);
        }
    }
}

fn reachable(f: &IrFunc) -> Vec<bool> {
    let mut seen = vec![false; f.blocks.len()];
    let mut stack = vec![0usize];
    while let Some(b) = stack.pop() {
        if seen[b] {
            continue;
        }
        seen[b] = true;
        let (slots, n) = f.blocks[b].term.successor_slots();
        for s in &slots[..n] {
            stack.push(s.index());
        }
    }
    seen
}

/// Drops blocks no path from the entry reaches, renumbering the rest.
fn remove_unreachable(f: &mut IrFunc) -> usize {
    let seen = reachable(f);
    if seen.iter().all(|&s| s) {
        return 0;
    }
    let mut remap = vec![u32::MAX; f.blocks.len()];
    let mut next = 0;
    for (i, &s) in seen.iter().enumerate() {
        if s {
            remap[i] = next;
            next += 1;
        }
    }
    let removed = f.blocks.len() - next as usize;
    let old = std::mem::take(&mut f.blocks);
    f.blocks = old
        .into_iter()
        .enumerate()
        .filter(|(i, _)| seen[*i])
        .map(|(_, mut b)| {
            for s in b.term.successors_mut() {
                s.0 = remap[s.index()];
            }
            b
        })
        .collect();
    removed
}

/// Redirects jumps to empty `jump`-only blocks straight to their target.
fn thread_jumps(f: &mut IrFunc) {
    let n = f.blocks.len();
    let forward: Vec<Option<BlockId>> = f
        .blocks
        .iter()
        .enumerate()
        .map(|(i, b)| match b.term {
            Term::Jump(t) if b.insts.is_empty() && t.index() != i => Some(t),
            _ => None,
        })
        .collect();
    let resolve = |mut b: BlockId| {
        let mut hops = 0;
        while let Some(t) = forward[b.index()] {
            b = t;
            hops += 1;
            if hops > n {
                break; // a cycle of empty blocks
            }
        }
        b
    };
    for b in &mut f.blocks {
        for s in b.term.successors_mut() {
            *s = resolve(*s);
        }
    }
}

/// Appends a block to its only predecessor when that predecessor jumps to it.
fn merge_blocks(f: &mut IrFunc) -> usize {
    let mut merged = 0;
    loop {
        let mut preds = vec![0u32; f.blocks.len()];
        for b in &f.blocks {
            for s in b.term.successors() {
                preds[s.index()] += 1;
            }
        }
        let mut did = false;
        for a in 0..f.blocks.len() {
            let Term::Jump(t) = f.blocks[a].term else { continue };
            let t = t.index();
            if t == a || t == 0 || preds[t] != 1 {
                continue;
            }
            let tail = std::mem::replace(&mut f.blocks[t], Block { insts: Vec::new(), term: Term::Unreachable });
            f.blocks[a].insts.extend(tail.insts);
            f.blocks[a].term = tail.term;
            merged += 1;
            did = true;
            break; // `t` is now an orphan; recompute
        }
        if !did {
            break;
        }
        remove_unreachable(f);
    }
    merged
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

fn def_counts(f: &IrFunc) -> Vec<u32> {
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

/// `t = op ..; r = t` with `t` used only by that move: write `r` directly.
fn coalesce_moves(f: &mut IrFunc) -> usize {
    let uses = use_counts(f);
    let defs = def_counts(f);
    let mut removed = 0;
    for bi in 0..f.blocks.len() {
        let mut i = 0;
        while i < f.blocks[bi].insts.len() {
            let Inst::Move { dst: r, src: t } = f.blocks[bi].insts[i] else {
                i += 1;
                continue;
            };
            if uses[t.index()] != 1 || defs[t.index()] != 1 || f.pinned[t.index()] || f.pinned[r.index()] || t == r {
                i += 1;
                continue;
            }
            // find t's definition earlier in this block
            let Some(j) = (0..i).rev().find(|&j| f.blocks[bi].insts[j].def() == Some(t)) else {
                i += 1;
                continue;
            };
            // r must be neither read nor written between the definition and the move
            let mut clash = false;
            for k in j + 1..i {
                let inst = &f.blocks[bi].insts[k];
                inst.for_each_use(|u| clash |= u == r);
                clash |= inst.def() == Some(r);
            }
            // the defining instruction must not read r after writing it (it reads before)
            if clash || f.reg_ty(r) != f.reg_ty(t) || matches!(f.blocks[bi].insts[j], Inst::MakeClosure { .. }) {
                i += 1;
                continue;
            }
            *f.blocks[bi].insts[j].def_mut().expect("has a def") = r;
            f.blocks[bi].insts.remove(i);
            removed += 1;
        }
    }
    removed
}

/// Removes side-effect-free instructions whose result is never read.
fn remove_dead_insts(f: &mut IrFunc, fx: &FuncEffects) -> usize {
    let mut removed = 0;
    loop {
        let uses = use_counts(f);
        let mut changed = false;
        for b in &mut f.blocks {
            let before = b.insts.len();
            b.insts.retain(|i| match i.def() {
                Some(d) => uses[d.index()] > 0 || f.pinned[d.index()] || !i.removable(fx),
                None => true,
            });
            // empty `CloseUpvals` are noise
            b.insts.retain(|i| !matches!(i, Inst::CloseUpvals { regs } if regs.is_empty()));
            if b.insts.len() != before {
                changed = true;
                removed += before - b.insts.len();
            }
        }
        if !changed {
            return removed;
        }
    }
}

/// Registers still referenced anywhere (used by tests).
#[allow(dead_code)]
pub fn referenced_regs(f: &IrFunc) -> HashSet<VReg> {
    let mut s = HashSet::default();
    for b in &f.blocks {
        for i in &b.insts {
            i.for_each_use(|r| {
                s.insert(r);
            });
            if let Some(d) = i.def() {
                s.insert(d);
            }
        }
        b.term.for_each_use(|r| {
            s.insert(r);
        });
    }
    s
}

/// Removes functions that nothing reachable from the entry calls or creates a
/// closure of, renumbering the rest. Returns how many were removed.
pub fn prune_functions(prog: &mut IrProgram) -> usize {
    let n = prog.funcs.len();
    let mut live = vec![false; n];
    let mut stack = vec![prog.entry.index()];
    while let Some(f) = stack.pop() {
        if live[f] {
            continue;
        }
        live[f] = true;
        for b in &prog.funcs[f].blocks {
            for i in &b.insts {
                match i {
                    Inst::Call { func, .. } | Inst::MakeClosure { func, .. } => stack.push(func.index()),
                    _ => {}
                }
            }
        }
    }
    let removed = live.iter().filter(|&&l| !l).count();
    if removed == 0 {
        return 0;
    }
    let mut remap = vec![0u32; n];
    let mut next = 0;
    for (i, &l) in live.iter().enumerate() {
        if l {
            remap[i] = next;
            next += 1;
        }
    }
    let old = std::mem::take(&mut prog.funcs);
    prog.funcs = old.into_iter().enumerate().filter(|(i, _)| live[*i]).map(|(_, f)| f).collect();
    for f in &mut prog.funcs {
        for b in &mut f.blocks {
            for i in &mut b.insts {
                match i {
                    Inst::Call { func, .. } | Inst::MakeClosure { func, .. } => func.0 = remap[func.index()],
                    _ => {}
                }
            }
        }
    }
    prog.entry = FuncId(remap[prog.entry.index()]);
    removed
}
