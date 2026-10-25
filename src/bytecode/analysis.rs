//! Liveness analysis over instruction lists.

use std::collections::HashMap;

use super::insn::*;
use crate::gc::BitSet;

/// Successor lists of a straight-line instruction list with labels.
#[derive(Debug)]
pub struct Cfg {
    pub succ: Vec<Vec<usize>>,
    pub label_index: HashMap<Label, usize>,
}

pub fn build_cfg(items: &[Item]) -> Cfg {
    let mut label_index = HashMap::new();
    for (i, it) in items.iter().enumerate() {
        if let Insn::Label(l) = it.insn {
            label_index.insert(l, i);
        }
    }
    let succ = items
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let mut s = Vec::new();
            if it.insn.falls_through() && i + 1 < items.len() {
                s.push(i + 1);
            }
            if let Some(t) = it.insn.target() {
                if let Some(&ti) = label_index.get(&t) {
                    s.push(ti);
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
