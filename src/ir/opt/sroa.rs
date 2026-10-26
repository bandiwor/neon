//! Scalar replacement of aggregates and removal of dead private objects, both driven by the escape
//! analysis ([`crate::ir::escape`]).
//!
//! * A struct or enum that is *private* (fresh, never escaping, never returned), allocated at one
//!   place and used only through field reads and writes (and, for enums, tag checks) does not need
//!   to exist: each field becomes a register of its own. `s.x` is a move, `s.x = v` is a move, the
//!   variant of an enum is known statically so `tag(e)` is a constant and a `BranchTag` on it a
//!   plain jump.
//! * An array, map, set or struct that is private and only ever *written* (pushed to, inserted
//!   into, assigned a field) is dead together with all the writes.
//!
//! Both leave the unused field registers and the argument computations to the next DCE round.

use std::collections::HashMap as StdMap;

use crate::builtin::Builtin;
use crate::fx::FxHashMap as HashMap;
use crate::ir::effects::FuncEffects;
use crate::ir::escape::classify;
use crate::ir::types::*;

use super::simplify::def_counts;

#[derive(Clone)]
struct Candidate {
    /// `Some(tag)` for an enum, `None` for a struct.
    tag: Option<u32>,
    fields: Vec<VReg>,
    /// The new registers, one per field.
    fr: Vec<VReg>,
}

fn occurrences(inst: &Inst, r: VReg) -> usize {
    let mut n = 0;
    inst.for_each_use(|u| n += (u == r) as usize);
    n
}

/// Replaces private structs and enums by their fields. Returns the number of objects removed.
pub fn sroa_func(f: &mut IrFunc, fx: &FuncEffects) -> usize {
    let classes = classify(f, fx.escapes());
    let defs = def_counts(f);
    let mut cands: HashMap<VReg, Candidate> = HashMap::default();
    for b in &f.blocks {
        for i in &b.insts {
            let (dst, tag, fields) = match i {
                Inst::NewStruct { dst, fields, .. } => (*dst, None, fields),
                Inst::NewEnum { dst, tag, fields, .. } => (*dst, Some(*tag), fields),
                _ => continue,
            };
            if defs[dst.index()] == 1
                && !f.pinned[dst.index()]
                && dst.index() >= f.params as usize
                && classes.is_private(dst)
                && classes.is_alone(dst)
            {
                cands.insert(dst, Candidate { tag, fields: fields.clone(), fr: Vec::new() });
            }
        }
    }
    if cands.is_empty() {
        return 0;
    }

    // every use must be a field access (or a tag check) of a matching object
    for b in &f.blocks {
        for i in &b.insts {
            let mut used: Vec<VReg> = Vec::new();
            i.for_each_use(|u| {
                if cands.contains_key(&u) {
                    used.push(u);
                }
            });
            for r in used {
                let c = &cands[&r];
                let ok = occurrences(i, r) == 1
                    && match i {
                        Inst::GetField { obj, idx, .. } | Inst::SetField { obj, idx, .. } => {
                            *obj == r && c.tag.is_none() && (*idx as usize) < c.fields.len()
                        }
                        Inst::EnumTag { obj, .. } => *obj == r && c.tag.is_some(),
                        Inst::EnumField { obj, idx, .. } => {
                            *obj == r && c.tag.is_some() && (*idx as usize) < c.fields.len()
                        }
                        _ => false,
                    };
                if !ok {
                    cands.remove(&r);
                }
            }
        }
        let mut used = Vec::new();
        b.term.for_each_use(|u| {
            if cands.contains_key(&u) {
                used.push(u);
            }
        });
        for r in used {
            if !(matches!(b.term, Term::BranchTag { obj, .. } if obj == r) && cands[&r].tag.is_some()) {
                cands.remove(&r);
            }
        }
    }
    if cands.is_empty() {
        return 0;
    }

    let keys: Vec<VReg> = cands.keys().copied().collect();
    for k in &keys {
        let types: Vec<IrTy> = cands[k].fields.iter().map(|&r| f.reg_ty(r)).collect();
        let fr: Vec<VReg> = types.into_iter().map(|t| f.new_reg(t)).collect();
        cands.get_mut(k).expect("candidate").fr = fr;
    }

    for bi in 0..f.blocks.len() {
        let old = std::mem::take(&mut f.blocks[bi].insts);
        let mut out = Vec::with_capacity(old.len());
        for inst in old {
            match inst {
                Inst::NewStruct { dst, .. } | Inst::NewEnum { dst, .. } if cands.contains_key(&dst) => {
                    let c = &cands[&dst];
                    for (fr, src) in c.fr.iter().zip(&c.fields) {
                        out.push(Inst::Move { dst: *fr, src: *src });
                    }
                }
                Inst::GetField { dst, obj, idx } if cands.contains_key(&obj) => {
                    out.push(Inst::Move { dst, src: cands[&obj].fr[idx as usize] });
                }
                Inst::SetField { obj, idx, src } if cands.contains_key(&obj) => {
                    out.push(Inst::Move { dst: cands[&obj].fr[idx as usize], src });
                }
                Inst::EnumTag { dst, obj } if cands.contains_key(&obj) => {
                    let tag = cands[&obj].tag.expect("enum") as u64;
                    out.push(Inst::Const { dst, val: Const::Bits(tag) });
                }
                Inst::EnumField { dst, obj, idx } if cands.contains_key(&obj) => {
                    out.push(Inst::Move { dst, src: cands[&obj].fr[idx as usize] });
                }
                other => out.push(other),
            }
        }
        f.blocks[bi].insts = out;
        if let Term::BranchTag { obj, tag, then, els } = f.blocks[bi].term {
            if let Some(c) = cands.get(&obj) {
                f.blocks[bi].term = Term::Jump(if c.tag == Some(tag) { then } else { els });
            }
        }
    }
    cands.len()
}

/// Builtins that only put things into a container they are given, never trap, and whose result
/// (unit or a flag) nobody has to look at.
fn write_only(b: Builtin) -> bool {
    use Builtin::*;
    matches!(b, ArrPush | ArrClear | ArrReverse | MapInsert | MapClear | MapRemove | SetInsert | SetRemove | SetClear)
}

/// Removes private objects that are only written to, together with the writes.
pub fn dead_objects(f: &mut IrFunc, fx: &FuncEffects) -> usize {
    let classes = classify(f, fx.escapes());
    let defs = def_counts(f);
    let mut uses = vec![0u32; f.regs.len()];
    for b in &f.blocks {
        for i in &b.insts {
            i.for_each_use(|r| uses[r.index()] += 1);
        }
        b.term.for_each_use(|r| uses[r.index()] += 1);
    }

    let mut dead: StdMap<VReg, ()> = StdMap::new();
    for b in &f.blocks {
        for i in &b.insts {
            let dst = match i {
                Inst::NewStruct { dst, .. }
                | Inst::NewArray { dst, .. }
                | Inst::NewMap { dst, .. }
                | Inst::NewSet { dst, .. } => *dst,
                _ => continue,
            };
            if defs[dst.index()] == 1
                && !f.pinned[dst.index()]
                && dst.index() >= f.params as usize
                && classes.is_private(dst)
                && classes.is_alone(dst)
            {
                dead.insert(dst, ());
            }
        }
    }
    if dead.is_empty() {
        return 0;
    }
    // every use must be a write that can be dropped; count them to compare with all the uses
    let mut writes: StdMap<VReg, u32> = StdMap::new();
    let mut doomed: Vec<(usize, usize)> = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for (ii, i) in b.insts.iter().enumerate() {
            let target = match i {
                Inst::SetField { obj, .. } if dead.contains_key(obj) && occurrences(i, *obj) == 1 => Some(*obj),
                Inst::Intrinsic { dst, b: bt, args, .. }
                    if write_only(*bt)
                        && uses[dst.index()] == 0
                        && !f.pinned[dst.index()]
                        && args.first().is_some_and(|a| dead.contains_key(a))
                        && occurrences(i, args[0]) == 1 =>
                {
                    Some(args[0])
                }
                _ => None,
            };
            if let Some(t) = target {
                *writes.entry(t).or_insert(0) += 1;
                doomed.push((bi, ii));
            }
        }
    }
    // an object is dead when its uses are exactly those writes
    let truly_dead: Vec<VReg> =
        dead.keys().copied().filter(|d| uses[d.index()] == writes.get(d).copied().unwrap_or(0)).collect();
    if truly_dead.is_empty() {
        return 0;
    }
    let mut remove: Vec<(usize, usize)> = doomed
        .into_iter()
        .filter(|&(bi, ii)| {
            let target = match &f.blocks[bi].insts[ii] {
                Inst::SetField { obj, .. } => *obj,
                Inst::Intrinsic { args, .. } => args[0],
                _ => unreachable!(),
            };
            truly_dead.contains(&target)
        })
        .collect();
    // and the allocations themselves (their operands become unused for the next DCE round)
    for (bi, b) in f.blocks.iter().enumerate() {
        for (ii, i) in b.insts.iter().enumerate() {
            if let Some(d) = i.def() {
                if truly_dead.contains(&d)
                    && matches!(
                        i,
                        Inst::NewStruct { .. } | Inst::NewArray { .. } | Inst::NewMap { .. } | Inst::NewSet { .. }
                    )
                {
                    remove.push((bi, ii));
                }
            }
        }
    }
    remove.sort_unstable_by(|a, b| b.cmp(a));
    for (bi, ii) in &remove {
        f.blocks[*bi].insts.remove(*ii);
    }
    truly_dead.len()
}
