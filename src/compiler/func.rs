//! Translation of one IR function into bytecode instructions.

use crate::fx::FxHashMap as HashMap;

use super::regalloc::Allocation;
use super::CompileError;
use crate::builtin::Builtin;
use crate::bytecode::opcode::CmpKind;
use crate::bytecode::{Capture, Insn, Item, Label};
use crate::ir::{self, BlockId, Const, DescId, Inst, IrFunc, Term, VReg};
use crate::runtime::numeric::{ArithOp, CmpOp, NumTy};

/// The program-wide pool of 64-bit constants.
#[derive(Debug, Default)]
pub struct ConstPool {
    pub values: Vec<u64>,
    index: HashMap<u64, u32>,
}

impl ConstPool {
    pub fn intern(&mut self, bits: u64) -> u32 {
        *self.index.entry(bits).or_insert_with(|| {
            self.values.push(bits);
            self.values.len() as u32 - 1
        })
    }
}

/// How many fields one `new*` instruction can take at most; larger literals
/// are built incrementally.
const MAX_WINDOW: usize = 200;

pub struct FuncEmitter<'a> {
    f: &'a IrFunc,
    alloc: &'a Allocation,
    consts: &'a mut ConstPool,
    pub items: Vec<Item>,
    /// Linear position of the instruction being emitted (matches the allocator's numbering).
    pos: u32,
    /// The block emitted after the current one, for fall-through decisions.
    next_block: Option<BlockId>,
}

fn block_label(b: BlockId) -> Label {
    Label(b.0)
}

impl<'a> FuncEmitter<'a> {
    pub fn new(f: &'a IrFunc, alloc: &'a Allocation, consts: &'a mut ConstPool) -> Self {
        FuncEmitter { f, alloc, consts, items: Vec::new(), pos: 0, next_block: None }
    }

    fn err(&self, what: &str) -> CompileError {
        CompileError::new(&self.f.name, what)
    }

    fn s(&self, v: VReg) -> u8 {
        self.alloc.slot(v)
    }

    fn is_ref(&self, v: VReg) -> bool {
        self.f.reg_ty(v).is_ref()
    }

    fn push(&mut self, insn: Insn) {
        self.items.push(Item::new(insn));
    }

    /// Pushes an instruction that defines `dst`, noting whether it holds a reference.
    fn push_def(&mut self, insn: Insn, dst: VReg) {
        let r = self.is_ref(dst);
        self.items.push(Item::with_ref(insn, r));
    }

    fn u8_of(&self, v: u32, what: &str) -> Result<u8, CompileError> {
        u8::try_from(v).map_err(|_| self.err(&format!("{what} {v} does not fit in 8 bits")))
    }

    fn desc16(&self, d: DescId) -> Result<u16, CompileError> {
        if d.0 >= 0x7FFF {
            return Err(self.err("too many type descriptors"));
        }
        Ok(d.0 as u16)
    }

    pub fn emit_function(&mut self) -> Result<(), CompileError> {
        let order = self.alloc.order.clone();
        for (i, &b) in order.iter().enumerate() {
            self.next_block = order.get(i + 1).copied();
            self.pos = self.alloc.block_start[b.index()];
            self.push(Insn::Label(block_label(b)));
            let blk = &self.f.blocks[b.index()];
            for inst in &blk.insts {
                self.emit_inst(inst)?;
                self.pos += 2;
            }
            self.emit_term(&blk.term)?;
        }
        Ok(())
    }

    // ---- terminators ---------------------------------------------------------------------------

    fn emit_term(&mut self, t: &Term) -> Result<(), CompileError> {
        match t {
            Term::Jump(b) => {
                if self.next_block != Some(*b) {
                    self.push(Insn::Jmp { to: block_label(*b) });
                }
            }
            Term::Branch { cond, then, els } => {
                let cond = self.s(*cond);
                if self.next_block == Some(*then) {
                    self.push(Insn::JmpIfNot { cond, to: block_label(*els) });
                } else if self.next_block == Some(*els) {
                    self.push(Insn::JmpIf { cond, to: block_label(*then) });
                } else {
                    self.push(Insn::JmpIfNot { cond, to: block_label(*els) });
                    self.push(Insn::Jmp { to: block_label(*then) });
                }
            }
            Term::BranchTag { obj, tag, then, els } => {
                let tag = self.u8_of(*tag, "enum variant number")?;
                let obj = self.s(*obj);
                self.push(Insn::MatchTagJmp { obj, tag, to: block_label(*els) });
                if self.next_block != Some(*then) {
                    self.push(Insn::Jmp { to: block_label(*then) });
                }
            }
            Term::Return(v) => {
                let src = self.s(*v);
                self.push(Insn::Ret { src });
            }
            Term::Unreachable => self.push(Insn::Unreachable),
        }
        Ok(())
    }

    // ---- argument windows --------------------------------------------------------------------------

    /// Makes `srcs` available in consecutive registers and returns the first.
    /// `clobbers` says the instruction lets a callee overwrite everything from
    /// the window upwards, so the window must sit above all live values.
    fn window(&mut self, srcs: &[VReg], clobbers: bool) -> Result<u8, CompileError> {
        let floor = self.alloc.floor_at(self.pos) as usize;
        let slots: Vec<usize> = srcs.iter().map(|&v| self.s(v) as usize).collect();
        if slots.is_empty() {
            return Ok(if clobbers { self.reg_checked(floor, 1)? } else { 0 });
        }
        let consecutive = slots.windows(2).all(|w| w[1] == w[0] + 1);
        if consecutive && (!clobbers || slots[0] >= floor) {
            return Ok(slots[0] as u8);
        }
        let base = floor;
        let n = srcs.len();
        let base_reg = self.reg_checked(base, n)?;
        let mut moves: Vec<(usize, usize, bool)> =
            srcs.iter().enumerate().map(|(i, &v)| (base + i, slots[i], self.is_ref(v))).collect();
        moves.retain(|m| m.0 != m.1);
        let temp = moves.iter().flat_map(|m| [m.0, m.1]).chain([base + n]).max().unwrap() + 1;
        while !moves.is_empty() {
            let free = (0..moves.len()).find(|&i| !moves.iter().enumerate().any(|(j, m)| j != i && m.1 == moves[i].0));
            match free {
                Some(i) => {
                    let (d, s, r) = moves.remove(i);
                    self.items.push(Item::with_ref(Insn::Move { dst: d as u8, src: s as u8 }, r));
                }
                None => {
                    // every pending destination is still needed as a source: a cycle.
                    // Park the first destination's old value in a scratch register.
                    let tmp = self.reg_checked(temp, 1)?;
                    let d = moves[0].0;
                    let r = moves.iter().find(|m| m.1 == d).map(|m| m.2).unwrap_or(false);
                    self.items.push(Item::with_ref(Insn::Move { dst: tmp, src: d as u8 }, r));
                    for m in moves.iter_mut() {
                        if m.1 == d {
                            m.1 = temp;
                        }
                    }
                }
            }
        }
        Ok(base_reg)
    }

    /// Checks that registers `from..from+count` exist and returns `from` as a register.
    fn reg_checked(&self, from: usize, count: usize) -> Result<u8, CompileError> {
        if from + count > 256 {
            return Err(self.err("needs more than 256 registers (call or literal window too high)"));
        }
        Ok(from as u8)
    }

    // ---- instructions --------------------------------------------------------------------------------------

    fn emit_inst(&mut self, inst: &Inst) -> Result<(), CompileError> {
        use Inst as I;
        match inst {
            I::Const { dst, val } => {
                let d = self.s(*dst);
                match *val {
                    Const::Bits(b) => {
                        if let Ok(imm) = i16::try_from(b as i64) {
                            self.push_def(Insn::LoadI { dst: d, imm }, *dst);
                        } else {
                            let k = self.consts.intern(b);
                            self.push_def(Insn::LoadK { dst: d, k }, *dst);
                        }
                    }
                    Const::Str(s) => self.push_def(Insn::LoadStr { dst: d, s }, *dst),
                    Const::Unit => self.push_def(Insn::LoadI { dst: d, imm: 0 }, *dst),
                }
            }
            I::Move { dst, src } => {
                let (d, s) = (self.s(*dst), self.s(*src));
                if d != s {
                    self.push_def(Insn::Move { dst: d, src: s }, *dst);
                }
            }
            I::Arith { dst, op, ty, lhs, rhs } => {
                let (d, a, b) = (self.s(*dst), self.s(*lhs), self.s(*rhs));
                let insn = if matches!(op, ArithOp::And | ArithOp::Or | ArithOp::Xor) {
                    Insn::Bitwise { op: *op, dst: d, a, b }
                } else {
                    Insn::Arith { op: *op, ty: *ty, dst: d, a, b }
                };
                self.push_def(insn, *dst);
            }
            I::Cmp { dst, op, ty, lhs, rhs } => {
                let (kind, swap) = cmp_kind(*op, *ty);
                let (mut a, mut b) = (self.s(*lhs), self.s(*rhs));
                if swap {
                    std::mem::swap(&mut a, &mut b);
                }
                self.push_def(Insn::Cmp { kind, dst: self.s(*dst), a, b }, *dst);
            }
            I::Neg { dst, ty, src } => self.push_def(Insn::Neg { ty: *ty, dst: self.s(*dst), src: self.s(*src) }, *dst),
            I::BitNot { dst, ty, src } => {
                self.push_def(Insn::BitNot { ty: *ty, dst: self.s(*dst), src: self.s(*src) }, *dst)
            }
            I::BoolNot { dst, src } => self.push_def(Insn::BoolNot { dst: self.s(*dst), src: self.s(*src) }, *dst),
            I::Cast { dst, from, to, src } => {
                self.push_def(Insn::Cast { from: *from, to: *to, dst: self.s(*dst), src: self.s(*src) }, *dst)
            }

            I::NewStruct { dst, desc, fields } => {
                let n = self.u8_of(fields.len() as u32, "struct field count")?;
                let desc = self.desc16(*desc)?;
                let base = self.window(fields, false)?;
                self.push_def(Insn::NewStruct { dst: self.s(*dst), base, n, desc }, *dst);
            }
            I::GetField { dst, obj, idx } => {
                let idx = self.u8_of(*idx, "field index")?;
                self.push_def(Insn::GetField { dst: self.s(*dst), obj: self.s(*obj), idx }, *dst);
            }
            I::SetField { obj, idx, src } => {
                let idx = self.u8_of(*idx, "field index")?;
                self.push(Insn::SetField { obj: self.s(*obj), idx, src: self.s(*src) });
            }
            I::NewEnum { dst, desc, tag, fields } => {
                let n = self.u8_of(fields.len() as u32, "variant field count")?;
                let tag = self.u8_of(*tag, "enum variant number")?;
                let desc = self.desc16(*desc)?;
                let base = self.window(fields, false)?;
                self.push_def(Insn::NewEnum { dst: self.s(*dst), base, n, desc, tag }, *dst);
            }
            I::EnumTag { dst, obj } => self.push_def(Insn::EnumTag { dst: self.s(*dst), obj: self.s(*obj) }, *dst),
            I::EnumField { dst, obj, idx } => {
                let idx = self.u8_of(*idx, "variant field index")?;
                self.push_def(Insn::EnumField { dst: self.s(*dst), obj: self.s(*obj), idx }, *dst);
            }
            I::NewArray { dst, desc, elems } => self.emit_new_collection(*dst, *desc, elems, Builtin::ArrPush)?,
            I::NewSet { dst, desc, elems } => self.emit_new_collection(*dst, *desc, elems, Builtin::SetInsert)?,
            I::ArrayGet { dst, arr, idx, .. } => {
                self.push_def(Insn::ArrayGet { dst: self.s(*dst), arr: self.s(*arr), idx: self.s(*idx) }, *dst)
            }
            I::ArraySet { arr, idx, src, .. } => {
                self.push(Insn::ArraySet { arr: self.s(*arr), idx: self.s(*idx), src: self.s(*src) })
            }
            I::NewMap { dst, desc, entries } => {
                let desc16 = self.desc16(*desc)?;
                let first = entries.len().min(MAX_WINDOW / 2);
                let flat: Vec<VReg> = entries[..first].iter().flat_map(|&(k, v)| [k, v]).collect();
                let base = self.window(&flat, false)?;
                self.push_def(Insn::NewMap { dst: self.s(*dst), base, pairs: first as u8, desc: desc16 }, *dst);
                for &(k, v) in &entries[first..] {
                    self.push(Insn::MapSet { map: self.s(*dst), key: self.s(k), val: self.s(v) });
                }
            }
            I::MapGet { dst, map, key } => {
                self.push_def(Insn::MapGet { dst: self.s(*dst), map: self.s(*map), key: self.s(*key) }, *dst)
            }
            I::MapSet { map, key, val } => {
                self.push(Insn::MapSet { map: self.s(*map), key: self.s(*key), val: self.s(*val) })
            }

            I::Call { dst, func, args } => {
                if args.len() > 250 {
                    return Err(self.err("call with more than 250 arguments"));
                }
                let base = self.window(args, true)?;
                self.push_def(Insn::Call { base, func: func.0, nargs: args.len() as u8 }, *dst);
                self.move_result(*dst, base);
            }
            I::CallClosure { dst, callee, args } => {
                if args.len() > 250 {
                    return Err(self.err("call with more than 250 arguments"));
                }
                let mut srcs = vec![*callee];
                srcs.extend_from_slice(args);
                let base = self.window(&srcs, true)?;
                self.push_def(Insn::CallClosure { base, nargs: args.len() as u8 }, *dst);
                self.move_result(*dst, base);
            }
            I::MakeClosure { dst, func, captures } => {
                let caps = captures
                    .iter()
                    .map(|c| match *c {
                        ir::Capture::Local(r) => Ok(Capture::Local(self.s(r))),
                        ir::Capture::Upvalue(i) => Ok(Capture::Upvalue(self.u8_of(i, "upvalue index")?)),
                    })
                    .collect::<Result<Vec<_>, CompileError>>()?;
                self.push_def(Insn::MakeClosure { dst: self.s(*dst), func: func.0, captures: caps }, *dst);
            }
            I::GetUpval { dst, idx } => {
                let idx = self.u8_of(*idx, "upvalue index")?;
                self.push_def(Insn::GetUpval { dst: self.s(*dst), idx }, *dst);
            }
            I::SetUpval { idx, src } => {
                let idx = self.u8_of(*idx, "upvalue index")?;
                self.push(Insn::SetUpval { idx, src: self.s(*src) });
            }
            I::CloseUpvals { regs } => {
                for &r in regs {
                    self.push(Insn::Close { reg: self.s(r) });
                }
            }
            I::GetGlobal { dst, slot } => {
                let g = self.global16(slot.0)?;
                self.push_def(Insn::GetGlobal { dst: self.s(*dst), g }, *dst);
            }
            I::SetGlobal { slot, src } => {
                let g = self.global16(slot.0)?;
                self.push(Insn::SetGlobal { src: self.s(*src), g });
            }
            I::Intrinsic { dst, b, desc, args } => {
                if args.len() > 250 {
                    return Err(self.err("intrinsic with more than 250 arguments"));
                }
                let desc = desc.map(|d| self.desc16(d)).transpose()?;
                let base = self.window(args, false)?;
                self.push_def(Insn::Intrinsic { dst: self.s(*dst), base, nargs: args.len() as u8, b: *b, desc }, *dst);
            }
        }
        Ok(())
    }

    fn global16(&self, g: u32) -> Result<u16, CompileError> {
        u16::try_from(g).map_err(|_| self.err("too many global variables"))
    }

    /// A call leaves its result in the window base; copy it where the IR wants it.
    fn move_result(&mut self, dst: VReg, base: u8) {
        let d = self.s(dst);
        if d != base {
            self.push_def(Insn::Move { dst: d, src: base }, dst);
        }
    }

    /// Array and set literals: up to [`MAX_WINDOW`] elements go straight into the
    /// `new` instruction, the rest are appended one by one.
    fn emit_new_collection(
        &mut self,
        dst: VReg,
        desc: DescId,
        elems: &[VReg],
        append: Builtin,
    ) -> Result<(), CompileError> {
        let desc16 = self.desc16(desc)?;
        let first = elems.len().min(MAX_WINDOW);
        let base = self.window(&elems[..first], false)?;
        let d = self.s(dst);
        let n = first as u8;
        let insn = if append == Builtin::ArrPush {
            Insn::NewArray { dst: d, base, n, desc: desc16 }
        } else {
            Insn::NewSet { dst: d, base, n, desc: desc16 }
        };
        self.push_def(insn, dst);
        for &e in &elems[first..] {
            let base = self.window(&[dst, e], false)?;
            let tmp_dst = self.reg_checked(base as usize, 2)?;
            // the appending builtins return unit; its result goes into the scratch slot
            self.push(Insn::Intrinsic { dst: tmp_dst, base, nargs: 2, b: append, desc: Some(desc16) });
        }
        Ok(())
    }
}

/// Maps an IR comparison to a bytecode one; `true` means "swap the operands"
/// (`a > b` is `b < a`).
pub fn cmp_kind(op: CmpOp, ty: ir::CmpTy) -> (CmpKind, bool) {
    let (base, swap) = match op {
        CmpOp::Eq => (CmpOp::Eq, false),
        CmpOp::Ne => (CmpOp::Ne, false),
        CmpOp::Lt => (CmpOp::Lt, false),
        CmpOp::Le => (CmpOp::Le, false),
        CmpOp::Gt => (CmpOp::Lt, true),
        CmpOp::Ge => (CmpOp::Le, true),
    };
    use CmpKind as K;
    let pick = |eq, ne, lt, le| match base {
        CmpOp::Eq => eq,
        CmpOp::Ne => ne,
        CmpOp::Lt => lt,
        _ => le,
    };
    let kind = match ty {
        ir::CmpTy::Str => pick(K::EqStr, K::NeStr, K::LtStr, K::LeStr),
        ir::CmpTy::Num(NumTy::F64) => pick(K::EqF64, K::NeF64, K::LtF64, K::LeF64),
        ir::CmpTy::Num(NumTy::F32) => pick(K::EqF32, K::NeF32, K::LtF32, K::LeF32),
        ir::CmpTy::Num(n) if n.is_signed() => pick(K::Eq, K::Ne, K::LtS, K::LeS),
        ir::CmpTy::Num(_) => pick(K::Eq, K::Ne, K::LtU, K::LeU),
    };
    (kind, swap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::regalloc::allocate;
    use crate::ir::{Block, FuncId, IrTy};

    fn emit(f: &IrFunc) -> Vec<Insn> {
        let alloc = allocate(f).unwrap();
        let mut consts = ConstPool::default();
        let mut em = FuncEmitter::new(f, &alloc, &mut consts);
        em.emit_function().unwrap();
        em.items.into_iter().map(|i| i.insn).collect()
    }

    fn func(params: u32, nregs: usize, insts: Vec<Inst>, ret: u32) -> IrFunc {
        let mut f = IrFunc::new("t");
        f.params = params;
        f.regs = vec![IrTy::Num(NumTy::I64); nregs];
        f.pinned = vec![false; nregs];
        f.blocks = vec![Block { insts, term: Term::Return(VReg(ret)) }];
        f
    }

    /// Runs the `move`s up to the first call and returns the registers.
    fn run_moves(insns: &[Insn], init: &[u64]) -> (Vec<u64>, Insn) {
        let mut r = vec![0u64; 16];
        r[..init.len()].copy_from_slice(init);
        for i in insns {
            match i {
                Insn::Move { dst, src } => r[*dst as usize] = r[*src as usize],
                Insn::Label(_) => {}
                other => return (r, other.clone()),
            }
        }
        panic!("no call");
    }

    #[test]
    fn comparison_mapping() {
        use crate::ir::CmpTy;
        let c = |op, ty| cmp_kind(op, ty);
        assert_eq!(c(CmpOp::Lt, CmpTy::Num(NumTy::I32)), (CmpKind::LtS, false));
        assert_eq!(c(CmpOp::Gt, CmpTy::Num(NumTy::I32)), (CmpKind::LtS, true));
        assert_eq!(c(CmpOp::Ge, CmpTy::Num(NumTy::U8)), (CmpKind::LeU, true));
        assert_eq!(c(CmpOp::Ne, CmpTy::Num(NumTy::U64)), (CmpKind::Ne, false));
        assert_eq!(c(CmpOp::Le, CmpTy::Num(NumTy::F32)), (CmpKind::LeF32, false));
        assert_eq!(c(CmpOp::Gt, CmpTy::Num(NumTy::F64)), (CmpKind::LtF64, true));
        assert_eq!(c(CmpOp::Eq, CmpTy::Str), (CmpKind::EqStr, false));
        assert_eq!(c(CmpOp::Ge, CmpTy::Str), (CmpKind::LeStr, true));
    }

    #[test]
    fn swapped_arguments_are_moved_through_a_scratch_register() {
        // g(b, a): the window needs r0 <- r1 and r1 <- r0, a cycle
        let call = Inst::Call { dst: VReg(2), func: FuncId(0), args: vec![VReg(1), VReg(0)] };
        let insns = emit(&func(2, 3, vec![call], 2));
        let (regs, call) = run_moves(&insns, &[10, 20]);
        let Insn::Call { base, .. } = call else { panic!("{call:?}") };
        assert_eq!((regs[base as usize], regs[base as usize + 1]), (20, 10), "{insns:?}");
    }

    #[test]
    fn rotated_arguments_resolve_longer_cycles() {
        // g(c, a, b) from params a=r0, b=r1, c=r2
        let call = Inst::Call { dst: VReg(3), func: FuncId(0), args: vec![VReg(2), VReg(0), VReg(1)] };
        let insns = emit(&func(3, 4, vec![call], 3));
        let (regs, call) = run_moves(&insns, &[1, 2, 3]);
        let Insn::Call { base, .. } = call else { panic!() };
        assert_eq!(&regs[base as usize..base as usize + 3], &[3, 1, 2], "{insns:?}");
    }

    #[test]
    fn arguments_already_in_place_need_no_moves() {
        let call = Inst::Call { dst: VReg(2), func: FuncId(0), args: vec![VReg(0), VReg(1)] };
        let insns = emit(&func(2, 3, vec![call], 2));
        assert!(matches!(insns[1], Insn::Call { base: 0, nargs: 2, .. }), "{insns:?}");
    }

    #[test]
    fn duplicated_arguments_are_copied() {
        let call = Inst::Call { dst: VReg(1), func: FuncId(0), args: vec![VReg(0), VReg(0)] };
        let insns = emit(&func(1, 2, vec![call], 1));
        let (regs, call) = run_moves(&insns, &[7]);
        let Insn::Call { base, .. } = call else { panic!() };
        assert_eq!(&regs[base as usize..base as usize + 2], &[7, 7], "{insns:?}");
    }

    #[test]
    fn small_and_large_constants() {
        let insts = vec![
            Inst::Const { dst: VReg(0), val: Const::Bits(5) },
            Inst::Const { dst: VReg(1), val: Const::Bits(-3i64 as u64) },
            Inst::Const { dst: VReg(2), val: Const::Bits(40_000) },
            Inst::Const { dst: VReg(3), val: Const::Bits(u64::MAX - 1) },
            Inst::Const { dst: VReg(4), val: Const::Unit },
        ];
        let mut f = func(0, 5, insts, 0);
        f.blocks[0].insts.push(Inst::Arith {
            dst: VReg(0),
            op: ArithOp::Add,
            ty: NumTy::I64,
            lhs: VReg(0),
            rhs: VReg(1),
        });
        f.blocks[0].insts.push(Inst::Arith {
            dst: VReg(0),
            op: ArithOp::Add,
            ty: NumTy::I64,
            lhs: VReg(2),
            rhs: VReg(3),
        });
        f.blocks[0].insts.push(Inst::Move { dst: VReg(0), src: VReg(4) });
        let insns = emit(&f);
        assert!(matches!(insns[1], Insn::LoadI { imm: 5, .. }));
        assert!(matches!(insns[2], Insn::LoadI { imm: -3, .. }));
        assert!(matches!(insns[3], Insn::LoadK { k: 0, .. }), "40000 does not fit in an i16");
        // u64::MAX - 1 is -2 as an i64, which fits the immediate form
        assert!(matches!(insns[4], Insn::LoadI { imm: -2, .. }));
    }

    #[test]
    fn jumps_to_the_next_block_disappear() {
        let mut f = IrFunc::new("t");
        f.params = 1;
        f.regs = vec![IrTy::Bool];
        f.pinned = vec![false];
        let b0 = f.new_block();
        let (b1, b2, b3) = (f.new_block(), f.new_block(), f.new_block());
        f.blocks[b0.index()].term = Term::Branch { cond: VReg(0), then: b1, els: b2 };
        f.blocks[b1.index()].term = Term::Jump(b3);
        f.blocks[b2.index()].term = Term::Jump(b3);
        f.blocks[b3.index()].term = Term::Return(VReg(0));
        let insns = emit(&f);
        let jumps = insns.iter().filter(|i| matches!(i, Insn::Jmp { .. })).count();
        assert_eq!(jumps, 1, "only the else-branch needs an explicit jump: {insns:?}");
        assert!(matches!(insns[1], Insn::JmpIfNot { .. }));
    }
}
