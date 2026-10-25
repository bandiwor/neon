//! Textual form of the IR, for tests and debugging.

use std::fmt::{self, Write};

use super::types::*;

fn regs(rs: &[VReg]) -> String {
    rs.iter().map(|r| format!("v{}", r.0)).collect::<Vec<_>>().join(", ")
}

fn ty_name(t: IrTy) -> String {
    match t {
        IrTy::Num(n) => n.name().to_string(),
        IrTy::Bool => "bool".into(),
        IrTy::Char => "char".into(),
        IrTy::Ref => "ref".into(),
        IrTy::Unit => "unit".into(),
    }
}

impl fmt::Display for Const {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Const::Bits(b) => write!(f, "{b:#x}"),
            Const::Str(i) => write!(f, "str#{i}"),
            Const::Unit => write!(f, "()"),
        }
    }
}

impl fmt::Display for Inst {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use Inst::*;
        match self {
            Const { dst, val } => write!(f, "v{} = const {val}", dst.0),
            Move { dst, src } => write!(f, "v{} = v{}", dst.0, src.0),
            Arith { dst, op, ty, lhs, rhs } => {
                write!(f, "v{} = {}.{} v{}, v{}", dst.0, op.name(), ty.name(), lhs.0, rhs.0)
            }
            Cmp { dst, op, ty, lhs, rhs } => {
                let t = match ty {
                    CmpTy::Num(n) => n.name(),
                    CmpTy::Str => "str",
                };
                write!(f, "v{} = {}.{} v{}, v{}", dst.0, op.name(), t, lhs.0, rhs.0)
            }
            Neg { dst, ty, src } => write!(f, "v{} = neg.{} v{}", dst.0, ty.name(), src.0),
            BitNot { dst, ty, src } => write!(f, "v{} = bitnot.{} v{}", dst.0, ty.name(), src.0),
            BoolNot { dst, src } => write!(f, "v{} = not v{}", dst.0, src.0),
            Cast { dst, from, to, src } => write!(f, "v{} = cast {}->{} v{}", dst.0, from.name(), to.name(), src.0),
            NewStruct { dst, desc, fields } => write!(f, "v{} = new_struct d{} [{}]", dst.0, desc.0, regs(fields)),
            GetField { dst, obj, idx } => write!(f, "v{} = v{}.{idx}", dst.0, obj.0),
            SetField { obj, idx, src } => write!(f, "v{}.{idx} = v{}", obj.0, src.0),
            NewEnum { dst, desc, tag, fields } => {
                write!(f, "v{} = new_enum d{} #{tag} [{}]", dst.0, desc.0, regs(fields))
            }
            EnumTag { dst, obj } => write!(f, "v{} = tag v{}", dst.0, obj.0),
            EnumField { dst, obj, idx } => write!(f, "v{} = v{}.payload{idx}", dst.0, obj.0),
            NewArray { dst, desc, elems } => write!(f, "v{} = new_array d{} [{}]", dst.0, desc.0, regs(elems)),
            ArrayGet { dst, arr, idx, idx_ty } => write!(f, "v{} = v{}[v{}:{}]", dst.0, arr.0, idx.0, idx_ty.name()),
            ArraySet { arr, idx, idx_ty, src } => write!(f, "v{}[v{}:{}] = v{}", arr.0, idx.0, idx_ty.name(), src.0),
            NewMap { dst, desc, entries } => {
                let e: Vec<_> = entries.iter().map(|(k, v)| format!("v{}: v{}", k.0, v.0)).collect();
                write!(f, "v{} = new_map d{} {{{}}}", dst.0, desc.0, e.join(", "))
            }
            MapGet { dst, map, key } => write!(f, "v{} = map_get v{}[v{}]", dst.0, map.0, key.0),
            MapSet { map, key, val } => write!(f, "map_set v{}[v{}] = v{}", map.0, key.0, val.0),
            NewSet { dst, desc, elems } => write!(f, "v{} = new_set d{} {{{}}}", dst.0, desc.0, regs(elems)),
            Call { dst, func, args } => write!(f, "v{} = call f{}({})", dst.0, func.0, regs(args)),
            CallClosure { dst, callee, args } => write!(f, "v{} = call_closure v{}({})", dst.0, callee.0, regs(args)),
            MakeClosure { dst, func, captures } => {
                let c: Vec<_> = captures
                    .iter()
                    .map(|c| match c {
                        Capture::Local(r) => format!("v{}", r.0),
                        Capture::Upvalue(i) => format!("up{i}"),
                    })
                    .collect();
                write!(f, "v{} = closure f{} [{}]", dst.0, func.0, c.join(", "))
            }
            GetUpval { dst, idx } => write!(f, "v{} = up{idx}", dst.0),
            SetUpval { idx, src } => write!(f, "up{idx} = v{}", src.0),
            CloseUpvals { regs: r } => write!(f, "close [{}]", regs(r)),
            GetGlobal { dst, slot } => write!(f, "v{} = g{}", dst.0, slot.0),
            SetGlobal { slot, src } => write!(f, "g{} = v{}", slot.0, src.0),
            Intrinsic { dst, b, desc, args } => {
                let d = desc.map(|d| format!("<d{}>", d.0)).unwrap_or_default();
                write!(f, "v{} = intrinsic {:?}{d}({})", dst.0, b, regs(args))
            }
        }
    }
}

impl fmt::Display for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Term::Jump(b) => write!(f, "jump b{}", b.0),
            Term::Branch { cond, then, els } => write!(f, "branch v{} ? b{} : b{}", cond.0, then.0, els.0),
            Term::BranchTag { obj, tag, then, els } => {
                write!(f, "branch_tag v{} == #{tag} ? b{} : b{}", obj.0, then.0, els.0)
            }
            Term::Return(r) => write!(f, "return v{}", r.0),
            Term::Unreachable => write!(f, "unreachable"),
        }
    }
}

impl fmt::Display for IrFunc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let params: Vec<_> = (0..self.params).map(|i| format!("v{i}: {}", ty_name(self.regs[i as usize]))).collect();
        writeln!(f, "fn {}({}) -> {} {{", self.name, params.join(", "), ty_name(self.ret))?;
        for (i, b) in self.blocks.iter().enumerate() {
            writeln!(f, "  b{i}:")?;
            for inst in &b.insts {
                writeln!(f, "    {inst}")?;
            }
            writeln!(f, "    {}", b.term)?;
        }
        write!(f, "}}")
    }
}

impl fmt::Display for IrProgram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, func) in self.funcs.iter().enumerate() {
            let mut s = String::new();
            write!(s, "{func}")?;
            writeln!(f, "f{i}: {s}")?;
        }
        Ok(())
    }
}
