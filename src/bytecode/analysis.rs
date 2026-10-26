//! Liveness analysis over instruction lists.

use crate::fx::FxHashMap as HashMap;

use super::insn::*;
use crate::gc::BitSet;

/// Successor lists of a straight-line instruction list with labels.
#[derive(Debug)]
pub struct Cfg {
    pub succ: Vec<Succ>,
    pub label_index: HashMap<Label, usize>,
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
    let mut label_index = HashMap::default();
    for (i, it) in items.iter().enumerate() {
        if let Insn::Label(l) = it.insn {
            label_index.insert(l, i);
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
                if let Some(&ti) = label_index.get(&t) {
                    s.idx[s.n as usize] = ti;
                    s.n += 1;
                }
            }
            s
        })
        .collect();
    Cfg { succ, label_index }
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
pub fn liveness(items: &[Item], always_live: &BitSet) -> Vec<BitSet> {
    let cfg = build_cfg(items);
    let n = items.len();
    let mut live_in = vec![BitSet::new(); n];
    let mut live_out = vec![BitSet::new(); n];
    let mut changed = true;
    while changed {
        changed = false;
        for i in (0..n).rev() {
            let mut out = *always_live;
            for &s in &cfg.succ[i] {
                out.union_with(&live_in[s]);
            }
            let inn = transfer(&items[i].insn, &out, always_live);
            if inn != live_in[i] || out != live_out[i] {
                live_in[i] = inn;
                live_out[i] = out;
                changed = true;
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
