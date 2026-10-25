//! IR well-formedness checks, run after lowering and after every pass in tests.

use super::types::*;

/// Returns a description of the first problem found, if any.
pub fn verify_program(p: &IrProgram) -> Result<(), String> {
    for (i, f) in p.funcs.iter().enumerate() {
        verify_func(p, f).map_err(|e| format!("f{i} `{}`: {e}", f.name))?;
    }
    if p.entry.index() >= p.funcs.len() {
        return Err("entry function out of range".into());
    }
    Ok(())
}

pub fn verify_func(p: &IrProgram, f: &IrFunc) -> Result<(), String> {
    if f.blocks.is_empty() {
        return Err("no blocks".into());
    }
    if f.pinned.len() != f.regs.len() {
        return Err("pinned/regs length mismatch".into());
    }
    if f.params as usize > f.regs.len() {
        return Err("more parameters than registers".into());
    }
    let nregs = f.regs.len() as u32;
    let nblocks = f.blocks.len() as u32;
    let check_reg = |r: VReg, what: &str| -> Result<(), String> {
        if r.0 >= nregs {
            Err(format!("{what}: v{} out of range ({} registers)", r.0, nregs))
        } else {
            Ok(())
        }
    };
    let mut defined = vec![false; f.regs.len()];
    for d in defined.iter_mut().take(f.params as usize) {
        *d = true;
    }
    for b in &f.blocks {
        for inst in &b.insts {
            if let Some(d) = inst.def() {
                check_reg(d, "def")?;
                defined[d.index()] = true;
            }
        }
    }
    for (bi, b) in f.blocks.iter().enumerate() {
        for inst in &b.insts {
            let mut err = None;
            inst.for_each_use(|r| {
                if r.0 >= nregs {
                    err = Some(format!("b{bi}: use v{} out of range", r.0));
                } else if !defined[r.index()] {
                    err = Some(format!("b{bi}: v{} is used but never defined", r.0));
                }
            });
            if let Some(e) = err {
                return Err(e);
            }
            match inst {
                Inst::Call { func, args, dst } => {
                    let callee =
                        p.funcs.get(func.index()).ok_or_else(|| format!("b{bi}: call of unknown f{}", func.0))?;
                    if callee.params as usize != args.len() {
                        return Err(format!(
                            "b{bi}: call of `{}` with {} args, expects {}",
                            callee.name,
                            args.len(),
                            callee.params
                        ));
                    }
                    for (a, i) in args.iter().zip(0..) {
                        if f.regs[a.index()] != callee.regs[i] {
                            return Err(format!(
                                "b{bi}: call of `{}`: arg {i} is {:?}, expected {:?}",
                                callee.name,
                                f.regs[a.index()],
                                callee.regs[i]
                            ));
                        }
                    }
                    if f.regs[dst.index()] != callee.ret {
                        return Err(format!(
                            "b{bi}: call of `{}`: result is {:?}, callee returns {:?}",
                            callee.name,
                            f.regs[dst.index()],
                            callee.ret
                        ));
                    }
                }
                Inst::MakeClosure { func, captures, .. } => {
                    let callee =
                        p.funcs.get(func.index()).ok_or_else(|| format!("b{bi}: closure of unknown f{}", func.0))?;
                    if callee.upvals.len() != captures.len() {
                        return Err(format!(
                            "b{bi}: closure of `{}` captures {}, expects {}",
                            callee.name,
                            captures.len(),
                            callee.upvals.len()
                        ));
                    }
                    for (c, ty) in captures.iter().zip(&callee.upvals) {
                        match c {
                            Capture::Local(r) => {
                                if !f.pinned[r.index()] {
                                    return Err(format!("b{bi}: captured v{} is not pinned", r.0));
                                }
                                if f.regs[r.index()] != *ty {
                                    return Err(format!("b{bi}: captured v{} has the wrong type", r.0));
                                }
                            }
                            Capture::Upvalue(i) => {
                                if f.upvals.get(*i as usize) != Some(ty) {
                                    return Err(format!("b{bi}: bad upvalue forwarding up{i}"));
                                }
                            }
                        }
                    }
                }
                Inst::GetUpval { idx, dst } => {
                    if f.upvals.get(*idx as usize) != Some(&f.regs[dst.index()]) {
                        return Err(format!("b{bi}: bad upvalue read up{idx}"));
                    }
                }
                Inst::SetUpval { idx, src } => {
                    if f.upvals.get(*idx as usize) != Some(&f.regs[src.index()]) {
                        return Err(format!("b{bi}: bad upvalue write up{idx}"));
                    }
                }
                Inst::GetGlobal { slot, dst } => {
                    if p.globals.get(slot.index()).map(|g| g.ty) != Some(f.regs[dst.index()]) {
                        return Err(format!("b{bi}: bad global read g{}", slot.0));
                    }
                }
                Inst::SetGlobal { slot, src } => {
                    if p.globals.get(slot.index()).map(|g| g.ty) != Some(f.regs[src.index()]) {
                        return Err(format!("b{bi}: bad global write g{}", slot.0));
                    }
                }
                Inst::Move { dst, src } => {
                    if f.regs[dst.index()] != f.regs[src.index()] {
                        return Err(format!(
                            "b{bi}: move v{} <- v{} changes type {:?} -> {:?}",
                            dst.0,
                            src.0,
                            f.regs[src.index()],
                            f.regs[dst.index()]
                        ));
                    }
                }
                Inst::NewStruct { desc, .. }
                | Inst::NewEnum { desc, .. }
                | Inst::NewArray { desc, .. }
                | Inst::NewMap { desc, .. }
                | Inst::NewSet { desc, .. } => {
                    if desc.index() >= p.descs.len() {
                        return Err(format!("b{bi}: descriptor d{} out of range", desc.0));
                    }
                }
                Inst::Const { val: Const::Str(i), .. } if *i as usize >= p.strings.len() => {
                    return Err(format!("b{bi}: string #{i} out of range"));
                }
                _ => {}
            }
        }
        let mut err = None;
        b.term.for_each_use(|r| {
            if r.0 >= nregs {
                err = Some(format!("b{bi}: terminator uses v{} out of range", r.0));
            }
        });
        if let Some(e) = err {
            return Err(e);
        }
        for s in b.term.successors() {
            if s.0 >= nblocks {
                return Err(format!("b{bi}: jump to missing b{}", s.0));
            }
        }
        if let Term::Return(r) = b.term {
            if f.regs[r.index()] != f.ret {
                return Err(format!("b{bi}: returns {:?}, function returns {:?}", f.regs[r.index()], f.ret));
            }
        }
    }
    Ok(())
}
