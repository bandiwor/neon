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
