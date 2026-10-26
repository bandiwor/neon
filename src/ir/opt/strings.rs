//! Compile-time string work.
//!
//! * `"a" + "b"` of two constants is one constant string (chains collapse step by step).
//! * Two printing calls in a row with constant text become one: `print("x = "); println("1")` is
//!   `print("x = 1\n")`. The bytes written are exactly the same.
//!
//! * `to_string(c)` and `c.format(spec)` of constants are constant strings (so a format string with
//!   constant fields is one string).
//! * `print(to_string(x))` of a number, boolean or character prints `x` directly: no string is built.
//!
//! A constant is a register whose only definition is `const str`.

use crate::builtin::Builtin;
use crate::fx::FxHashMap as HashMap;
use crate::ir::effects::FuncEffects;
use crate::ir::types::*;

use super::simplify::def_counts;

fn intern(strings: &mut Vec<String>, s: String) -> u32 {
    match strings.iter().position(|x| *x == s) {
        Some(i) => i as u32,
        None => {
            strings.push(s);
            strings.len() as u32 - 1
        }
    }
}

fn use_counts(f: &IrFunc) -> Vec<u32> {
    let mut uses = vec![0u32; f.regs.len()];
    for b in &f.blocks {
        for i in &b.insts {
            i.for_each_use(|r| uses[r.index()] += 1);
        }
        b.term.for_each_use(|r| uses[r.index()] += 1);
    }
    uses
}

/// The printing builtin of `inst` if it has one constant string argument: the builtin, the text
/// it would write and the register holding the argument.
fn const_print(inst: &Inst, consts: &HashMap<VReg, u32>, strings: &[String]) -> Option<(Builtin, String)> {
    match inst {
        Inst::Intrinsic { b: b @ (Builtin::Print | Builtin::Println), args, .. } => match args.as_slice() {
            &[a] => consts.get(&a).map(|&i| {
                let mut text = strings[i as usize].clone();
                if *b == Builtin::Println {
                    text.push('\n');
                }
                (*b, text)
            }),
            _ => None,
        },
        _ => None,
    }
}

/// `print(s)` / `println(s)` where `s = to_string(x)` with `x` a number, boolean or character that
/// still holds the same value: the call printing `x` itself.
fn print_without_to_string(
    insts: &[Inst],
    at: usize,
    descs: &[TypeDesc],
    uses: &[u32],
    pinned: &[bool],
) -> Option<Inst> {
    let Inst::Intrinsic { dst, b: b @ (Builtin::Print | Builtin::Println), args, .. } = &insts[at] else {
        return None;
    };
    let [s] = args.as_slice() else { return None };
    if uses[s.index()] != 1 || pinned[s.index()] {
        return None;
    }
    let from = at.saturating_sub(24);
    let k = (from..at).rev().find(|&k| insts[k].def() == Some(*s))?;
    let Inst::Intrinsic { b: Builtin::ToString, desc: Some(d), args: conv_args, .. } = &insts[k] else {
        return None;
    };
    let [x] = conv_args.as_slice() else { return None };
    if !matches!(descs[d.index()], TypeDesc::Num(_) | TypeDesc::Bool | TypeDesc::Char) || pinned[x.index()] {
        return None;
    }
    if insts[k + 1..at].iter().any(|i| i.def() == Some(*x)) {
        return None;
    }
    Some(Inst::Intrinsic { dst: *dst, b: *b, desc: Some(*d), args: vec![*x] })
}

/// The constant text of `to_string(x)` / `x.format(spec)` when the operands are constants.
fn fold_conversion(
    inst: &Inst,
    descs: &[TypeDesc],
    strings: &[String],
    texts: &HashMap<VReg, u32>,
    nums: &HashMap<VReg, u64>,
) -> Option<String> {
    use crate::runtime::{fmtspec, format as fmt};
    let Inst::Intrinsic { b: b @ (Builtin::ToString | Builtin::Format), desc: Some(d), args, .. } = inst else {
        return None;
    };
    let desc = &descs[d.index()];
    let value = *args.first()?;
    match (b, desc) {
        (Builtin::ToString, TypeDesc::Num(n)) => nums.get(&value).map(|&v| fmt::format_num(*n, v)),
        (Builtin::ToString, TypeDesc::Bool) => nums.get(&value).map(|&v| fmt::format_bool(v).to_string()),
        (Builtin::ToString, TypeDesc::Char) => nums.get(&value).map(|&v| fmt::format_char(v)),
        (Builtin::ToString, TypeDesc::Str) => texts.get(&value).map(|&i| strings[i as usize].clone()),
        (Builtin::Format, _) => {
            let spec = &strings[*texts.get(args.get(1)?)? as usize];
            let kind = fmtspec::kind_of(desc)?;
            let (bits, text) = if kind == fmtspec::Kind::Str {
                (0, Some(strings[*texts.get(&value)? as usize].as_str()))
            } else {
                (*nums.get(&value)?, None)
            };
            fmtspec::format_with(kind, bits, text, spec).ok()
        }
        _ => None,
    }
}

pub fn fold_strings(strings: &mut Vec<String>, descs: &[TypeDesc], f: &mut IrFunc, fx: &FuncEffects) -> usize {
    let defs = def_counts(f);
    let mut uses = use_counts(f);
    let mut consts: HashMap<VReg, u32> = HashMap::default();
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Const { dst, val: Const::Str(s) } = i {
                if defs[dst.index()] == 1 && !f.pinned[dst.index()] {
                    consts.insert(*dst, *s);
                }
            }
        }
    }
    let mut nums: HashMap<VReg, u64> = HashMap::default();
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Const { dst, val: Const::Bits(v) } = i {
                if defs[dst.index()] == 1 && !f.pinned[dst.index()] {
                    nums.insert(*dst, *v);
                }
            }
        }
    }
    let mut changes = 0;
    for bi in 0..f.blocks.len() {
        let mut ii = 0;
        while ii < f.blocks[bi].insts.len() {
            if let Some(new) = print_without_to_string(&f.blocks[bi].insts, ii, descs, &uses, &f.pinned) {
                f.blocks[bi].insts[ii] = new;
                changes += 1;
                continue;
            }
            if let (Some(dst), Some(text)) =
                (f.blocks[bi].insts[ii].def(), fold_conversion(&f.blocks[bi].insts[ii], descs, strings, &consts, &nums))
            {
                if !f.pinned[dst.index()] {
                    let idx = intern(strings, text);
                    f.blocks[bi].insts[ii] = Inst::Const { dst, val: Const::Str(idx) };
                    if defs[dst.index()] == 1 {
                        consts.insert(dst, idx);
                    }
                    changes += 1;
                    continue;
                }
            }
            match f.blocks[bi].insts[ii].clone() {
                Inst::Intrinsic { dst, b: Builtin::StrConcat, args, .. } if !f.pinned[dst.index()] => {
                    if let &[a, b] = args.as_slice() {
                        if let (Some(&x), Some(&y)) = (consts.get(&a), consts.get(&b)) {
                            let joined = format!("{}{}", strings[x as usize], strings[y as usize]);
                            let idx = intern(strings, joined);
                            f.blocks[bi].insts[ii] = Inst::Const { dst, val: Const::Str(idx) };
                            if defs[dst.index()] == 1 {
                                consts.insert(dst, idx);
                            }
                            changes += 1;
                        }
                    }
                }
                first @ Inst::Intrinsic { b: Builtin::Print | Builtin::Println, .. } => {
                    // the next printing call, with only side-effect-free instructions in between
                    let insts = &f.blocks[bi].insts;
                    let mut j = ii + 1;
                    while j < insts.len() && insts[j].removable(fx) {
                        j += 1;
                    }
                    // `print(x); println()` is `println(x)`
                    if let (
                        Inst::Intrinsic { b: Builtin::Print, desc, dst: first_dst, args: print_args },
                        Some(Inst::Intrinsic { b: Builtin::Println, dst: next_dst, args: line_args, .. }),
                    ) = (&first, insts.get(j))
                    {
                        let between_keeps_x = |x: VReg| insts[ii + 1..j].iter().all(|i| i.def() != Some(x));
                        if let ([x], true, true, true) = (
                            print_args.as_slice(),
                            line_args.is_empty(),
                            uses[first_dst.index()] == 0,
                            print_args.first().is_some_and(|&x| between_keeps_x(x)),
                        ) {
                            let merged =
                                Inst::Intrinsic { dst: *next_dst, b: Builtin::Println, desc: *desc, args: vec![*x] };
                            let insts = &mut f.blocks[bi].insts;
                            insts[j] = merged;
                            insts.remove(ii);
                            changes += 1;
                            continue;
                        }
                    }
                    let (Some((_, one)), Some(next), true) =
                        (const_print(&first, &consts, strings), insts.get(j).cloned(), j < insts.len())
                    else {
                        ii += 1;
                        continue;
                    };
                    let Some((kind, two)) = const_print(&next, &consts, strings) else {
                        ii += 1;
                        continue;
                    };
                    let Inst::Intrinsic { dst: first_dst, desc, .. } = &first else { unreachable!() };
                    let Inst::Intrinsic { dst: next_dst, .. } = &next else { unreachable!() };
                    // the first call's (unit) result must be unused
                    if uses[first_dst.index()] != 0 {
                        ii += 1;
                        continue;
                    }
                    let mut text = one + &two;
                    if kind == Builtin::Println {
                        text.pop(); // Println adds the newline itself
                    }
                    let idx = intern(strings, text);
                    let arg = f.new_reg(IrTy::Ref);
                    uses.resize(f.regs.len(), 0);
                    let merged = Inst::Intrinsic { dst: *next_dst, b: kind, desc: *desc, args: vec![arg] };
                    let insts = &mut f.blocks[bi].insts;
                    insts[j] = merged;
                    insts.insert(j, Inst::Const { dst: arg, val: Const::Str(idx) });
                    insts.remove(ii);
                    consts.insert(arg, idx);
                    changes += 1;
                    // `ii` now holds what followed the removed call; the merged call is further on
                    continue;
                }
                _ => {}
            }
            ii += 1;
        }
    }
    changes
}
