//! Peephole optimization of instruction lists.
//!
//! Rules, each applied only when liveness proves it safe:
//!
//! * delete `move r, r` and duplicate / ping-pong moves
//! * delete pure instructions whose result is never read
//! * retarget `op t ...; move d, t` to `op d ...` when `t` dies at the move
//! * fuse `cmp t, a, b; jmpifnot t, L` into one compare-and-branch
//!   (and `jmpif`, by inverting the integer comparison)
//! * fold `loadi t, k; add/sub r, a, t` into an immediate form
//! * rotate loops: `L: cmpjmp ..exit; body; jmp L; exit:` becomes `cmpjmp ..exit; B: body;
//!   inverted-cmpjmp ..B; exit:`, one dispatch fewer per iteration
//! * `cmpjmp ..L1; jmp L2; L1:` becomes one inverted `cmpjmp ..L2` (likewise `jmpif`/`jmpifnot`)
//! * a `jmp` to a `ret` is the `ret`
//! * `move t, s; op .., t, ..` reads `s` directly when `t` dies there
//! * a constant loaded again into a register that still holds it is not loaded twice
//! * remove jumps to the next instruction, thread jumps through jumps,
//!   drop unreachable code and unused labels
//!
//! Registers in `always_live` (captured by closures) are never treated as dead.

use crate::fx::{FxHashMap as HashMap, FxHashSet as HashSet};

use super::analysis::liveness;
use super::insn::*;
use super::opcode::CmpKind;
use crate::gc::BitSet;
use crate::runtime::numeric::{decode_int, ArithOp, NumTy};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PeepholeStats {
    pub moves_removed: usize,
    pub dead_removed: usize,
    pub coalesced: usize,
    pub fused: usize,
    pub immediates: usize,
    pub jumps_removed: usize,
    pub rotated: usize,
    pub branches: usize,
    pub forwarded: usize,
    pub loads: usize,
}

impl PeepholeStats {
    pub fn total(&self) -> usize {
        self.moves_removed
            + self.dead_removed
            + self.coalesced
            + self.fused
            + self.immediates
            + self.jumps_removed
            + self.rotated
            + self.branches
            + self.forwarded
            + self.loads
    }
}

/// Optimizes `items` in place until no rule applies.
pub fn peephole(items: &mut Vec<Item>, always_live: &BitSet) -> PeepholeStats {
    let mut stats = PeepholeStats::default();
    for _ in 0..16 {
        let before = stats.total();
        local_rules(items, always_live, &mut stats);
        remove_unused_labels(items);
        control_flow(items, &mut stats);
        rotate_loops(items, &mut stats);
        branch_over_jump(items, &mut stats);
        jump_to_return(items, &mut stats);
        redundant_loads(items, always_live, &mut stats);
        if stats.total() == before {
            break;
        }
    }
    remove_unused_labels(items);
    stats
}

/// The integer comparison with the same truth value when its result is negated.
fn invert(kind: CmpKind, a: Reg, b: Reg) -> Option<(CmpKind, Reg, Reg)> {
    Some(match kind {
        CmpKind::Eq => (CmpKind::Ne, a, b),
        CmpKind::Ne => (CmpKind::Eq, a, b),
        CmpKind::LtS => (CmpKind::LeS, b, a),
        CmpKind::LeS => (CmpKind::LtS, b, a),
        CmpKind::LtU => (CmpKind::LeU, b, a),
        CmpKind::LeU => (CmpKind::LtU, b, a),
        _ => return None,
    })
}

fn local_rules(items: &mut Vec<Item>, always: &BitSet, stats: &mut PeepholeStats) {
    let mut i = 0;
    let mut live = liveness(items, always);
    while i < items.len() {
        let mut changed = false;
        // 1. self-moves
        if let Insn::Move { dst, src } = items[i].insn {
            if dst == src {
                items.remove(i);
                stats.moves_removed += 1;
                live = liveness(items, always);
                continue;
            }
        }
        // 2. dead pure definitions
        if items[i].insn.is_pure() {
            if let Some(d) = items[i].insn.def() {
                if !live[i].contains(d) {
                    items.remove(i);
                    stats.dead_removed += 1;
                    live = liveness(items, always);
                    continue;
                }
            }
        }
        if i + 1 < items.len() {
            let (first, second) = (items[i].insn.clone(), items[i + 1].insn.clone());
            match (&first, &second) {
                // 3. `move a, b; move b, a` / duplicate moves: the second is redundant
                (Insn::Move { dst: a, src: b }, Insn::Move { dst: c, src: d })
                    if (a == d && b == c) || (a == c && b == d) =>
                {
                    items.remove(i + 1);
                    stats.moves_removed += 1;
                    changed = true;
                }
                // 4. retarget `op t; move d, t` to write `d` directly
                (_, Insn::Move { dst: d, src: t })
                    if first.def() == Some(*t)
                        && !live[i + 1].contains(*t)
                        && *t != *d
                        && first.clone().set_def_probe(*d) =>
                {
                    items[i].insn.set_def(*d);
                    items.remove(i + 1);
                    stats.coalesced += 1;
                    changed = true;
                }
                // 5. fuse compare + branch
                (Insn::Cmp { kind, dst: t, a, b }, Insn::JmpIfNot { cond, to })
                    if cond == t && !live[i + 1].contains(*t) && kind.fused().is_some() =>
                {
                    items[i].insn = Insn::CmpJmp { kind: kind.fused().unwrap(), a: *a, b: *b, to: *to };
                    items.remove(i + 1);
                    stats.fused += 1;
                    changed = true;
                }
                (Insn::Cmp { kind, dst: t, a, b }, Insn::JmpIf { cond, to })
                    if cond == t && !live[i + 1].contains(*t) =>
                {
                    if let Some((k2, a2, b2)) = invert(*kind, *a, *b) {
                        if let Some(f) = k2.fused() {
                            items[i].insn = Insn::CmpJmp { kind: f, a: a2, b: b2, to: *to };
                            items.remove(i + 1);
                            stats.fused += 1;
                            changed = true;
                        }
                    }
                }
                // 5b. read the source of a copy directly when the copy dies in the next instruction
                (Insn::Move { dst: t, src: s }, next)
                    if t != s && !live[i + 1].contains(*t) && {
                        let mut probe = next.clone();
                        replace_use(&mut probe, *t, *s)
                    } =>
                {
                    let mut replaced = items[i + 1].insn.clone();
                    replace_use(&mut replaced, *t, *s);
                    items[i + 1].insn = replaced;
                    items.remove(i);
                    stats.forwarded += 1;
                    changed = true;
                }
                // 6. immediates
                (Insn::LoadI { dst: t, imm }, Insn::Arith { op, ty, dst, a, b })
                    if ty.is_int() && matches!(op, ArithOp::Add | ArithOp::Sub) && !live[i + 1].contains(*t) =>
                {
                    let v = decode_int(*ty, *imm as i64 as u64);
                    let operand = if b == t && a != t {
                        Some(*a)
                    } else if a == t && b != t && *op == ArithOp::Add {
                        Some(*b)
                    } else {
                        None
                    };
                    if let Some(src) = operand {
                        let replacement = match op {
                            ArithOp::Add => add_imm(*ty, *dst, src, v),
                            _ => add_imm(*ty, *dst, src, -v),
                        };
                        if let Some(r) = replacement {
                            items[i].insn = r;
                            items[i].ref_result = false;
                            items.remove(i + 1);
                            stats.immediates += 1;
                            changed = true;
                        }
                    }
                }
                _ => {}
            }
        }
        if changed {
            live = liveness(items, always);
            // re-examine the same position: the new instruction may combine again
        } else {
            i += 1;
        }
    }
}

/// Replaces every read of `from` in `insn` by `to`; false if `insn` does not read `from` through an
/// operand field (calls and other register-window instructions read fixed positions and are left alone).
fn replace_use(insn: &mut Insn, from: Reg, to: Reg) -> bool {
    use Insn::*;
    let mut hit = false;
    let mut sub = |r: &mut Reg| {
        if *r == from {
            *r = to;
            hit = true;
        }
    };
    match insn {
        Move { src, .. } | Neg { src, .. } | BitNot { src, .. } | BoolNot { src, .. } | Cast { src, .. } => sub(src),
        AddImm { src, .. } | SubImm { src, .. } => sub(src),
        Arith { a, b, .. } | Bitwise { a, b, .. } | Cmp { a, b, .. } | CmpJmp { a, b, .. } => {
            sub(a);
            sub(b);
        }
        JmpIf { cond, .. } | JmpIfNot { cond, .. } => sub(cond),
        MatchTagJmp { obj, .. } => sub(obj),
        GetField { obj, .. } | EnumTag { obj, .. } | EnumField { obj, .. } => sub(obj),
        SetField { obj, src, .. } => {
            sub(obj);
            sub(src);
        }
        ArrayGet { arr, idx, .. } => {
            sub(arr);
            sub(idx);
        }
        ArraySet { arr, idx, src } => {
            sub(arr);
            sub(idx);
            sub(src);
        }
        MapGet { map, key, .. } => {
            sub(map);
            sub(key);
        }
        MapSet { map, key, val } => {
            sub(map);
            sub(key);
            sub(val);
        }
        Ret { src } => sub(src),
        _ => {}
    }
    hit
}

/// `src + v` as an immediate instruction, if `v` fits.
fn add_imm(ty: NumTy, dst: Reg, src: Reg, v: i128) -> Option<Insn> {
    if (-128..=127).contains(&v) {
        Some(Insn::AddImm { ty, dst, src, imm: v as i8 })
    } else if (-255..0).contains(&v) {
        Some(Insn::SubImm { ty, dst, src, imm: (-v) as u8 })
    } else {
        None
    }
}

trait SetDefProbe {
    fn set_def_probe(self, r: Reg) -> bool;
}

impl SetDefProbe for Insn {
    /// Whether `set_def` would succeed (without keeping the change).
    fn set_def_probe(mut self, r: Reg) -> bool {
        self.set_def(r)
    }
}

fn control_flow(items: &mut Vec<Item>, stats: &mut PeepholeStats) {
    // thread jumps through unconditional jumps: L1: jmp L2
    loop {
        let mut forward: HashMap<Label, Label> = HashMap::default();
        for (i, it) in items.iter().enumerate() {
            if let Insn::Label(l) = it.insn {
                // the first real instruction after the label(s)
                let mut j = i + 1;
                while j < items.len() && matches!(items[j].insn, Insn::Label(_)) {
                    j += 1;
                }
                if let Some(Insn::Jmp { to }) = items.get(j).map(|x| &x.insn) {
                    if *to != l {
                        forward.insert(l, *to);
                    }
                }
            }
        }
        let mut changed = false;
        for it in items.iter_mut() {
            if let Some(t) = it.insn.target_mut() {
                let mut cur = *t;
                let mut hops = 0;
                while let Some(&n) = forward.get(&cur) {
                    cur = n;
                    hops += 1;
                    if hops > 32 {
                        break;
                    }
                }
                if cur != *t && hops <= 32 {
                    *t = cur;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
        stats.jumps_removed += 1;
    }

    // jumps to the very next instruction (only labels in between)
    let mut i = 0;
    while i < items.len() {
        if let Some(t) = items[i].insn.target() {
            let mut j = i + 1;
            let mut lands_next = false;
            while j < items.len() {
                match items[j].insn {
                    Insn::Label(l) if l == t => {
                        lands_next = true;
                        break;
                    }
                    Insn::Label(_) => j += 1,
                    _ => break,
                }
            }
            let removable = matches!(
                items[i].insn,
                Insn::Jmp { .. }
                    | Insn::JmpIf { .. }
                    | Insn::JmpIfNot { .. }
                    | Insn::CmpJmp { .. }
                    | Insn::MatchTagJmp { .. }
            );
            if lands_next && removable {
                items.remove(i);
                stats.jumps_removed += 1;
                continue;
            }
        }
        i += 1;
    }

    // unreachable code after an unconditional transfer, up to the next label
    let mut i = 0;
    while i < items.len() {
        if !items[i].insn.falls_through() {
            let mut j = i + 1;
            while j < items.len() && !matches!(items[j].insn, Insn::Label(_)) {
                j += 1;
            }
            if j > i + 1 {
                items.drain(i + 1..j);
                stats.jumps_removed += 1;
            }
        }
        i += 1;
    }
}

/// Replaces the back edge `jmp head` of a loop whose header is a single integer compare-and-branch
/// (and whose exit directly follows the jump) by the inverted compare, jumping straight to the body.
fn rotate_loops(items: &mut Vec<Item>, stats: &mut PeepholeStats) {
    let used = items.iter().filter_map(|it| match it.insn {
        Insn::Label(l) => Some(l.0),
        _ => it.insn.target().map(|l| l.0),
    });
    let mut next_label = used.max().map_or(0, |m| m + 1);
    let mut i = 0;
    while i < items.len() {
        let Insn::Jmp { to: head } = items[i].insn else {
            i += 1;
            continue;
        };
        let Some(h) = items.iter().position(|it| matches!(it.insn, Insn::Label(l) if l == head)) else {
            i += 1;
            continue;
        };
        // the header's first real instruction
        let mut k = h + 1;
        while k < items.len() && matches!(items[k].insn, Insn::Label(_)) {
            k += 1;
        }
        let rotation = match items.get(k).map(|x| &x.insn) {
            Some(&Insn::CmpJmp { kind, a, b, to: exit }) if h < i && k < i => {
                // the loop exit must be what follows the jump
                let mut j = i + 1;
                let mut exit_follows = false;
                while let Some(Insn::Label(l)) = items.get(j).map(|x| &x.insn) {
                    if *l == exit {
                        exit_follows = true;
                        break;
                    }
                    j += 1;
                }
                if exit_follows {
                    invert(kind.cmp(), a, b).and_then(|(k2, a2, b2)| k2.fused().map(|f| (f, a2, b2)))
                } else {
                    None
                }
            }
            _ => None,
        };
        let Some((kind, a, b)) = rotation else {
            i += 1;
            continue;
        };
        let body = Label(next_label);
        next_label += 1;
        items.insert(k + 1, Item { insn: Insn::Label(body), ref_result: false });
        items[i + 1] = Item { insn: Insn::CmpJmp { kind, a, b, to: body }, ref_result: false };
        stats.rotated += 1;
        i += 2;
    }
}

/// `cmpjmp .., L1; jmp L2; L1:` -> one branch with the inverted condition straight to `L2`.
fn branch_over_jump(items: &mut Vec<Item>, stats: &mut PeepholeStats) {
    let mut i = 0;
    while i + 1 < items.len() {
        let Insn::Jmp { to: far } = items[i + 1].insn else {
            i += 1;
            continue;
        };
        let near = match items[i].insn.target() {
            Some(t) if !matches!(items[i].insn, Insn::Jmp { .. } | Insn::MatchTagJmp { .. }) => t,
            _ => {
                i += 1;
                continue;
            }
        };
        // `near` must be the very next position after the jump
        let mut j = i + 2;
        let mut lands = false;
        while let Some(Insn::Label(l)) = items.get(j).map(|x| &x.insn) {
            if *l == near {
                lands = true;
                break;
            }
            j += 1;
        }
        if !lands || near == far {
            i += 1;
            continue;
        }
        let inverted = match items[i].insn.clone() {
            Insn::CmpJmp { kind, a, b, .. } => invert(kind.cmp(), a, b)
                .and_then(|(k, a, b)| k.fused().map(|f| Insn::CmpJmp { kind: f, a, b, to: far })),
            Insn::JmpIfNot { cond, .. } => Some(Insn::JmpIf { cond, to: far }),
            Insn::JmpIf { cond, .. } => Some(Insn::JmpIfNot { cond, to: far }),
            _ => None,
        };
        match inverted {
            Some(new) => {
                items[i].insn = new;
                items.remove(i + 1);
                stats.branches += 1;
            }
            None => i += 1,
        }
    }
}

/// The first real instruction at or after the label `to`.
fn instruction_at(items: &[Item], to: Label) -> Option<&Insn> {
    let at = items.iter().position(|it| matches!(it.insn, Insn::Label(l) if l == to))?;
    items[at + 1..].iter().map(|it| &it.insn).find(|i| !matches!(i, Insn::Label(_)))
}

/// A jump whose target is a `ret` just returns; a branch that returns the same value on both
/// outcomes is not needed.
fn jump_to_return(items: &mut Vec<Item>, stats: &mut PeepholeStats) {
    let mut i = 0;
    while i < items.len() {
        match items[i].insn.clone() {
            Insn::Jmp { to } => {
                if let Some(&Insn::Ret { src }) = instruction_at(items, to) {
                    items[i].insn = Insn::Ret { src };
                    stats.branches += 1;
                }
            }
            Insn::JmpIf { to, .. } | Insn::JmpIfNot { to, .. } | Insn::CmpJmp { to, .. } => {
                if let (Some(Insn::Ret { src: here }), Some(&Insn::Ret { src: there })) =
                    (items.get(i + 1).map(|x| &x.insn), instruction_at(items, to))
                {
                    if *here == there {
                        items.remove(i);
                        stats.branches += 1;
                        continue;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
}

/// Drops a constant load into a register that is known to hold that very constant (within a
/// straight-line stretch: labels and calls forget everything they might change).
fn redundant_loads(items: &mut Vec<Item>, always: &BitSet, stats: &mut PeepholeStats) {
    let mut known: HashMap<Reg, Insn> = HashMap::default();
    let mut i = 0;
    while i < items.len() {
        let insn = items[i].insn.clone();
        if matches!(insn, Insn::Label(_)) {
            known.clear();
            i += 1;
            continue;
        }
        if matches!(insn, Insn::LoadI { .. } | Insn::LoadK { .. } | Insn::LoadStr { .. }) {
            let dst = insn.def().expect("loads define a register");
            if known.get(&dst) == Some(&insn) {
                items.remove(i);
                stats.loads += 1;
                continue;
            }
            known.insert(dst, insn);
            i += 1;
            continue;
        }
        if let Some(from) = insn.clobbers_from() {
            // the callee may also change registers captured by closures
            known.retain(|r, _| (*r as u16) < from && !always.contains(*r));
        }
        if let Some(d) = insn.def() {
            known.remove(&d);
        }
        i += 1;
    }
}

fn remove_unused_labels(items: &mut Vec<Item>) {
    let used: HashSet<Label> = items.iter().filter_map(|it| it.insn.target()).collect();
    items.retain(|it| match it.insn {
        Insn::Label(l) => used.contains(&l),
        _ => true,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(v: Vec<Insn>) -> Vec<Item> {
        v.into_iter().map(Item::new).collect()
    }

    fn loads(v: Vec<Insn>, always: &[u8]) -> (Vec<Insn>, usize) {
        let mut it = items(v);
        let mut stats = PeepholeStats::default();
        let always: BitSet = always.iter().copied().collect();
        redundant_loads(&mut it, &always, &mut stats);
        (it.into_iter().map(|i| i.insn).collect(), stats.loads)
    }

    #[test]
    fn a_second_identical_load_goes() {
        let (out, n) = loads(
            vec![Insn::LoadI { dst: 1, imm: 5 }, Insn::Move { dst: 2, src: 1 }, Insn::LoadI { dst: 1, imm: 5 }],
            &[],
        );
        assert_eq!((out.len(), n), (2, 1));
    }

    #[test]
    fn other_values_writes_labels_and_calls_keep_the_load() {
        for between in [
            Insn::LoadI { dst: 1, imm: 6 },
            Insn::Move { dst: 1, src: 0 },
            Insn::Label(Label(0)),
            Insn::Call { func: 0, base: 1, nargs: 0 },
        ] {
            let (out, n) = loads(vec![Insn::LoadI { dst: 1, imm: 5 }, between, Insn::LoadI { dst: 1, imm: 5 }], &[]);
            assert_eq!((out.len(), n), (3, 0), "{out:?}");
        }
        // a call above the register leaves it alone
        let (out, n) = loads(
            vec![
                Insn::LoadI { dst: 1, imm: 5 },
                Insn::Call { func: 0, base: 4, nargs: 0 },
                Insn::LoadI { dst: 1, imm: 5 },
            ],
            &[],
        );
        assert_eq!((out.len(), n), (2, 1), "{out:?}");
        // ... unless a closure may have changed it
        let (out, n) = loads(
            vec![
                Insn::LoadI { dst: 1, imm: 5 },
                Insn::Call { func: 0, base: 4, nargs: 0 },
                Insn::LoadI { dst: 1, imm: 5 },
            ],
            &[1],
        );
        assert_eq!((out.len(), n), (3, 0), "{out:?}");
    }
}
