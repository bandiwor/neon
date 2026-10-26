//! Loop-invariant code motion.
//!
//! An instruction in a loop moves in front of the loop (into its preheader) when
//!
//! * it can neither trap nor have side effects, so executing it when the loop would not have
//!   reached it (or run at all) changes nothing: constants, comparisons, float and bitwise
//!   arithmetic, casts, enum tags, pure builtins, and heap reads (`len`, field loads) in loops
//!   that never write to the heap or call out;
//! * every operand is invariant: not defined in the loop, or defined by an instruction that moves
//!   too; and
//! * its result register has no other definition in the function (so no use can observe a
//!   different value).
//!
//! Loops are natural loops of back edges (the target dominates the source). A preheader is created
//! when the header has several outside predecessors. At most [`MAX_PER_LOOP`] instructions move out
//! of one loop and nothing moves in functions that already use many registers: hoisted values live
//! through the whole loop and every register costs a frame slot.

use crate::fx::FxHashSet as HashSet;

use super::simplify::{clobbers_memory, def_counts, predecessors};
use crate::ir::effects::FuncEffects;
use crate::ir::types::*;

const MAX_PER_LOOP: usize = 12;
/// Functions with more registers than this are left alone.
const MAX_REGS: usize = 180;

pub fn licm_func(f: &mut IrFunc, fx: &FuncEffects) -> usize {
    if f.regs.len() > MAX_REGS {
        return 0;
    }
    let mut moved = 0;
    // each transformation may add a block, so analyses are recomputed
    for _ in 0..16 {
        let n = hoist_one_loop(f, fx);
        if n == 0 {
            break;
        }
        moved += n;
        if f.regs.len() > MAX_REGS {
            break;
        }
    }
    moved
}

/// Blocks in reverse post-order from the entry (unreachable blocks are absent).
pub(super) fn reverse_post_order(f: &IrFunc) -> Vec<usize> {
    let n = f.blocks.len();
    let mut seen = vec![false; n];
    let mut order = Vec::with_capacity(n);
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
    order
}

/// Immediate dominators (Cooper, Harvey, Kennedy); `usize::MAX` for unreachable blocks.
pub(super) fn dominators(f: &IrFunc, rpo: &[usize], preds: &[Vec<usize>]) -> Vec<usize> {
    let n = f.blocks.len();
    let mut order = vec![usize::MAX; n];
    for (i, &b) in rpo.iter().enumerate() {
        order[b] = i;
    }
    let mut idom = vec![usize::MAX; n];
    idom[0] = 0;
    let mut changed = true;
    while changed {
        changed = false;
        for &b in rpo.iter().skip(1) {
            let mut new = usize::MAX;
            for &p in &preds[b] {
                if idom[p] == usize::MAX {
                    continue;
                }
                new = if new == usize::MAX {
                    p
                } else {
                    let (mut x, mut y) = (p, new);
                    while x != y {
                        while order[x] > order[y] {
                            x = idom[x];
                        }
                        while order[y] > order[x] {
                            y = idom[y];
                        }
                    }
                    x
                };
            }
            if new != idom[b] {
                idom[b] = new;
                changed = true;
            }
        }
    }
    idom
}

fn dominates(idom: &[usize], a: usize, mut b: usize) -> bool {
    loop {
        if a == b {
            return true;
        }
        if b == 0 || idom[b] == usize::MAX {
            return false;
        }
        b = idom[b];
    }
}

pub(super) struct Loop {
    pub header: usize,
    pub body: HashSet<usize>,
}

pub(super) fn find_loops(f: &IrFunc, idom: &[usize], preds: &[Vec<usize>]) -> Vec<Loop> {
    let mut loops: Vec<Loop> = Vec::new();
    for (l, b) in f.blocks.iter().enumerate() {
        if idom[l] == usize::MAX {
            continue;
        }
        for s in b.term.successors() {
            let h = s.index();
            if !dominates(idom, h, l) {
                continue;
            }
            // natural loop of the back edge l -> h
            let mut body: HashSet<usize> = HashSet::default();
            body.insert(h);
            let mut stack = vec![l];
            while let Some(x) = stack.pop() {
                if body.insert(x) {
                    stack.extend(preds[x].iter().copied().filter(|&p| idom[p] != usize::MAX));
                }
            }
            match loops.iter_mut().find(|lp| lp.header == h) {
                Some(lp) => lp.body.extend(body),
                None => loops.push(Loop { header: h, body }),
            }
        }
    }
    // innermost first
    loops.sort_by_key(|l| l.body.len());
    loops
}

/// Whether the instruction can move out of a loop (given its operands are invariant): by its
/// effects it must be hoistable (no writes, I/O, traps, endless loops, hidden preconditions and no
/// fresh object per execution), and if it reads memory, nothing in the loop may write any.
fn movable_kind(i: &Inst, loop_writes_heap: bool, fx: &FuncEffects) -> bool {
    let e = i.effects(fx);
    if !e.hoistable() || (e.reads() && loop_writes_heap) {
        return false;
    }
    // variables captured by closures change behind the back of the loop; global reads stay put too
    !matches!(i, Inst::GetUpval { .. } | Inst::GetGlobal { .. })
}

/// Registers that are only ever an operand of an integer `+` or `-`: when they hold a small
/// constant the bytecode folds them into an immediate (`addi`), so moving them out of a loop would
/// only cost a register.
fn immediate_operands(f: &IrFunc) -> Vec<bool> {
    use crate::runtime::numeric::ArithOp;
    let mut only = vec![true; f.regs.len()];
    for b in &f.blocks {
        for i in &b.insts {
            match i {
                Inst::Arith { op: ArithOp::Add | ArithOp::Sub, ty, .. } if ty.is_int() => {}
                _ => i.for_each_use(|r| only[r.index()] = false),
            }
        }
        b.term.for_each_use(|r| only[r.index()] = false);
    }
    only
}

fn small_constant(i: &Inst) -> bool {
    matches!(i, Inst::Const { val: Const::Bits(v), .. } if (-128..=127).contains(&(*v as i64)))
}

fn hoist_one_loop(f: &mut IrFunc, fx: &FuncEffects) -> usize {
    let immediates = immediate_operands(f);
    let preds = predecessors(f);
    let rpo = reverse_post_order(f);
    let idom = dominators(f, &rpo, &preds);
    let defs = def_counts(f);

    for lp in find_loops(f, &idom, &preds) {
        if lp.header == 0 {
            continue;
        }
        let outside: Vec<usize> =
            preds[lp.header].iter().copied().filter(|p| idom[*p] != usize::MAX && !lp.body.contains(p)).collect();
        if outside.is_empty() {
            continue;
        }
        let mut body: Vec<usize> = lp.body.iter().copied().collect();
        body.sort_unstable();

        let loop_writes_heap = body.iter().any(|&b| f.blocks[b].insts.iter().any(|i| clobbers_memory(i, fx)));
        // registers defined in the loop
        let mut defined_in_loop = vec![false; f.regs.len()];
        for &b in &body {
            for i in &f.blocks[b].insts {
                if let Some(d) = i.def() {
                    defined_in_loop[d.index()] = true;
                }
            }
        }

        let mut hoisted: Vec<(usize, usize)> = Vec::new();
        let mut moves_out = vec![false; f.regs.len()];
        'grow: loop {
            let mut grew = false;
            for &b in &body {
                for (ii, i) in f.blocks[b].insts.iter().enumerate() {
                    if hoisted.len() >= MAX_PER_LOOP {
                        break 'grow;
                    }
                    let Some(d) = i.def() else { continue };
                    if hoisted.contains(&(b, ii))
                        || defs[d.index()] != 1
                        || f.pinned[d.index()]
                        || !movable_kind(i, loop_writes_heap, fx)
                        || (small_constant(i) && immediates[d.index()])
                    {
                        continue;
                    }
                    let mut invariant = true;
                    i.for_each_use(|r| {
                        if f.pinned[r.index()] || (defined_in_loop[r.index()] && !moves_out[r.index()]) {
                            invariant = false;
                        }
                    });
                    if invariant {
                        hoisted.push((b, ii));
                        moves_out[d.index()] = true;
                        grew = true;
                    }
                }
            }
            if !grew {
                break;
            }
        }
        if hoisted.is_empty() {
            continue;
        }

        // the preheader
        let pre = match outside.as_slice() {
            [p] if matches!(f.blocks[*p].term, Term::Jump(t) if t.index() == lp.header) => *p,
            _ => {
                let pre = f.new_block();
                f.blocks[pre.index()].term = Term::Jump(BlockId(lp.header as u32));
                for &p in &outside {
                    for s in f.blocks[p].term.successors_mut() {
                        if s.index() == lp.header {
                            *s = pre;
                        }
                    }
                }
                pre.index()
            }
        };

        // take the instructions out (highest index first so earlier positions stay valid)
        let mut taken: Vec<((usize, usize), Inst)> = Vec::new();
        let mut order = hoisted.clone();
        order.sort_unstable_by(|a, b| b.cmp(a));
        for (b, ii) in order {
            taken.push(((b, ii), f.blocks[b].insts.remove(ii)));
        }
        // back in discovery order, which respects dependencies
        for pos in &hoisted {
            let at = taken.iter().position(|(p, _)| p == pos).expect("taken");
            let (_, inst) = taken.remove(at);
            f.blocks[pre].insts.push(inst);
        }
        return hoisted.len();
    }
    0
}
