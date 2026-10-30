//! Liveness analysis over instruction lists.

use super::insn::*;
use crate::gc::BitSet;

/// Successor lists of a straight-line instruction list with labels.
#[derive(Debug)]
pub struct Cfg {
    pub succ: Vec<Succ>,
}

/// At most two successors (fall-through and jump target), stored inline.
#[derive(Debug, Clone, Copy, Default)]
pub struct Succ {
    idx: [usize; 2],
    n: u8,
}

impl<'a> IntoIterator for &'a Succ {
    type Item = &'a usize;
    type IntoIter = std::slice::Iter<'a, usize>;

    fn into_iter(self) -> Self::IntoIter {
        self.idx[..self.n as usize].iter()
    }
}

pub fn build_cfg(items: &[Item]) -> Cfg {
    // labels are small numbers: a plain table finds them faster than a hash map
    let mut label_index: Vec<usize> = Vec::new();
    for (i, it) in items.iter().enumerate() {
        if let Insn::Label(l) = it.insn {
            let l = l.0 as usize;
            if label_index.len() <= l {
                label_index.resize(l + 1, usize::MAX);
            }
            label_index[l] = i;
        }
    }
    let succ = items
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let mut s = Succ::default();
            if it.insn.falls_through() && i + 1 < items.len() {
                s.idx[s.n as usize] = i + 1;
                s.n += 1;
            }
            if let Some(t) = it.insn.target() {
                if let Some(&ti) = label_index.get(t.0 as usize).filter(|&&ti| ti != usize::MAX) {
                    s.idx[s.n as usize] = ti;
                    s.n += 1;
                }
            }
            s
        })
        .collect();
    Cfg { succ }
}

/// The registers live *before* `insn` given those live after it.
/// `always_live` registers (captured by closures) are never killed.
pub fn transfer(insn: &Insn, live_out: &BitSet, always_live: &BitSet) -> BitSet {
    let mut s = *live_out;
    if let Some(from) = insn.clobbers_from() {
        s.remove_from(from);
    }
    if let Some(d) = insn.def() {
        s.remove(d);
    }
    insn.for_each_use(|r| s.insert(r));
    s.union_with(always_live);
    s
}

/// Live-out sets: `result[i]` holds the registers live after item `i` executes.
///
/// A backward worklist: every item is visited once in reverse order (which is all straight-line
/// code needs), and only items whose successors' sets changed are visited again.
pub fn liveness(items: &[Item], always_live: &BitSet) -> Vec<BitSet> {
    let cfg = build_cfg(items);
    let n = items.len();
    // predecessor lists in one flat array
    let mut start = vec![0u32; n + 1];
    for succ in &cfg.succ {
        for &s in succ {
            start[s + 1] += 1;
        }
    }
    for i in 0..n {
        start[i + 1] += start[i];
    }
    let mut fill = start.clone();
    let mut preds = vec![0u32; start[n] as usize];
    for (i, succ) in cfg.succ.iter().enumerate() {
        for &s in succ {
            preds[fill[s] as usize] = i as u32;
            fill[s] += 1;
        }
    }

    let mut live_in = vec![BitSet::new(); n];
    let mut live_out = vec![BitSet::new(); n];
    let mut queued = vec![true; n];
    let mut work: Vec<u32> = (0..n as u32).collect(); // popped from the end: last item first
    while let Some(i) = work.pop() {
        let i = i as usize;
        queued[i] = false;
        let mut out = *always_live;
        for &s in &cfg.succ[i] {
            out.union_with(&live_in[s]);
        }
        let inn = transfer(&items[i].insn, &out, always_live);
        live_out[i] = out;
        if inn != live_in[i] {
            live_in[i] = inn;
            for &p in &preds[start[i] as usize..start[i + 1] as usize] {
                if !queued[p as usize] {
                    queued[p as usize] = true;
                    work.push(p);
                }
            }
        }
    }
    live_out
}

/// Registers live before each item (the entry set of the function is `[0]`).
pub fn live_in_sets(items: &[Item], always_live: &BitSet) -> Vec<BitSet> {
    let out = liveness(items, always_live);
    items.iter().zip(&out).map(|(it, o)| transfer(&it.insn, o, always_live)).collect()
}

/// Answers "is register `r` still live after item `k`?" on the current instruction list, without
/// computing liveness for the whole function: a search forward from the item that stops at the first
/// read (live) or write (dead) on each path. Registers are reused soon after their last read, so the
/// search is short, and an edit needs no recomputation — only [`LiveQuery::invalidate`], because
/// positions and labels may have moved.
#[derive(Debug, Default)]
pub struct LiveQuery {
    label_index: Vec<usize>,
    labels_valid: bool,
    seen: Vec<u32>,
    stamp: u32,
    stack: Vec<usize>,
}

impl LiveQuery {
    pub fn new() -> LiveQuery {
        LiveQuery::default()
    }

    /// Call after the instruction list was edited.
    pub fn invalidate(&mut self) {
        self.labels_valid = false;
    }

    fn build_labels(&mut self, items: &[Item]) {
        self.label_index.clear();
        for (i, it) in items.iter().enumerate() {
            if let Insn::Label(l) = it.insn {
                let l = l.0 as usize;
                if self.label_index.len() <= l {
                    self.label_index.resize(l + 1, usize::MAX);
                }
                self.label_index[l] = i;
            }
        }
        self.labels_valid = true;
    }

    /// The same answer as `liveness(items, always_live)[k].contains(r)`.
    pub fn live_after(&mut self, items: &[Item], dead: &[bool], k: usize, r: u8, always_live: &BitSet) -> bool {
        if always_live.contains(r) {
            return true;
        }
        if !self.labels_valid {
            self.build_labels(items);
        }
        if self.seen.len() < items.len() {
            self.seen.resize(items.len(), 0);
        }
        self.stamp = self.stamp.wrapping_add(1);
        if self.stamp == 0 {
            self.seen.iter_mut().for_each(|s| *s = 0);
            self.stamp = 1;
        }
        self.stack.clear();
        self.push_successors(items, dead, k);
        while let Some(n) = self.stack.pop() {
            if self.seen[n] == self.stamp {
                continue;
            }
            self.seen[n] = self.stamp;
            if dead[n] {
                // a removed instruction is not there: control just moves on
                if n + 1 < items.len() {
                    self.stack.push(n + 1);
                }
                continue;
            }
            let insn = &items[n].insn;
            let mut used = false;
            insn.for_each_use(|u| used |= u == r);
            if used {
                return true;
            }
            // overwritten (or discarded by a call that reuses the registers above its base): dead on this path
            if insn.def() == Some(r) || insn.clobbers_from().is_some_and(|from| u16::from(r) >= from) {
                continue;
            }
            self.push_successors(items, dead, n);
        }
        false
    }

    fn push_successors(&mut self, items: &[Item], _dead: &[bool], i: usize) {
        let insn = &items[i].insn;
        if insn.falls_through() && i + 1 < items.len() {
            self.stack.push(i + 1);
        }
        if let Some(t) = insn.target() {
            if let Some(&ti) = self.label_index.get(t.0 as usize).filter(|&&ti| ti != usize::MAX) {
                self.stack.push(ti);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::numeric::{ArithOp, NumTy};

    /// A small deterministic generator of instruction lists with loops, branches and calls.
    fn random_items(seed: u64, len: usize) -> Vec<Item> {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut next = move |n: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % n
        };
        let labels = 4u32;
        let mut items = Vec::new();
        let mut placed = vec![false; labels as usize];
        for _ in 0..len {
            let r = |n: &mut dyn FnMut(u64) -> u64| n(6) as u8;
            let insn = match next(10) {
                0 => Insn::Move { dst: r(&mut next), src: r(&mut next) },
                1 => Insn::LoadI { dst: r(&mut next), imm: next(100) as i16 },
                2 | 3 => Insn::Arith {
                    op: ArithOp::Add,
                    ty: NumTy::I64,
                    dst: r(&mut next),
                    a: r(&mut next),
                    b: r(&mut next),
                },
                4 => Insn::Jmp { to: Label(next(labels as u64) as u32) },
                5 => Insn::JmpIf { cond: r(&mut next), to: Label(next(labels as u64) as u32) },
                6 => Insn::JmpIfNot { cond: r(&mut next), to: Label(next(labels as u64) as u32) },
                7 => Insn::Call { base: next(5) as u8, func: 0, nargs: next(3) as u8 },
                8 => Insn::Ret { src: r(&mut next) },
                _ => {
                    let l = next(labels as u64) as usize;
                    if placed[l] {
                        Insn::Move { dst: r(&mut next), src: r(&mut next) }
                    } else {
                        placed[l] = true;
                        Insn::Label(Label(l as u32))
                    }
                }
            };
            items.push(Item::new(insn));
        }
        // every jump needs its label
        for (l, p) in placed.iter().enumerate() {
            if !p {
                items.push(Item::new(Insn::Label(Label(l as u32))));
            }
        }
        items.push(Item::new(Insn::Ret { src: 0 }));
        items
    }

    #[test]
    fn the_demand_driven_query_agrees_with_the_full_analysis() {
        let mut always = BitSet::new();
        always.insert(5);
        for seed in 1..200u64 {
            let items = random_items(seed, 12 + (seed % 40) as usize);
            let full = liveness(&items, &always);
            let none = vec![false; items.len()];
            let mut q = LiveQuery::new();
            for (k, live) in full.iter().enumerate() {
                for r in 0..8u8 {
                    assert_eq!(
                        q.live_after(&items, &none, k, r, &always),
                        live.contains(r),
                        "seed {seed} item {k} r{r}"
                    );
                }
            }
            // with some plain instructions marked removed: the same as really removing them
            let dead: Vec<bool> = items
                .iter()
                .enumerate()
                .map(|(i, it)| {
                    matches!(it.insn, Insn::Move { .. } | Insn::LoadI { .. } | Insn::Arith { .. })
                        && (i + seed as usize).is_multiple_of(3)
                })
                .collect();
            let kept: Vec<Item> = items.iter().zip(&dead).filter(|(_, d)| !**d).map(|(it, _)| it.clone()).collect();
            let full = liveness(&kept, &always);
            let index_in_kept: Vec<usize> = {
                let mut k = 0;
                dead.iter()
                    .map(|d| {
                        let at = k;
                        if !d {
                            k += 1;
                        }
                        at
                    })
                    .collect()
            };
            let mut q = LiveQuery::new();
            for (k, d) in dead.iter().enumerate() {
                if *d {
                    continue;
                }
                for r in 0..8u8 {
                    assert_eq!(
                        q.live_after(&items, &dead, k, r, &always),
                        full[index_in_kept[k]].contains(r),
                        "seed {seed} (with removals) item {k} r{r}"
                    );
                }
            }
        }
    }
}
