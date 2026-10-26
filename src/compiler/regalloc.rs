//! Block layout, virtual-register liveness, live intervals and frame-slot
//! assignment.
//!
//! The allocator is a linear scan over single-range intervals that always
//! picks the lowest free frame slot (so long-lived values settle at the
//! bottom of the frame, the way Lua keeps locals under temporaries). Calls
//! then place their argument window just above every slot that is live
//! across the call, which is what makes the sliding-window convention work.

use crate::ir::{BlockId, IrFunc, Term, VReg};

/// Marks a virtual register that never appears in the function body.
pub const NO_SLOT: u16 = u16::MAX;

/// A dense bit set over virtual registers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VSet {
    words: Vec<u64>,
}

impl VSet {
    pub fn new(n: usize) -> VSet {
        VSet { words: vec![0; n.div_ceil(64)] }
    }

    pub fn insert(&mut self, i: usize) {
        self.words[i >> 6] |= 1 << (i & 63);
    }

    pub fn remove(&mut self, i: usize) {
        self.words[i >> 6] &= !(1 << (i & 63));
    }

    pub fn contains(&self, i: usize) -> bool {
        self.words[i >> 6] & (1 << (i & 63)) != 0
    }

    /// Adds everything in `other`; reports whether the set grew.
    pub fn union_with(&mut self, other: &VSet) -> bool {
        let mut changed = false;
        for (a, b) in self.words.iter_mut().zip(&other.words) {
            let n = *a | *b;
            changed |= n != *a;
            *a = n;
        }
        changed
    }

    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.words
            .iter()
            .enumerate()
            .flat_map(|(wi, &w)| (0..64).filter(move |b| w & (1 << b) != 0).map(move |b| wi * 64 + b))
    }
}

/// Reachable blocks in emission order. Fall-through successors (the `then`
/// side of a branch, the target of a jump) are placed right after their
/// predecessor, so most jumps disappear. A branch never falls through into a block that
/// just returns when its other side continues the code: those exits go to the end, so the
/// loop that leaves through them keeps its hot path free of jumps.
pub fn block_layout(f: &IrFunc) -> Vec<BlockId> {
    let mut placed = vec![false; f.blocks.len()];
    let mut order = Vec::with_capacity(f.blocks.len());
    let mut pending = vec![BlockId(0)];
    let mut exits: Vec<BlockId> = Vec::new();
    let is_exit = |b: BlockId| {
        let blk = &f.blocks[b.index()];
        matches!(blk.term, Term::Return(_) | Term::Unreachable) && blk.insts.len() <= 2
    };
    while let Some(start) = pending.pop() {
        let mut cur = start;
        loop {
            if placed[cur.index()] {
                break;
            }
            placed[cur.index()] = true;
            order.push(cur);
            let succ = f.blocks[cur.index()].term.successors();
            let branching = succ.len() == 2 && succ[0] != succ[1];
            let mut next = None;
            for s in succ {
                if placed[s.index()] {
                    continue;
                }
                if branching && is_exit(s) {
                    exits.push(s);
                    continue;
                }
                if next.is_none() {
                    next = Some(s);
                } else {
                    pending.push(s);
                }
            }
            match next {
                Some(n) => cur = n,
                None => break,
            }
        }
    }
    // exits reached only from branches are laid out last (or already are)
    for e in exits {
        if !placed[e.index()] {
            placed[e.index()] = true;
            order.push(e);
        }
    }
    order
}

/// Live-in / live-out virtual registers of each block (indexed by block id).
#[derive(Debug)]
pub struct BlockLiveness {
    pub live_in: Vec<VSet>,
    pub live_out: Vec<VSet>,
}

pub fn block_liveness(f: &IrFunc, order: &[BlockId]) -> BlockLiveness {
    let nr = f.regs.len();
    let nb = f.blocks.len();
    let mut uses = vec![VSet::new(nr); nb];
    let mut defs = vec![VSet::new(nr); nb];
    for &b in order {
        let blk = &f.blocks[b.index()];
        let (u, d) = (&mut uses[b.index()], &mut defs[b.index()]);
        for inst in &blk.insts {
            inst.for_each_use(|r| {
                if !d.contains(r.index()) {
                    u.insert(r.index());
                }
            });
            if let Some(r) = inst.def() {
                d.insert(r.index());
            }
        }
        blk.term.for_each_use(|r| {
            if !d.contains(r.index()) {
                u.insert(r.index());
            }
        });
    }
    let mut live_in = vec![VSet::new(nr); nb];
    let mut live_out = vec![VSet::new(nr); nb];
    let mut changed = true;
    while changed {
        changed = false;
        for &b in order.iter().rev() {
            let bi = b.index();
            let mut out = VSet::new(nr);
            for s in f.blocks[bi].term.successors() {
                out.union_with(&live_in[s.index()]);
            }
            // in = uses ∪ (out − defs)
            let mut inn = uses[bi].clone();
            for r in out.iter() {
                if !defs[bi].contains(r) {
                    inn.insert(r);
                }
            }
            if out != live_out[bi] || inn != live_in[bi] {
                live_out[bi] = out;
                live_in[bi] = inn;
                changed = true;
            }
        }
    }
    BlockLiveness { live_in, live_out }
}

/// The result of allocation for one function.
#[derive(Debug)]
pub struct Allocation {
    pub order: Vec<BlockId>,
    /// Linear position of each block's first instruction and of its terminator.
    pub block_start: Vec<u32>,
    pub block_end: Vec<u32>,
    /// Frame slot of each virtual register ([`NO_SLOT`] if unused).
    pub slots: Vec<u16>,
    /// `[start, end]` positions of each virtual register's live interval.
    pub intervals: Vec<Option<(u32, u32)>>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct OutOfRegisters;

impl Allocation {
    /// One more than the highest slot of any value that is live across the
    /// instruction at `pos` (it must survive, so a callee frame or argument
    /// window has to start above it).
    pub fn floor_at(&self, pos: u32) -> u16 {
        let mut floor = 0;
        for (v, iv) in self.intervals.iter().enumerate() {
            if let Some((s, e)) = *iv {
                if s < pos && pos < e {
                    floor = floor.max(self.slots[v] + 1);
                }
            }
        }
        floor
    }
}

/// Maximum number of frame slots.
pub const MAX_SLOTS: u16 = 256;

pub fn allocate(f: &IrFunc) -> Result<Allocation, OutOfRegisters> {
    let order = block_layout(f);
    let live = block_liveness(f, &order);
    let nr = f.regs.len();

    // Number every instruction (terminators included) in layout order, two
    // apart: instruction k sits at 2k+2, the odd positions in between mean
    // "just before / just after", and 0 is before everything (so a parameter
    // is live across even the first instruction).
    let mut block_start = vec![0u32; f.blocks.len()];
    let mut block_end = vec![0u32; f.blocks.len()];
    let mut pos = 2u32;
    for &b in &order {
        block_start[b.index()] = pos;
        pos += 2 * f.blocks[b.index()].insts.len() as u32;
        block_end[b.index()] = pos;
        pos += 2;
    }
    let last_pos = pos - 1;

    let mut intervals: Vec<Option<(u32, u32)>> = vec![None; nr];
    fn touch(intervals: &mut [Option<(u32, u32)>], v: usize, p: u32) {
        intervals[v] = Some(match intervals[v] {
            None => (p, p),
            Some((s, e)) => (s.min(p), e.max(p)),
        });
    }
    for &b in &order {
        let bi = b.index();
        let blk = &f.blocks[bi];
        for r in live.live_in[bi].iter() {
            touch(&mut intervals, r, block_start[bi] - 1);
        }
        for r in live.live_out[bi].iter() {
            touch(&mut intervals, r, block_end[bi] + 1);
        }
        let mut p = block_start[bi];
        for inst in &blk.insts {
            inst.for_each_use(|r| touch(&mut intervals, r.index(), p));
            if let Some(d) = inst.def() {
                touch(&mut intervals, d.index(), p);
            }
            p += 2;
        }
        blk.term.for_each_use(|r| touch(&mut intervals, r.index(), p));
    }
    // parameters exist from the very start; captured registers keep their
    // slot for the whole function
    for v in 0..f.params as usize {
        touch(&mut intervals, v, 0);
    }
    for (v, &pinned) in f.pinned.iter().enumerate() {
        if pinned && intervals[v].is_some() {
            touch(&mut intervals, v, 0);
            touch(&mut intervals, v, last_pos);
        }
    }

    // linear scan: parameters take slots 0.., everything else lowest-free
    let mut slots = vec![NO_SLOT; nr];
    let mut busy_until: Vec<Option<u32>> = Vec::new(); // per slot: end of its current occupant
    let mut assign =
        |slots: &mut Vec<u16>, v: usize, iv: (u32, u32), want: Option<u16>| -> Result<(), OutOfRegisters> {
            let s = match want {
                Some(s) => s as usize,
                None => (0..).find(|&s| busy_until.get(s).copied().flatten().is_none_or(|e| e <= iv.0)).unwrap(),
            };
            if s >= MAX_SLOTS as usize {
                return Err(OutOfRegisters);
            }
            if busy_until.len() <= s {
                busy_until.resize(s + 1, None);
            }
            busy_until[s] = Some(iv.1);
            slots[v] = s as u16;
            Ok(())
        };
    for (v, iv) in intervals.iter().take(f.params as usize).enumerate() {
        assign(&mut slots, v, iv.unwrap(), Some(v as u16))?;
    }
    let mut rest: Vec<usize> = (f.params as usize..nr).filter(|&v| intervals[v].is_some()).collect();
    rest.sort_by_key(|&v| (intervals[v].unwrap().0, v));
    for v in rest {
        let iv = intervals[v].unwrap();
        assign(&mut slots, v, iv, None)?;
    }
    Ok(Allocation { order, block_start, block_end, slots, intervals })
}

impl Allocation {
    pub fn slot(&self, v: VReg) -> u8 {
        debug_assert!(self.slots[v.index()] != NO_SLOT, "v{} has no slot", v.0);
        self.slots[v.index()] as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Block, Inst, IrTy, Term};
    use crate::runtime::numeric::{ArithOp, NumTy};

    fn i64ty() -> IrTy {
        IrTy::Num(NumTy::I64)
    }

    fn add(dst: u32, a: u32, b: u32) -> Inst {
        Inst::Arith { dst: VReg(dst), op: ArithOp::Add, ty: NumTy::I64, lhs: VReg(a), rhs: VReg(b) }
    }

    fn func(params: u32, nregs: usize, blocks: Vec<Block>) -> IrFunc {
        let mut f = IrFunc::new("t");
        f.params = params;
        f.regs = vec![i64ty(); nregs];
        f.pinned = vec![false; nregs];
        f.blocks = blocks;
        f
    }

    #[test]
    fn vset_basics() {
        let mut s = VSet::new(130);
        for i in [0, 63, 64, 129] {
            s.insert(i);
        }
        assert!(s.contains(64) && !s.contains(65));
        s.remove(64);
        assert_eq!(s.iter().collect::<Vec<_>>(), [0, 63, 129]);
        let mut t = VSet::new(130);
        assert!(t.union_with(&s));
        assert!(!t.union_with(&s));
    }

    #[test]
    fn layout_puts_fallthrough_successors_next() {
        // b0: branch b1/b2 ; b1: jump b3 ; b2: jump b3 ; b3: ret ; b4 unreachable
        let mut f = func(1, 1, vec![]);
        for _ in 0..5 {
            f.new_block();
        }
        f.blocks[0].term = Term::Branch { cond: VReg(0), then: BlockId(1), els: BlockId(2) };
        f.blocks[1].term = Term::Jump(BlockId(3));
        f.blocks[2].term = Term::Jump(BlockId(3));
        f.blocks[3].term = Term::Return(VReg(0));
        assert_eq!(block_layout(&f), [BlockId(0), BlockId(1), BlockId(3), BlockId(2)]);
    }

    #[test]
    fn returning_blocks_go_last_when_the_other_side_continues() {
        // b0: jump b1 ; b1: branch b2 (ret) / b3 ; b3: jump b1 (a loop that leaves through b2)
        let mut f = func(1, 1, vec![]);
        for _ in 0..4 {
            f.new_block();
        }
        f.blocks[0].term = Term::Jump(BlockId(1));
        f.blocks[1].term = Term::Branch { cond: VReg(0), then: BlockId(2), els: BlockId(3) };
        f.blocks[2].term = Term::Return(VReg(0));
        f.blocks[3].term = Term::Jump(BlockId(1));
        assert_eq!(block_layout(&f), [BlockId(0), BlockId(1), BlockId(3), BlockId(2)]);
        // a plain jump to a returning block still falls into it
        f.blocks[0].term = Term::Jump(BlockId(2));
        assert_eq!(block_layout(&f), [BlockId(0), BlockId(2)]);
    }

    #[test]
    fn loop_carried_values_are_live_around_the_loop() {
        // v0 param (bound), v1 counter; b0: v1=v0+v0 ; b1: v2 = v1+v0 ; v1 = v2 ; branch b1/b2 ; b2: ret v1
        let mut f = func(1, 3, vec![]);
        for _ in 0..3 {
            f.new_block();
        }
        f.blocks[0].insts = vec![add(1, 0, 0)];
        f.blocks[0].term = Term::Jump(BlockId(1));
        f.blocks[1].insts = vec![add(2, 1, 0), Inst::Move { dst: VReg(1), src: VReg(2) }];
        f.blocks[1].term = Term::Branch { cond: VReg(1), then: BlockId(1), els: BlockId(2) };
        f.blocks[2].term = Term::Return(VReg(1));
        let order = block_layout(&f);
        let live = block_liveness(&f, &order);
        assert!(live.live_in[1].contains(0) && live.live_in[1].contains(1));
        assert!(live.live_out[1].contains(0) && live.live_out[1].contains(1));
        assert!(!live.live_out[1].contains(2));
        assert!(!live.live_in[0].contains(1));
    }

    #[test]
    fn parameters_keep_their_slots_and_temporaries_reuse_dead_ones() {
        // v0, v1 params; v2 = v0+v1; v3 = v2+v2; ret v3
        let mut f = func(2, 4, vec![Block { insts: vec![add(2, 0, 1), add(3, 2, 2)], term: Term::Return(VReg(3)) }]);
        f.pinned = vec![false; 4];
        let a = allocate(&f).unwrap();
        assert_eq!((a.slots[0], a.slots[1]), (0, 1));
        // v2 starts where v0/v1 die, v3 where v2 dies: all squeeze into the bottom slots
        assert!(a.slots.iter().all(|&s| s <= 2), "{:?}", a.slots);
    }

    #[test]
    fn overlapping_values_get_distinct_slots() {
        // v0 = const-ish param; v1 = v0+v0; v2 = v0+v1; v3 = v1+v2; ret v3
        let f = func(
            1,
            4,
            vec![Block { insts: vec![add(1, 0, 0), add(2, 0, 1), add(3, 1, 2)], term: Term::Return(VReg(3)) }],
        );
        let a = allocate(&f).unwrap();
        let (s1, s2) = (a.slots[1], a.slots[2]);
        assert_ne!(s1, s2);
        assert_ne!(a.slots[0], s1, "v0 is still live while v1 is defined");
    }

    #[test]
    fn floor_sits_above_values_live_across_a_position() {
        // v1 lives across position 1 (defined at 0, used at 2)
        let f = func(
            1,
            3,
            vec![Block { insts: vec![add(1, 0, 0), add(2, 0, 0), add(2, 1, 2)], term: Term::Return(VReg(2)) }],
        );
        let a = allocate(&f).unwrap();
        // instructions sit at positions 2, 4, 6
        assert!(a.floor_at(4) > a.slots[1]);
        assert!(a.floor_at(2) > a.slots[0], "a parameter is live across the very first instruction");
        assert_eq!(a.floor_at(2), a.slots[0] + 1);
    }

    #[test]
    fn pinned_registers_live_for_the_whole_function() {
        let mut f = func(
            0,
            3,
            vec![Block { insts: vec![add(0, 0, 0), add(1, 1, 1), add(2, 2, 2)], term: Term::Return(VReg(1)) }],
        );
        f.pinned[0] = true;
        let a = allocate(&f).unwrap();
        assert_eq!(a.intervals[0], Some((0, 9)), "from before the first instruction to after the terminator");
        assert_ne!(a.slots[0], a.slots[1]);
        assert_ne!(a.slots[0], a.slots[2]);
    }

    #[test]
    fn too_many_simultaneously_live_values_is_an_error() {
        let n = 300usize;
        // define 300 values, then sum them all so they are live together
        let mut insts: Vec<Inst> = (1..=n).map(|i| add(i as u32, 0, 0)).collect();
        for i in 2..=n {
            insts.push(add(1, 1, i as u32));
        }
        let f = func(1, n + 1, vec![Block { insts, term: Term::Return(VReg(1)) }]);
        assert_eq!(allocate(&f).unwrap_err(), OutOfRegisters);
    }
}
