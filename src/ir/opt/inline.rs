//! Function inlining.
//!
//! A call is replaced by a copy of the callee's blocks with registers and
//! block ids remapped into the caller. Parameters become moves from the
//! argument registers, and every `return` becomes a move into the call's
//! destination followed by a jump to the code after the call.

use super::OptConfig;
use crate::ir::types::*;

/// Whether `callee` may be copied into other functions.
fn inlinable(prog: &IrProgram, id: FuncId, cfg: &OptConfig) -> bool {
    let f = &prog.funcs[id.index()];
    if id == prog.entry || !f.upvals.is_empty() || f.pinned.iter().any(|&p| p) {
        return false;
    }
    if f.size() > cfg.inline_threshold {
        return false;
    }
    // direct recursion cannot be fully inlined
    !f.blocks.iter().flat_map(|b| &b.insts).any(|i| matches!(i, Inst::Call { func, .. } if *func == id))
}

/// One round of inlining over all call sites. Returns how many calls were inlined.
pub fn inline_round(prog: &mut IrProgram, cfg: &OptConfig) -> usize {
    let candidates: Vec<bool> = (0..prog.funcs.len()).map(|i| inlinable(prog, FuncId(i as u32), cfg)).collect();
    // callee bodies as they are at the start of the round
    let snapshot: Vec<Option<IrFunc>> =
        prog.funcs.iter().enumerate().map(|(i, f)| if candidates[i] { Some(f.clone()) } else { None }).collect();
    let mut total = 0;
    for fi in 0..prog.funcs.len() {
        total += inline_into(&mut prog.funcs[fi], FuncId(fi as u32), &snapshot, cfg);
    }
    total
}

fn inline_into(caller: &mut IrFunc, self_id: FuncId, callees: &[Option<IrFunc>], cfg: &OptConfig) -> usize {
    let mut count = 0;
    let mut bi = 0;
    while bi < caller.blocks.len() {
        let site = caller.blocks[bi].insts.iter().position(|i| {
            matches!(i, Inst::Call { func, .. } if *func != self_id && callees[func.index()].as_ref().is_some_and(|c| caller.regs.len() + c.regs.len() <= cfg.max_regs))
        });
        let Some(ii) = site else {
            bi += 1;
            continue;
        };
        let Inst::Call { dst, func, args } = caller.blocks[bi].insts[ii].clone() else { unreachable!() };
        let callee = callees[func.index()].as_ref().expect("checked above");
        splice(caller, bi, ii, dst, &args, callee);
        count += 1;
        // stay on this block: the code before the call may contain more sites
        // (the code after it moved to a new block that the loop reaches later)
    }
    count
}

fn splice(caller: &mut IrFunc, bi: usize, ii: usize, dst: VReg, args: &[VReg], callee: &IrFunc) {
    let reg_base = caller.regs.len() as u32;
    let block_base = caller.blocks.len() as u32;
    caller.regs.extend(callee.regs.iter().copied());
    caller.pinned.extend(std::iter::repeat_n(false, callee.regs.len()));
    let map_reg = |r: VReg| VReg(r.0 + reg_base);
    let map_block = |b: BlockId| BlockId(b.0 + block_base);

    // split the caller block around the call
    let tail_insts = caller.blocks[bi].insts.split_off(ii + 1);
    caller.blocks[bi].insts.pop(); // the call itself
    let tail_term = std::mem::replace(&mut caller.blocks[bi].term, Term::Unreachable);

    // parameters
    for (k, &a) in args.iter().enumerate() {
        caller.blocks[bi].insts.push(Inst::Move { dst: VReg(k as u32 + reg_base), src: a });
    }

    // callee blocks
    for cb in &callee.blocks {
        let mut insts = cb.insts.clone();
        for inst in insts.iter_mut() {
            inst.for_each_use_mut(|r| *r = map_reg(*r));
            if let Some(d) = inst.def_mut() {
                *d = map_reg(*d);
            }
        }
        caller.blocks.push(Block { insts, term: cb.term.clone() });
    }
    let cont = BlockId(caller.blocks.len() as u32);
    caller.blocks.push(Block { insts: tail_insts, term: tail_term });
    caller.blocks[bi].term = Term::Jump(map_block(BlockId(0)));

    for k in 0..callee.blocks.len() {
        let idx = block_base as usize + k;
        let term = caller.blocks[idx].term.clone();
        caller.blocks[idx].term = match term {
            Term::Jump(t) => Term::Jump(map_block(t)),
            Term::Branch { cond, then, els } => {
                Term::Branch { cond: map_reg(cond), then: map_block(then), els: map_block(els) }
            }
            Term::BranchTag { obj, tag, then, els } => {
                Term::BranchTag { obj: map_reg(obj), tag, then: map_block(then), els: map_block(els) }
            }
            Term::Return(r) => {
                caller.blocks[idx].insts.push(Inst::Move { dst, src: map_reg(r) });
                Term::Jump(cont)
            }
            Term::Unreachable => Term::Unreachable,
        };
    }
}
