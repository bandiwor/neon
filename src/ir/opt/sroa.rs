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
use crate::ir::escape::Classes;
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
pub fn sroa_func(f: &mut IrFunc, classes: &Classes) -> usize {
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

/// What the objects of one alias class look like: a struct (`tag` unused) or an enum, and the types of
/// the field slots (for an enum the longest variant; the slots of different variants share registers).
#[derive(Clone, PartialEq)]
struct Shape {
    is_enum: bool,
    slots: Vec<IrTy>,
    /// The fewest slots any allocation fills; the others are set to zero where a shorter variant is built
    /// (so they cannot hold references).
    min: usize,
}

/// Scalar replacement of structs and enums that are rebuilt, copied around or *merged* at a join
/// (`let p = if c { (a, b) } else { (b, a) }`, a loop-carried `p = V { .. }`): when every register that
/// may hold the object is only ever assigned a new object of one shape or a copy of another such
/// register, and the object is only *read* (no field is ever assigned, so a copy is as good as a shared
/// reference), each register gets one register per field slot and, for enums, one for the variant.
pub fn sroa_classes(f: &mut IrFunc, classes: &Classes) -> usize {
    use std::collections::BTreeMap;
    // members of each class that has an allocation
    let mut members: BTreeMap<usize, Vec<VReg>> = BTreeMap::new();
    for r in 0..f.regs.len() {
        let r = VReg(r as u32);
        if f.reg_ty(r) == IrTy::Ref {
            members.entry(classes.class_id(r)).or_default().push(r);
        }
    }
    // the shape of each class, or None when it cannot be replaced
    let mut shape: BTreeMap<usize, Option<Shape>> = BTreeMap::new();
    let mut allocs: BTreeMap<usize, usize> = BTreeMap::new();
    for b in &f.blocks {
        for i in &b.insts {
            let (dst, is_enum, fields) = match i {
                Inst::NewStruct { dst, fields, .. } => (*dst, false, fields),
                Inst::NewEnum { dst, fields, .. } => (*dst, true, fields),
                _ => continue,
            };
            let c = classes.class_id(dst);
            let types: Vec<IrTy> = fields.iter().map(|&r| f.reg_ty(r)).collect();
            *allocs.entry(c).or_insert(0) += 1;
            let merged = match shape.get(&c) {
                None => Some(Shape { is_enum, min: types.len(), slots: types }),
                Some(Some(have)) if have.is_enum == is_enum => {
                    // structs must agree exactly; enum variants share the slots they have in common
                    let n = if is_enum { have.slots.len().max(types.len()) } else { have.slots.len() };
                    let mut slots = have.slots.clone();
                    let mut good = is_enum || have.slots == types;
                    for (k, t) in types.iter().enumerate() {
                        match slots.get(k) {
                            Some(have_t) if have_t != t => good = false,
                            Some(_) => {}
                            None => slots.push(*t),
                        }
                    }
                    slots.truncate(n);
                    let min = have.min.min(types.len());
                    good &= slots[min..].iter().all(|t| *t != IrTy::Ref);
                    good.then_some(Shape { is_enum, slots, min })
                }
                _ => None,
            };
            shape.insert(c, merged);
        }
    }
    let mut ok: BTreeMap<usize, bool> = BTreeMap::new();
    for (&c, s) in &shape {
        let mem = &members[&c];
        let good = s.is_some()
            && mem.iter().all(|&m| classes.is_private(m) && !f.pinned[m.index()] && m.index() >= f.params as usize);
        ok.insert(c, good);
    }
    let in_class = |r: VReg, ok: &BTreeMap<usize, bool>| ok.get(&classes.class_id(r)).copied().unwrap_or(false);
    let class_is_enum = |r: VReg| shape.get(&classes.class_id(r)).and_then(|s| s.as_ref()).is_some_and(|s| s.is_enum);
    // definitions and uses must keep to the allowed shapes
    for b in &f.blocks {
        for i in &b.insts {
            let def_ok = match i {
                Inst::NewStruct { .. } | Inst::NewEnum { .. } => true,
                Inst::Move { dst, src } => {
                    !in_class(*dst, &ok) || in_class(*src, &ok) && classes.class_id(*dst) == classes.class_id(*src)
                }
                other => other.def().is_none_or(|d| !in_class(d, &ok)),
            };
            if !def_ok {
                if let Some(d) = i.def() {
                    ok.insert(classes.class_id(d), false);
                }
            }
            let mut bad: Vec<VReg> = Vec::new();
            i.for_each_use(|u| {
                if !in_class(u, &ok) {
                    return;
                }
                // a field no allocation has (code that only runs for a variant this function never builds)
                let slots = shape.get(&classes.class_id(u)).and_then(|s| s.as_ref()).map_or(0, |s| s.slots.len());
                let allowed = match i {
                    Inst::GetField { obj, idx, .. } => {
                        *obj == u && occurrences(i, u) == 1 && !class_is_enum(u) && (*idx as usize) < slots
                    }
                    Inst::EnumField { obj, idx, .. } => {
                        *obj == u && occurrences(i, u) == 1 && class_is_enum(u) && (*idx as usize) < slots
                    }
                    Inst::EnumTag { obj, .. } => *obj == u && occurrences(i, u) == 1 && class_is_enum(u),
                    Inst::Move { src, dst } => *src == u && in_class(*dst, &ok),
                    _ => false,
                };
                if !allowed {
                    bad.push(u);
                }
            });
            for u in bad {
                ok.insert(classes.class_id(u), false);
            }
        }
        let mut bad: Vec<VReg> = Vec::new();
        b.term.for_each_use(|u| {
            let is_tag_test = matches!(b.term, Term::BranchTag { obj, .. } if obj == u) && class_is_enum(u);
            if !is_tag_test {
                bad.push(u);
            }
        });
        for u in bad {
            ok.insert(classes.class_id(u), false);
        }
    }
    // a class that lost its status after others were judged may leave Moves pointing across: recheck
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Move { dst, src } = i {
                if in_class(*dst, &ok) != in_class(*src, &ok) {
                    ok.insert(classes.class_id(*dst), false);
                    ok.insert(classes.class_id(*src), false);
                }
            }
        }
    }
    let chosen: Vec<usize> = ok.iter().filter(|(_, &g)| g).map(|(&c, _)| c).collect();
    if chosen.is_empty() {
        return 0;
    }
    // one register per field slot of every member, and one for the variant of an enum
    let mut fr: StdMap<VReg, Vec<VReg>> = StdMap::new();
    let mut tag_of: StdMap<VReg, VReg> = StdMap::new();
    for &c in &chosen {
        let sh = shape[&c].clone().expect("a shape");
        for &m in &members[&c] {
            let regs: Vec<VReg> = sh.slots.iter().map(|&t| f.new_reg(t)).collect();
            fr.insert(m, regs);
            if sh.is_enum {
                tag_of.insert(m, f.new_reg(IrTy::Num(crate::runtime::NumTy::U32)));
            }
        }
    }
    for bi in 0..f.blocks.len() {
        let old = std::mem::take(&mut f.blocks[bi].insts);
        let mut out = Vec::with_capacity(old.len());
        for inst in old {
            match inst {
                Inst::NewStruct { dst, fields, .. } if fr.contains_key(&dst) => {
                    for (to, from) in fr[&dst].iter().zip(&fields) {
                        out.push(Inst::Move { dst: *to, src: *from });
                    }
                }
                Inst::NewEnum { dst, tag, fields, .. } if fr.contains_key(&dst) => {
                    out.push(Inst::Const { dst: tag_of[&dst], val: Const::Bits(tag as u64) });
                    for (to, from) in fr[&dst].iter().zip(&fields) {
                        out.push(Inst::Move { dst: *to, src: *from });
                    }
                    // slots of longer variants stay defined (and are never read)
                    for to in fr[&dst].iter().skip(fields.len()) {
                        let val = if f.reg_ty(*to) == IrTy::Unit { Const::Unit } else { Const::Bits(0) };
                        out.push(Inst::Const { dst: *to, val });
                    }
                }
                Inst::Move { dst, src } if fr.contains_key(&dst) => {
                    if dst != src {
                        for (to, from) in fr[&dst].iter().zip(&fr[&src]) {
                            out.push(Inst::Move { dst: *to, src: *from });
                        }
                        if let (Some(&t), Some(&u)) = (tag_of.get(&dst), tag_of.get(&src)) {
                            out.push(Inst::Move { dst: t, src: u });
                        }
                    }
                }
                Inst::GetField { dst, obj, idx } | Inst::EnumField { dst, obj, idx } if fr.contains_key(&obj) => {
                    out.push(Inst::Move { dst, src: fr[&obj][idx as usize] });
                }
                Inst::EnumTag { dst, obj } if tag_of.contains_key(&obj) => {
                    out.push(Inst::Cast {
                        dst,
                        from: crate::runtime::NumTy::U32,
                        to: match f.reg_ty(dst) {
                            IrTy::Num(n) => n,
                            _ => crate::runtime::NumTy::U32,
                        },
                        src: tag_of[&obj],
                    });
                }
                other => out.push(other),
            }
        }
        f.blocks[bi].insts = out;
        if let Term::BranchTag { obj, tag, then, els } = f.blocks[bi].term {
            if let Some(&t) = tag_of.get(&obj) {
                let k = f.new_reg(IrTy::Num(crate::runtime::NumTy::U32));
                let cond = f.new_reg(IrTy::Bool);
                f.blocks[bi].insts.push(Inst::Const { dst: k, val: Const::Bits(tag as u64) });
                f.blocks[bi].insts.push(Inst::Cmp {
                    dst: cond,
                    op: crate::runtime::CmpOp::Eq,
                    ty: CmpTy::Num(crate::runtime::NumTy::U32),
                    lhs: t,
                    rhs: k,
                });
                f.blocks[bi].term = Term::Branch { cond, then, els };
            }
        }
    }
    chosen.len()
}

/// Builtins that only put things into a container they are given, never trap, and whose result
/// (unit or a flag) nobody has to look at.
fn write_only(b: Builtin) -> bool {
    use Builtin::*;
    matches!(b, ArrPush | ArrClear | ArrReverse | MapInsert | MapClear | MapRemove | SetInsert | SetRemove | SetClear)
}

/// Removes private objects that are only written to, together with the writes.
pub fn dead_objects(f: &mut IrFunc, classes: &Classes) -> usize {
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
