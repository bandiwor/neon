//! Structural verification of compiled bytecode.
//!
//! Catches compiler bugs early: out-of-range operands, malformed call windows,
//! reads of registers that no path has written, and stack maps that disagree
//! with the instruction stream.

use std::collections::HashMap;

use super::analysis::build_cfg;
use super::asm::{assemble, decode};
use super::chunk::{BytecodeProgram, Chunk};
use super::insn::{Capture, Insn, Item, Reg};
use crate::gc::BitSet;

/// Verifies every chunk; the error names the chunk and the problem.
pub fn verify_program(p: &BytecodeProgram) -> Result<(), String> {
    if p.entry as usize >= p.chunks.len() {
        return Err("entry chunk out of range".into());
    }
    for (i, c) in p.chunks.iter().enumerate() {
        verify_chunk(p, c).map_err(|e| format!("chunk {i} `{}`: {e}", c.name))?;
    }
    Ok(())
}

pub fn verify_chunk(p: &BytecodeProgram, c: &Chunk) -> Result<(), String> {
    let insns = decode(&c.code).map_err(|e| format!("does not decode: {e}"))?;
    let frame = c.frame_size as usize;
    if frame == 0 || frame > 256 {
        return Err(format!("frame size {frame} out of range"));
    }
    if (c.params as usize) > frame {
        return Err("more parameters than registers".into());
    }
    if c.param_refs.len() != c.params as usize {
        return Err("param_refs does not match the parameter count".into());
    }
    let reg_ok = |r: Reg, what: &str| -> Result<(), String> {
        if (r as usize) < frame {
            Ok(())
        } else {
            Err(format!("{what}: r{r} outside the frame of {frame}"))
        }
    };

    // operand ranges
    for insn in &insns {
        let mut bad = Ok(());
        insn.for_each_use(|r| bad = bad.clone().and(reg_ok(r, &format!("{insn:?} reads"))));
        bad?;
        if let Some(d) = insn.def() {
            reg_ok(d, &format!("{insn:?} writes"))?;
        }
        match insn {
            Insn::LoadK { k, .. } if *k as usize >= p.constants.len() => {
                return Err(format!("constant {k} out of range"))
            }
            Insn::LoadStr { s, .. } if *s as usize >= p.strings.len() => {
                return Err(format!("string {s} out of range"))
            }
            Insn::GetGlobal { g, .. } | Insn::SetGlobal { g, .. } if *g as usize >= p.globals.len() => {
                return Err(format!("global {g} out of range"))
            }
            Insn::NewStruct { desc, .. }
            | Insn::NewEnum { desc, .. }
            | Insn::NewArray { desc, .. }
            | Insn::NewMap { desc, .. }
            | Insn::NewSet { desc, .. }
                if *desc as usize >= p.descs.len() =>
            {
                return Err(format!("descriptor {desc} out of range"))
            }
            Insn::Intrinsic { desc: Some(d), .. } if *d as usize >= p.descs.len() => {
                return Err(format!("descriptor {d} out of range"))
            }
            Insn::GetUpval { idx, .. } | Insn::SetUpval { idx, .. } if *idx as usize >= c.upvals.len() => {
                return Err(format!("upvalue {idx} out of range ({} upvalues)", c.upvals.len()))
            }
            Insn::Call { base, func, .. } => {
                let callee = p.chunks.get(*func as usize).ok_or_else(|| format!("call of unknown chunk {func}"))?;
                if *base as usize + callee.params as usize > frame {
                    return Err(format!("call window r{base}+{} exceeds the frame", callee.params));
                }
            }
            Insn::MakeClosure { func, captures, .. } => {
                let callee = p.chunks.get(*func as usize).ok_or_else(|| format!("closure of unknown chunk {func}"))?;
                if callee.upvals.len() != captures.len() {
                    return Err(format!(
                        "closure of `{}` captures {} values, expects {}",
                        callee.name,
                        captures.len(),
                        callee.upvals.len()
                    ));
                }
                for cap in captures {
                    match cap {
                        Capture::Local(r) => reg_ok(*r, "capture")?,
                        Capture::Upvalue(u) if *u as usize >= c.upvals.len() => {
                            return Err(format!("captures own upvalue {u} out of range"))
                        }
                        _ => {}
                    }
                }
            }
            Insn::Close { reg } => reg_ok(*reg, "close")?,
            _ => {}
        }
    }

    // the last instruction must not fall off the end
    if let Some(last) = insns.iter().rev().find(|i| !matches!(i, Insn::Label(_))) {
        if last.falls_through() {
            return Err("the function can fall off its end".into());
        }
    }

    // stack maps
    let asm = assemble(&insns).map_err(|e| format!("does not reassemble: {e}"))?;
    if asm.code.len() != c.code.len() {
        return Err("decoded length differs from the code length".into());
    }
    let pc_of_insn = asm.pcs;
    let safe: HashMap<u32, usize> =
        insns.iter().enumerate().filter(|(_, i)| i.is_safe_point()).map(|(idx, _)| (pc_of_insn[idx], idx)).collect();
    for w in c.stack_maps.windows(2) {
        if w[0].pc >= w[1].pc {
            return Err("stack maps are not sorted by pc".into());
        }
    }
    for m in &c.stack_maps {
        if !safe.contains_key(&m.pc) {
            return Err(format!("stack map at pc {} is not at a safe-point", m.pc));
        }
        if m.live_roots.iter().any(|r| (r as usize) >= frame) {
            return Err(format!("stack map at pc {} roots a register outside the frame", m.pc));
        }
    }
    for pc in safe.keys() {
        if c.stack_map_at(*pc).is_none() {
            return Err(format!("safe-point at pc {pc} has no stack map"));
        }
    }

    check_definitely_assigned(c, &insns)
}

/// Every register an instruction reads must have been written on every path
/// that reaches it (parameters count as written at entry).
fn check_definitely_assigned(c: &Chunk, insns: &[Insn]) -> Result<(), String> {
    let items: Vec<Item> = insns.iter().cloned().map(Item::new).collect();
    let cfg = build_cfg(&items);
    let n = items.len();
    let mut all = BitSet::new();
    for r in 0..=255u8 {
        all.insert(r);
    }
    // must-defined: start from "everything" and intersect along edges
    let mut state_in = vec![all; n];
    let mut entry = BitSet::new();
    for r in 0..c.params {
        entry.insert(r);
    }
    if n == 0 {
        return Ok(());
    }
    state_in[0] = entry;
    let mut visited = vec![false; n];
    let mut stack = vec![0usize];
    visited[0] = true;
    while let Some(i) = stack.pop() {
        for &t in &cfg.succ[i] {
            if !visited[t] {
                visited[t] = true;
                stack.push(t);
            }
        }
    }
    let mut changed = true;
    while changed {
        changed = false;
        for i in 0..n {
            if !visited[i] {
                continue;
            }
            let mut out = state_in[i];
            let insn = &items[i].insn;
            if let Some(from) = insn.clobbers_from() {
                for r in from..256 {
                    out.remove(r as u8);
                }
            }
            if let Some(d) = insn.def() {
                out.insert(d);
            }
            for &t in &cfg.succ[i] {
                let mut merged = state_in[t];
                merged.intersect_with(&out);
                if merged != state_in[t] {
                    state_in[t] = merged;
                    changed = true;
                }
            }
        }
    }
    for (i, it) in items.iter().enumerate() {
        if !visited[i] {
            continue;
        }
        let mut missing = None;
        it.insn.for_each_use(|r| {
            // calls read their callee-frame window, whose registers are written just before
            if !state_in[i].contains(r) {
                missing = Some(r);
            }
        });
        if let Some(r) = missing {
            return Err(format!("{:?} reads r{r}, which is not written on every path", it.insn));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::asm::assemble;
    use crate::bytecode::insn::Label;
    use crate::gc::StackMap;
    use crate::runtime::numeric::{ArithOp, NumTy};

    fn chunk_of(insns: &[Insn], params: u8, frame: u16) -> Chunk {
        let asm = assemble(insns).unwrap();
        let mut stack_maps = Vec::new();
        for (i, insn) in insns.iter().enumerate() {
            if insn.is_safe_point() {
                stack_maps.push(StackMap { pc: asm.pcs[i], live_roots: BitSet::new() });
            }
        }
        Chunk {
            name: "t".into(),
            code: asm.code,
            params,
            frame_size: frame,
            param_refs: vec![false; params as usize],
            upvals: vec![],
            stack_maps,
        }
    }

    fn prog(c: Chunk) -> BytecodeProgram {
        BytecodeProgram { chunks: vec![c], ..Default::default() }
    }

    fn check(insns: &[Insn], params: u8, frame: u16) -> Result<(), String> {
        let p = prog(chunk_of(insns, params, frame));
        verify_program(&p)
    }

    fn add(dst: Reg, a: Reg, b: Reg) -> Insn {
        Insn::Arith { op: ArithOp::Add, ty: NumTy::I64, dst, a, b }
    }

    #[test]
    fn accepts_a_valid_function() {
        let l = Label(0);
        let insns = [
            Insn::JmpIf { cond: 0, to: l },
            Insn::LoadI { dst: 1, imm: 1 },
            Insn::Ret { src: 1 },
            Insn::Label(l),
            add(1, 0, 0),
            Insn::Ret { src: 1 },
        ];
        check(&insns, 1, 2).unwrap();
    }

    #[test]
    fn rejects_registers_outside_the_frame() {
        let e = check(&[Insn::LoadI { dst: 5, imm: 1 }, Insn::Ret { src: 5 }], 0, 3).unwrap_err();
        assert!(e.contains("outside the frame"), "{e}");
    }

    #[test]
    fn rejects_reads_of_unwritten_registers() {
        let e = check(&[add(1, 0, 0), Insn::Ret { src: 1 }], 0, 2).unwrap_err();
        assert!(e.contains("not written"), "{e}");
        // written on only one path
        let l = Label(0);
        let e = check(
            &[Insn::JmpIf { cond: 0, to: l }, Insn::LoadI { dst: 1, imm: 1 }, Insn::Label(l), Insn::Ret { src: 1 }],
            1,
            2,
        )
        .unwrap_err();
        assert!(e.contains("not written"), "{e}");
    }

    #[test]
    fn rejects_falling_off_the_end() {
        let e = check(&[Insn::LoadI { dst: 0, imm: 1 }], 0, 1).unwrap_err();
        assert!(e.contains("fall off"), "{e}");
    }

    #[test]
    fn rejects_bad_pool_indices() {
        let e = check(&[Insn::LoadK { dst: 0, k: 3 }, Insn::Ret { src: 0 }], 0, 1).unwrap_err();
        assert!(e.contains("constant"), "{e}");
        let e = check(&[Insn::GetGlobal { dst: 0, g: 0 }, Insn::Ret { src: 0 }], 0, 1).unwrap_err();
        assert!(e.contains("global"), "{e}");
        let e = check(&[Insn::LoadStr { dst: 0, s: 0 }, Insn::Ret { src: 0 }], 0, 1).unwrap_err();
        assert!(e.contains("string"), "{e}");
    }

    #[test]
    fn checks_stack_maps() {
        let insns = [Insn::NewArray { dst: 0, base: 0, n: 0, desc: 0 }, Insn::Ret { src: 0 }];
        let mut p = prog(chunk_of(&insns, 0, 1));
        p.descs = vec![crate::ir::TypeDesc::Unit];
        verify_program(&p).unwrap();
        p.chunks[0].stack_maps.clear();
        assert!(verify_program(&p).unwrap_err().contains("no stack map"));
        p.chunks[0].stack_maps = vec![StackMap { pc: 1, live_roots: BitSet::new() }];
        assert!(verify_program(&p).is_err());
        p.chunks[0].stack_maps = vec![StackMap { pc: 0, live_roots: [9u8].into_iter().collect() }];
        assert!(verify_program(&p).unwrap_err().contains("outside the frame"));
    }

    #[test]
    fn calls_clobber_the_registers_above_their_window() {
        // r2 is written before the call (above the window at r1), then read after it
        let callee = chunk_of(&[Insn::Ret { src: 0 }], 1, 1);
        let caller = chunk_of(
            &[
                Insn::LoadI { dst: 1, imm: 1 },
                Insn::LoadI { dst: 2, imm: 2 },
                Insn::Call { base: 1, func: 0, nargs: 1 },
                add(0, 1, 2),
                Insn::Ret { src: 0 },
            ],
            0,
            3,
        );
        let p = BytecodeProgram { chunks: vec![callee, caller], entry: 1, ..Default::default() };
        let e = verify_program(&p).unwrap_err();
        assert!(e.contains("r2"), "{e}");
    }
}
