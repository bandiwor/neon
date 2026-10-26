//! Dead store elimination inside a block.
//!
//! A store is dead when a later store to the same place overwrites it and nothing can look at the
//! place in between. Scanning a block backwards, `shadowed` holds the places that will be
//! overwritten before any possible read:
//!
//! * a field `(object register, slot)`, an element `(array register, index register)` or a global;
//! * the set only survives instructions that cannot read memory or be seen from outside: register
//!   arithmetic (which may trap: nothing observes the heap once the program stops), constants, and
//!   other stores. Any read, call, allocation of a closure or output empties it.
//!
//! Redefining a register used in a key drops the keys that mention it. Two different registers may
//! hold the same object; that is fine because a store never reads, and a store through the other
//! register simply does not shadow this one (it is not in the set).

use crate::effects::Effects;
use crate::fx::FxHashMap;
use crate::ir::effects::FuncEffects;
use crate::ir::types::*;

/// An index as far as the block knows it: a literal or just a register.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Idx {
    Lit(u64),
    Reg(VReg),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    Field(VReg, u32),
    Elem(VReg, Idx),
    Global(GlobalSlot),
}

impl Place {
    fn mentions(&self, r: VReg) -> bool {
        match *self {
            Place::Field(o, _) => o == r,
            Place::Elem(a, i) => a == r || i == Idx::Reg(r),
            Place::Global(_) => false,
        }
    }
}

fn place_of(i: &Inst, consts: &FxHashMap<VReg, u64>) -> Option<Place> {
    match i {
        Inst::SetField { obj, idx, .. } => Some(Place::Field(*obj, *idx)),
        Inst::ArraySet { arr, idx, .. } => {
            Some(Place::Elem(*arr, consts.get(idx).map_or(Idx::Reg(*idx), |&v| Idx::Lit(v))))
        }
        Inst::SetGlobal { slot, .. } => Some(Place::Global(*slot)),
        _ => None,
    }
}

pub fn dead_stores(f: &mut IrFunc, fx: &FuncEffects) -> usize {
    let mut removed = 0;
    for b in 0..f.blocks.len() {
        // the place each store writes, with indices known to be constants in this block spelled out
        let mut consts: FxHashMap<VReg, u64> = FxHashMap::default();
        let mut places: Vec<Option<Place>> = Vec::with_capacity(f.blocks[b].insts.len());
        for inst in &f.blocks[b].insts {
            places.push(place_of(inst, &consts));
            match inst {
                Inst::Const { dst, val: Const::Bits(v) } => {
                    consts.insert(*dst, *v);
                }
                other => {
                    if let Some(d) = other.def() {
                        consts.remove(&d);
                    }
                }
            }
        }
        let mut shadowed: Vec<Place> = Vec::new();
        let mut keep = vec![true; f.blocks[b].insts.len()];
        for (k, inst) in f.blocks[b].insts.iter().enumerate().rev() {
            if let Some(p) = places[k] {
                if shadowed.contains(&p) {
                    keep[k] = false;
                    removed += 1;
                    continue;
                } else {
                    shadowed.push(p);
                }
                continue;
            }
            if !shadowed.is_empty() {
                let e = inst.effects(fx);
                if e.without(Effects::TRAPS) != Effects::NONE {
                    shadowed.clear();
                }
            }
            if let Some(d) = inst.def() {
                shadowed.retain(|p| !p.mentions(d));
            }
        }
        if keep.iter().any(|k| !k) {
            let mut it = keep.iter();
            f.blocks[b].insts.retain(|_| *it.next().expect("one flag per instruction"));
        }
    }
    removed
}
