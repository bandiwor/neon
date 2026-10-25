use std::collections::{HashMap, HashSet};

use super::analysis::liveness;
use super::asm::{assemble, decode, decode_one};
use super::disasm::{disassemble_chunk, format_insn};
use super::insn::*;
use super::opcode::{CmpKind, Format, FusedKind, Opcode, EXT_ESCAPE};
use super::peephole::peephole;
use super::{Chunk, DecodeError};
use crate::builtin::Builtin;
use crate::gc::{BitSet, StackMap};
use crate::runtime::numeric::{self, ArithOp, CmpOp, NumTy};
use crate::runtime::Trap;

// ---- opcode table -------------------------------------------------------------------------------

#[test]
fn opcode_numbering_is_dense_and_stable() {
    assert!(Opcode::ALL.len() < EXT_ESCAPE as usize);
    for (i, op) in Opcode::ALL.iter().enumerate() {
        assert_eq!(*op as usize, i, "{op:?}");
        assert_eq!(Opcode::from_u8(i as u8), Some(*op));
    }
    assert_eq!(Opcode::from_u8(Opcode::ALL.len() as u8), None);
    assert_eq!(Opcode::from_u8(EXT_ESCAPE), None);
    // the first few are part of the format
    assert_eq!(Opcode::Move as u8, 0);
    assert_eq!(Opcode::LoadI as u8, 1);
}

#[test]
fn mnemonics_are_unique() {
    let mut seen = HashSet::new();
    for op in Opcode::ALL {
        assert!(seen.insert(op.mnemonic()), "duplicate mnemonic {}", op.mnemonic());
    }
}

#[test]
fn opcode_families_round_trip() {
    for &ty in &NumTy::ALL {
        let neg = Opcode::neg(ty);
        assert_eq!(neg.neg_ty(), Some(ty));
        for op in [ArithOp::Add, ArithOp::Sub, ArithOp::Mul, ArithOp::Div, ArithOp::Rem, ArithOp::Shl, ArithOp::Shr] {
            match Opcode::arith(op, ty) {
                Some(o) => assert_eq!(o.arith_parts(), Some((op, ty))),
                None => assert!(ty.is_float() && op.is_shift(), "{op:?} {ty:?}"),
            }
        }
        if ty.is_int() {
            assert_eq!(Opcode::bnot(ty).unwrap().bnot_ty(), Some(ty));
            assert_eq!(Opcode::addi(ty).unwrap().addi_ty(), Some(ty));
            assert_eq!(Opcode::subi(ty).unwrap().subi_ty(), Some(ty));
        } else {
            assert!(Opcode::bnot(ty).is_none() && Opcode::addi(ty).is_none());
        }
    }
    for op in Opcode::ALL {
        if let Some(k) = op.cmp_kind() {
            assert_eq!(Opcode::cmp(k), *op);
        }
        if let Some(f) = op.fused_kind() {
            assert_eq!(Opcode::fused(f), *op);
            assert_eq!(f.cmp().fused(), Some(f));
        }
    }
    assert_eq!(Opcode::bitwise(ArithOp::Add), None);
}

#[test]
fn formats_match_encoding_class() {
    assert_eq!(Opcode::Move.format(), Format::R3);
    assert_eq!(Opcode::LoadI.format(), Format::R2I);
    assert_eq!(Opcode::MatchTagJmp.format(), Format::Ext);
    assert_eq!(Opcode::fused(FusedKind::LtS).format(), Format::Ext);
    assert_eq!(Opcode::cmp(CmpKind::EqStr).format(), Format::R3);
}

// ---- exact word layouts ------------------------------------------------------------------------------

fn words(insns: &[Insn]) -> Vec<u32> {
    assemble(insns).unwrap().code
}

#[test]
fn r3_and_r2i_layouts() {
    let w = words(&[Insn::Arith { op: ArithOp::Add, ty: NumTy::I32, dst: 1, a: 2, b: 3 }]);
    assert_eq!(w, [((Opcode::arith(ArithOp::Add, NumTy::I32).unwrap() as u32) << 24) | (1 << 16) | (2 << 8) | 3]);
    let w = words(&[Insn::LoadI { dst: 7, imm: -2 }]);
    assert_eq!(w, [((Opcode::LoadI as u32) << 24) | (7 << 16) | 0xFFFE]);
    let w = words(&[Insn::Move { dst: 0xAB, src: 0xCD }]);
    assert_eq!(w, [0x00AB_CD00]);
}

#[test]
fn wide_layout_has_escape_and_extension_word() {
    let l = Label(0);
    let w = words(&[Insn::CmpJmp { kind: FusedKind::LtS, a: 4, b: 5, to: l }, Insn::Label(l)]);
    assert_eq!(w.len(), 2);
    assert_eq!(w[0], ((EXT_ESCAPE as u32) << 24) | ((Opcode::fused(FusedKind::LtS) as u32) << 16) | (4 << 8) | 5);
    assert_eq!(w[1], 0, "the target is the very next word");
    let w = words(&[Insn::MakeClosure { dst: 3, func: 9, captures: vec![Capture::Local(5), Capture::Upvalue(2)] }]);
    assert_eq!(w.len(), 4);
    assert_eq!(w[1], 9);
    assert_eq!(&w[2..], &[5, 0x102]);
}

#[test]
fn jump_offsets_are_relative_to_the_next_word() {
    let (a, b) = (Label(1), Label(2));
    // 0: jmp b ; 1: move ; 2: L b: ; and a backward jump
    let w = words(&[
        Insn::Label(a),
        Insn::Jmp { to: b },
        Insn::Move { dst: 0, src: 1 },
        Insn::Label(b),
        Insn::Jmp { to: a },
    ]);
    assert_eq!(w.len(), 3);
    assert_eq!(w[0] as u16 as i16, 1);
    assert_eq!(w[2] as u16 as i16, -3);
}

// ---- encode / decode round trips -----------------------------------------------------------------------

/// Re-expresses `insns` the way `decode(assemble(insns))` returns them.
fn normalize(insns: &[Insn]) -> Vec<Insn> {
    let asm = assemble(insns).unwrap();
    let mut label_pc: HashMap<Label, u32> = HashMap::new();
    for (i, insn) in insns.iter().enumerate() {
        if let Insn::Label(l) = insn {
            label_pc.insert(*l, asm.pcs[i]);
        }
    }
    let used: HashSet<u32> = insns.iter().filter_map(|i| i.target()).map(|l| label_pc[&l]).collect();
    let mut out: Vec<Insn> = Vec::new();
    for insn in insns {
        let mut insn = insn.clone();
        match &mut insn {
            Insn::Label(l) => {
                let pc = label_pc[l];
                if !used.contains(&pc) || out.last() == Some(&Insn::Label(Label(pc))) {
                    continue;
                }
                *l = Label(pc);
            }
            Insn::Call { nargs, .. } => *nargs = 0,
            _ => {}
        }
        if let Some(t) = insn.target_mut() {
            *t = Label(label_pc[t]);
        }
        out.push(insn);
    }
    out
}

fn roundtrip(insns: &[Insn]) {
    let asm = assemble(insns).unwrap();
    let back = decode(&asm.code).unwrap_or_else(|e| panic!("decode failed: {e}"));
    assert_eq!(back, normalize(insns));
}

fn every_variant() -> Vec<Insn> {
    let l = Label(0);
    vec![
        Insn::Label(l),
        Insn::Move { dst: 1, src: 2 },
        Insn::LoadI { dst: 1, imm: -300 },
        Insn::LoadK { dst: 2, k: 7 },
        Insn::LoadK { dst: 2, k: 70_000 },
        Insn::LoadStr { dst: 3, s: 9 },
        Insn::LoadStr { dst: 3, s: 100_000 },
        Insn::Arith { op: ArithOp::Mul, ty: NumTy::U16, dst: 1, a: 2, b: 3 },
        Insn::Arith { op: ArithOp::Rem, ty: NumTy::F64, dst: 1, a: 2, b: 3 },
        Insn::Arith { op: ArithOp::Shr, ty: NumTy::I8, dst: 1, a: 2, b: 3 },
        Insn::Bitwise { op: ArithOp::Xor, dst: 1, a: 2, b: 3 },
        Insn::AddImm { ty: NumTy::I64, dst: 1, src: 2, imm: -128 },
        Insn::SubImm { ty: NumTy::U8, dst: 1, src: 2, imm: 255 },
        Insn::Neg { ty: NumTy::F32, dst: 1, src: 2 },
        Insn::BitNot { ty: NumTy::U32, dst: 1, src: 2 },
        Insn::BoolNot { dst: 1, src: 2 },
        Insn::Cmp { kind: CmpKind::LeStr, dst: 1, a: 2, b: 3 },
        Insn::Cast { from: NumTy::F64, to: NumTy::I8, dst: 1, src: 2 },
        Insn::Jmp { to: l },
        Insn::JmpIf { cond: 4, to: l },
        Insn::JmpIfNot { cond: 4, to: l },
        Insn::CmpJmp { kind: FusedKind::NeF64, a: 1, b: 2, to: l },
        Insn::MatchTagJmp { obj: 3, tag: 200, to: l },
        Insn::Call { base: 5, func: 12, nargs: 2 },
        Insn::Call { base: 5, func: 123_456, nargs: 2 },
        Insn::CallClosure { base: 6, nargs: 3 },
        Insn::NewStruct { dst: 1, base: 2, n: 3, desc: 7 },
        Insn::GetField { dst: 1, obj: 2, idx: 3 },
        Insn::SetField { obj: 1, idx: 2, src: 3 },
        Insn::NewEnum { dst: 1, base: 2, n: 3, desc: 8, tag: 5 },
        Insn::EnumTag { dst: 1, obj: 2 },
        Insn::EnumField { dst: 1, obj: 2, idx: 3 },
        Insn::NewArray { dst: 1, base: 2, n: 3, desc: 9 },
        Insn::ArrayGet { dst: 1, arr: 2, idx: 3 },
        Insn::ArraySet { arr: 1, idx: 2, src: 3 },
        Insn::NewMap { dst: 1, base: 2, pairs: 3, desc: 10 },
        Insn::MapGet { dst: 1, map: 2, key: 3 },
        Insn::MapSet { map: 1, key: 2, val: 3 },
        Insn::NewSet { dst: 1, base: 2, n: 3, desc: 11 },
        Insn::MakeClosure { dst: 1, func: 4, captures: vec![] },
        Insn::MakeClosure {
            dst: 1,
            func: 4,
            captures: vec![Capture::Local(9), Capture::Upvalue(0), Capture::Local(255)],
        },
        Insn::GetUpval { dst: 1, idx: 2 },
        Insn::SetUpval { idx: 1, src: 2 },
        Insn::Close { reg: 5 },
        Insn::GetGlobal { dst: 1, g: 513 },
        Insn::SetGlobal { src: 1, g: 513 },
        Insn::Intrinsic { dst: 1, base: 2, nargs: 3, b: Builtin::Println, desc: Some(4) },
        Insn::Intrinsic { dst: 1, base: 2, nargs: 0, b: Builtin::Input, desc: None },
        Insn::Ret { src: 1 },
        Insn::Unreachable,
    ]
}

#[test]
fn every_instruction_round_trips() {
    let insns = every_variant();
    roundtrip(&insns);
    // and individually, to catch layout interference between neighbours
    for insn in &insns {
        if matches!(insn, Insn::Label(_)) || insn.target().is_some() {
            continue;
        }
        roundtrip(std::slice::from_ref(insn));
    }
}

#[test]
fn every_arithmetic_combination_round_trips() {
    let mut insns = Vec::new();
    for &ty in &NumTy::ALL {
        for op in [ArithOp::Add, ArithOp::Sub, ArithOp::Mul, ArithOp::Div, ArithOp::Rem, ArithOp::Shl, ArithOp::Shr] {
            if Opcode::arith(op, ty).is_some() {
                insns.push(Insn::Arith { op, ty, dst: 1, a: 2, b: 3 });
            }
        }
        insns.push(Insn::Neg { ty, dst: 1, src: 2 });
        if ty.is_int() {
            insns.push(Insn::BitNot { ty, dst: 1, src: 2 });
            insns.push(Insn::AddImm { ty, dst: 1, src: 2, imm: -5 });
            insns.push(Insn::SubImm { ty, dst: 1, src: 2, imm: 5 });
        }
        for &to in &NumTy::ALL {
            insns.push(Insn::Cast { from: ty, to, dst: 1, src: 2 });
        }
    }
    for op in [ArithOp::And, ArithOp::Or, ArithOp::Xor] {
        insns.push(Insn::Bitwise { op, dst: 1, a: 2, b: 3 });
    }
    for op in Opcode::ALL {
        if let Some(kind) = op.cmp_kind() {
            insns.push(Insn::Cmp { kind, dst: 1, a: 2, b: 3 });
        }
    }
    roundtrip(&insns);
}

#[test]
fn all_builtins_encode() {
    let insns: Vec<Insn> =
        Builtin::ALL.iter().map(|&b| Insn::Intrinsic { dst: 1, base: 2, nargs: 1, b, desc: Some(3) }).collect();
    roundtrip(&insns);
}

#[test]
fn far_jumps_are_widened() {
    let l = Label(0);
    let mut insns = vec![Insn::JmpIfNot { cond: 1, to: l }];
    insns.extend((0..40_000).map(|i| Insn::LoadI { dst: (i % 200) as u8, imm: 1 }));
    insns.push(Insn::Label(l));
    insns.push(Insn::Ret { src: 0 });
    let asm = assemble(&insns).unwrap();
    assert_eq!(asm.code.len(), 2 + 40_000 + 1, "the jump became a wide (2-word) instruction");
    assert_eq!(asm.code[0] >> 24, EXT_ESCAPE as u32);
    roundtrip(&insns);

    // a backward far jump too
    let mut back = vec![Insn::Label(l)];
    back.extend((0..40_000).map(|_| Insn::Move { dst: 0, src: 1 }));
    back.push(Insn::Jmp { to: l });
    let asm = assemble(&back).unwrap();
    assert_eq!(asm.code.len(), 40_000 + 2);
    roundtrip(&back);

    // jumps that fit stay short
    let near = vec![Insn::Jmp { to: l }, Insn::Label(l), Insn::Ret { src: 0 }];
    assert_eq!(assemble(&near).unwrap().code.len(), 2);
}

#[test]
fn relaxation_reaches_a_fixed_point() {
    // a jump right at the 16-bit limit whose widening pushes another jump out of range
    let (a, b) = (Label(0), Label(1));
    let mut insns = vec![Insn::Jmp { to: a }, Insn::Jmp { to: b }];
    insns.extend((0..32_766).map(|_| Insn::Move { dst: 0, src: 1 }));
    insns.push(Insn::Label(a));
    insns.push(Insn::Move { dst: 0, src: 2 });
    insns.push(Insn::Label(b));
    insns.push(Insn::Ret { src: 0 });
    roundtrip(&insns);
}

#[test]
fn assembly_errors() {
    use super::AsmError;
    assert_eq!(assemble(&[Insn::Jmp { to: Label(3) }]), Err(AsmError::UndefinedLabel(Label(3))));
    assert_eq!(assemble(&[Insn::Label(Label(1)), Insn::Label(Label(1))]), Err(AsmError::DuplicateLabel(Label(1))));
}

#[test]
fn decoding_rejects_garbage() {
    assert!(matches!(decode(&[0xFE00_0000]), Err(DecodeError::BadOpcode { .. })));
    assert!(
        matches!(
            decode(&[(EXT_ESCAPE as u32) << 24 | (Opcode::Move as u32) << 16]),
            Err(DecodeError::BadOpcode { .. })
        ),
        "an ext escape on a narrow opcode"
    );
    assert!(matches!(
        decode(&[((Opcode::JmpW as u32) << 16) | ((EXT_ESCAPE as u32) << 24)]),
        Err(DecodeError::Truncated { .. })
    ));
    assert!(
        matches!(decode(&[(Opcode::JmpW as u32) << 24]), Err(DecodeError::BadOpcode { .. })),
        "an ext opcode without the escape"
    );
    let bad_cast = (Opcode::Cast as u32) << 24 | 200;
    assert!(matches!(decode(&[bad_cast]), Err(DecodeError::BadOperand { .. })));
    assert_eq!(decode_one(&[0], 0).unwrap().len, 1);
}

// ---- randomized round trip -----------------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn reg(&mut self) -> Reg {
        self.below(256) as Reg
    }

    fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[self.below(xs.len() as u64) as usize]
    }
}

fn random_insn(r: &mut Rng, labels: u32) -> Insn {
    let label = Label(r.below(labels as u64) as u32);
    let ty = r.pick(&NumTy::ALL);
    let int = r.pick(&NumTy::INTS);
    match r.below(26) {
        0 => Insn::Move { dst: r.reg(), src: r.reg() },
        1 => Insn::LoadI { dst: r.reg(), imm: r.next() as i16 },
        2 => Insn::LoadK { dst: r.reg(), k: if r.below(2) == 0 { r.below(65536) as u32 } else { r.next() as u32 } },
        3 => Insn::LoadStr { dst: r.reg(), s: r.below(70_000) as u32 },
        4 => {
            let op = r.pick(&[ArithOp::Add, ArithOp::Sub, ArithOp::Mul, ArithOp::Div, ArithOp::Rem]);
            Insn::Arith { op, ty, dst: r.reg(), a: r.reg(), b: r.reg() }
        }
        5 => Insn::Arith { op: r.pick(&[ArithOp::Shl, ArithOp::Shr]), ty: int, dst: r.reg(), a: r.reg(), b: r.reg() },
        6 => Insn::Bitwise {
            op: r.pick(&[ArithOp::And, ArithOp::Or, ArithOp::Xor]),
            dst: r.reg(),
            a: r.reg(),
            b: r.reg(),
        },
        7 => Insn::AddImm { ty: int, dst: r.reg(), src: r.reg(), imm: r.next() as i8 },
        8 => Insn::SubImm { ty: int, dst: r.reg(), src: r.reg(), imm: r.next() as u8 },
        9 => Insn::Neg { ty, dst: r.reg(), src: r.reg() },
        10 => Insn::BitNot { ty: int, dst: r.reg(), src: r.reg() },
        11 => Insn::BoolNot { dst: r.reg(), src: r.reg() },
        12 => {
            let kinds: Vec<CmpKind> = Opcode::ALL.iter().filter_map(|o| o.cmp_kind()).collect();
            Insn::Cmp { kind: r.pick(&kinds), dst: r.reg(), a: r.reg(), b: r.reg() }
        }
        13 => Insn::Cast { from: ty, to: r.pick(&NumTy::ALL), dst: r.reg(), src: r.reg() },
        14 => Insn::Jmp { to: label },
        15 => Insn::JmpIf { cond: r.reg(), to: label },
        16 => Insn::JmpIfNot { cond: r.reg(), to: label },
        17 => {
            let kinds: Vec<FusedKind> = Opcode::ALL.iter().filter_map(|o| o.fused_kind()).collect();
            Insn::CmpJmp { kind: r.pick(&kinds), a: r.reg(), b: r.reg(), to: label }
        }
        18 => Insn::MatchTagJmp { obj: r.reg(), tag: r.next() as u8, to: label },
        19 => Insn::Call { base: r.reg(), func: r.below(200_000) as u32, nargs: r.below(4) as u8 },
        20 => Insn::NewEnum {
            dst: r.reg(),
            base: r.reg(),
            n: r.below(10) as u8,
            desc: r.next() as u16 & 0x7FFF,
            tag: r.next() as u8,
        },
        21 => {
            let n = r.below(5);
            let captures = (0..n)
                .map(|_| if r.below(2) == 0 { Capture::Local(r.reg()) } else { Capture::Upvalue(r.next() as u8) })
                .collect();
            Insn::MakeClosure { dst: r.reg(), func: r.next() as u32, captures }
        }
        22 => Insn::Intrinsic {
            dst: r.reg(),
            base: r.reg(),
            nargs: r.below(5) as u8,
            b: r.pick(Builtin::ALL),
            desc: if r.below(2) == 0 { None } else { Some(r.next() as u16 & 0x7FFF) },
        },
        23 => Insn::GetField { dst: r.reg(), obj: r.reg(), idx: r.reg() },
        24 => Insn::MapSet { map: r.reg(), key: r.reg(), val: r.reg() },
        _ => Insn::GetGlobal { dst: r.reg(), g: r.next() as u16 },
    }
}

#[test]
fn random_programs_round_trip() {
    let mut r = Rng(0x2545F4914F6CDD1D);
    for round in 0..300 {
        let labels = 1 + r.below(6) as u32;
        let n = 1 + r.below(60) as usize;
        let mut insns: Vec<Insn> = (0..n).map(|_| random_insn(&mut r, labels)).collect();
        // place every label somewhere
        for l in 0..labels {
            let at = r.below(insns.len() as u64 + 1) as usize;
            insns.insert(at, Insn::Label(Label(l)));
        }
        let asm = assemble(&insns).unwrap_or_else(|e| panic!("round {round}: {e}"));
        let back = decode(&asm.code).unwrap_or_else(|e| panic!("round {round}: {e}"));
        assert_eq!(back, normalize(&insns), "round {round}");
    }
}

// ---- disassembler -----------------------------------------------------------------------------------------------

#[test]
fn disassembly_shows_labels_offsets_and_roots() {
    let (head, exit) = (Label(0), Label(1));
    let insns = vec![
        Insn::LoadI { dst: 0, imm: 0 },
        Insn::Label(head),
        Insn::CmpJmp { kind: FusedKind::LtS, a: 0, b: 1, to: exit },
        Insn::AddImm { ty: NumTy::I32, dst: 0, src: 0, imm: 1 },
        Insn::Jmp { to: head },
        Insn::Label(exit),
        Insn::Ret { src: 0 },
    ];
    let asm = assemble(&insns).unwrap();
    let chunk = Chunk {
        name: "count".into(),
        code: asm.code,
        params: 1,
        frame_size: 2,
        param_refs: vec![false],
        upvals: vec![],
        stack_maps: vec![StackMap { pc: 1, live_roots: [1u8].into_iter().collect() }],
    };
    let text = disassemble_chunk(&chunk).unwrap();
    assert_eq!(
        text,
        "== count (params=1, frame=2, code=6 words) ==\n\
         \x20   0  loadi     r0, 0\n\
         \x20     L1:\n\
         \x20   1  lt.s.jf   r0, r1, L5    ; roots {r1}\n\
         \x20   3  addi.i32  r0, r0, 1\n\
         \x20   4  jmp       L1\n\
         \x20     L5:\n\
         \x20   5  ret       r0\n"
    );
    assert_eq!(chunk.stack_map_at(1).unwrap().live_roots.count(), 1);
    assert!(chunk.stack_map_at(2).is_none());
}

#[test]
fn every_instruction_has_a_listing() {
    for insn in every_variant() {
        let s = format_insn(&insn);
        assert!(!s.is_empty());
    }
    assert_eq!(format_insn(&Insn::Call { base: 3, func: 7, nargs: 2 }), "call      f7, [r3..r4]");
    assert_eq!(format_insn(&Insn::CallClosure { base: 3, nargs: 1 }), "callc     r3, args [r4]");
}

// ---- liveness ---------------------------------------------------------------------------------------------------------

fn items(insns: Vec<Insn>) -> Vec<Item> {
    insns.into_iter().map(Item::new).collect()
}

#[test]
fn liveness_of_straight_line_code() {
    let it = items(vec![
        Insn::LoadI { dst: 1, imm: 5 },
        Insn::Arith { op: ArithOp::Add, ty: NumTy::I32, dst: 2, a: 0, b: 1 },
        Insn::Ret { src: 2 },
    ]);
    let live = liveness(&it, &BitSet::new());
    assert_eq!(live[0].iter().collect::<Vec<_>>(), [0, 1]);
    assert_eq!(live[1].iter().collect::<Vec<_>>(), [2]);
    assert!(live[2].is_empty());
}

#[test]
fn liveness_through_loops_and_branches() {
    let (head, exit) = (Label(0), Label(1));
    let it = items(vec![
        Insn::LoadI { dst: 1, imm: 0 },
        Insn::Label(head),
        Insn::CmpJmp { kind: FusedKind::LtS, a: 1, b: 0, to: exit },
        Insn::AddImm { ty: NumTy::I32, dst: 1, src: 1, imm: 1 },
        Insn::Jmp { to: head },
        Insn::Label(exit),
        Insn::Ret { src: 1 },
    ]);
    let live = liveness(&it, &BitSet::new());
    // the bound (r0) stays live around the whole loop
    for i in [1, 2, 3, 4] {
        assert!(live[i].contains(0), "r0 live after item {i}");
    }
    assert!(live[4].contains(1) && live[4].contains(0));
    assert_eq!(live[5].iter().collect::<Vec<_>>(), [1]);
}

#[test]
fn calls_clobber_registers_above_their_base() {
    let it = items(vec![
        Insn::LoadI { dst: 0, imm: 1 },
        Insn::LoadI { dst: 2, imm: 2 },
        Insn::Call { base: 2, func: 0, nargs: 1 },
        Insn::Arith { op: ArithOp::Add, ty: NumTy::I32, dst: 1, a: 0, b: 2 },
        Insn::Ret { src: 1 },
    ]);
    let live = liveness(&it, &BitSet::new());
    // r0 survives the call (below the window); r2 is the argument going in and the result coming out
    assert_eq!(live[1].iter().collect::<Vec<_>>(), [0, 2]);
    assert_eq!(live[2].iter().collect::<Vec<_>>(), [0, 2]);
    assert!(!live[0].contains(2), "r2 is defined before the call");
}

#[test]
fn always_live_registers_are_never_dead() {
    let it = items(vec![Insn::LoadI { dst: 5, imm: 1 }, Insn::Ret { src: 0 }]);
    let always: BitSet = [5u8].into_iter().collect();
    let live = liveness(&it, &always);
    assert!(live[0].contains(5));
}

// ---- peephole ------------------------------------------------------------------------------------------------------------

fn opt(insns: Vec<Insn>, keep: &[u8]) -> (Vec<Insn>, super::peephole::PeepholeStats) {
    let mut it = items(insns);
    let always: BitSet = keep.iter().copied().collect();
    let stats = peephole(&mut it, &always);
    (it.into_iter().map(|i| i.insn).collect(), stats)
}

#[test]
fn removes_self_and_duplicate_moves() {
    let (out, stats) = opt(
        vec![
            Insn::Move { dst: 1, src: 1 },
            Insn::Move { dst: 2, src: 0 },
            Insn::Move { dst: 2, src: 0 },
            Insn::Move { dst: 0, src: 2 },
            Insn::Ret { src: 2 },
        ],
        &[],
    );
    assert_eq!(out, vec![Insn::Move { dst: 2, src: 0 }, Insn::Ret { src: 2 }]);
    assert!(stats.moves_removed >= 2);
}

#[test]
fn coalesces_result_moves() {
    let (out, stats) = opt(
        vec![
            Insn::Arith { op: ArithOp::Add, ty: NumTy::I32, dst: 5, a: 0, b: 1 },
            Insn::Move { dst: 2, src: 5 },
            Insn::Ret { src: 2 },
        ],
        &[],
    );
    assert_eq!(out, vec![Insn::Arith { op: ArithOp::Add, ty: NumTy::I32, dst: 2, a: 0, b: 1 }, Insn::Ret { src: 2 }]);
    assert_eq!(stats.coalesced, 1);
    // not when the temporary is still needed afterwards
    let (out, _) = opt(
        vec![
            Insn::Arith { op: ArithOp::Add, ty: NumTy::I32, dst: 5, a: 0, b: 1 },
            Insn::Move { dst: 2, src: 5 },
            Insn::Arith { op: ArithOp::Add, ty: NumTy::I32, dst: 3, a: 2, b: 5 },
            Insn::Ret { src: 3 },
        ],
        &[],
    );
    assert_eq!(out.len(), 4);
}

#[test]
fn removes_dead_pure_definitions_but_keeps_trapping_ones() {
    let (out, stats) = opt(
        vec![Insn::LoadI { dst: 3, imm: 1 }, Insn::Cmp { kind: CmpKind::Eq, dst: 4, a: 0, b: 1 }, Insn::Ret { src: 0 }],
        &[],
    );
    assert_eq!(out, vec![Insn::Ret { src: 0 }]);
    assert_eq!(stats.dead_removed, 2);
    // integer division may trap, so an unused result does not make it dead
    let div = Insn::Arith { op: ArithOp::Div, ty: NumTy::I32, dst: 3, a: 0, b: 1 };
    let (out, _) = opt(vec![div.clone(), Insn::Ret { src: 0 }], &[]);
    assert_eq!(out, vec![div, Insn::Ret { src: 0 }]);
}

#[test]
fn captured_registers_are_never_dead() {
    // r3 is captured by a closure: the store must stay even though nothing reads it here
    let (out, _) = opt(vec![Insn::LoadI { dst: 3, imm: 7 }, Insn::Ret { src: 0 }], &[3]);
    assert_eq!(out.len(), 2);
}

#[test]
fn fuses_compare_and_branch() {
    let l = Label(0);
    let (out, stats) = opt(
        vec![
            Insn::Cmp { kind: CmpKind::LtS, dst: 5, a: 0, b: 1 },
            Insn::JmpIfNot { cond: 5, to: l },
            Insn::LoadI { dst: 0, imm: 1 },
            Insn::Label(l),
            Insn::Ret { src: 0 },
        ],
        &[],
    );
    assert_eq!(out[0], Insn::CmpJmp { kind: FusedKind::LtS, a: 0, b: 1, to: l });
    assert_eq!(stats.fused, 1);

    // jmpif inverts the integer comparison: jump if a < b  ==  jump unless b <= a
    let (out, _) = opt(
        vec![
            Insn::Cmp { kind: CmpKind::LtS, dst: 5, a: 0, b: 1 },
            Insn::JmpIf { cond: 5, to: l },
            Insn::LoadI { dst: 0, imm: 1 },
            Insn::Label(l),
            Insn::Ret { src: 0 },
        ],
        &[],
    );
    assert_eq!(out[0], Insn::CmpJmp { kind: FusedKind::LeS, a: 1, b: 0, to: l });

    // float comparisons are not inverted (NaN), and a still-live flag blocks fusion
    let (out, _) = opt(
        vec![
            Insn::Cmp { kind: CmpKind::LtF64, dst: 5, a: 0, b: 1 },
            Insn::JmpIf { cond: 5, to: l },
            Insn::LoadI { dst: 0, imm: 1 },
            Insn::Label(l),
            Insn::Ret { src: 0 },
        ],
        &[],
    );
    assert!(matches!(out[0], Insn::Cmp { .. }));
    let (out, _) = opt(
        vec![
            Insn::Cmp { kind: CmpKind::LtS, dst: 5, a: 0, b: 1 },
            Insn::JmpIfNot { cond: 5, to: l },
            Insn::LoadI { dst: 0, imm: 1 },
            Insn::Label(l),
            Insn::Ret { src: 5 },
        ],
        &[],
    );
    assert!(matches!(out[0], Insn::Cmp { .. }));
    // float "jump if false" fuses
    let (out, _) = opt(
        vec![
            Insn::Cmp { kind: CmpKind::LtF64, dst: 5, a: 0, b: 1 },
            Insn::JmpIfNot { cond: 5, to: l },
            Insn::LoadI { dst: 0, imm: 1 },
            Insn::Label(l),
            Insn::Ret { src: 0 },
        ],
        &[],
    );
    assert!(matches!(out[0], Insn::CmpJmp { kind: FusedKind::LtF64, .. }));
}

#[test]
fn folds_small_constants_into_immediates() {
    let ty = NumTy::I32;
    let (out, stats) = opt(
        vec![
            Insn::LoadI { dst: 5, imm: 7 },
            Insn::Arith { op: ArithOp::Add, ty, dst: 1, a: 0, b: 5 },
            Insn::Ret { src: 1 },
        ],
        &[],
    );
    assert_eq!(out[0], Insn::AddImm { ty, dst: 1, src: 0, imm: 7 });
    assert_eq!(stats.immediates, 1);
    let (out, _) = opt(
        vec![
            Insn::LoadI { dst: 5, imm: 7 },
            Insn::Arith { op: ArithOp::Sub, ty, dst: 1, a: 0, b: 5 },
            Insn::Ret { src: 1 },
        ],
        &[],
    );
    assert_eq!(out[0], Insn::AddImm { ty, dst: 1, src: 0, imm: -7 });
    // constant on the left of an addition commutes; on the left of a subtraction it does not
    let (out, _) = opt(
        vec![
            Insn::LoadI { dst: 5, imm: 3 },
            Insn::Arith { op: ArithOp::Add, ty, dst: 1, a: 5, b: 0 },
            Insn::Ret { src: 1 },
        ],
        &[],
    );
    assert_eq!(out[0], Insn::AddImm { ty, dst: 1, src: 0, imm: 3 });
    let (out, _) = opt(
        vec![
            Insn::LoadI { dst: 5, imm: 3 },
            Insn::Arith { op: ArithOp::Sub, ty, dst: 1, a: 5, b: 0 },
            Insn::Ret { src: 1 },
        ],
        &[],
    );
    assert_eq!(out.len(), 3);
    // too large for an immediate
    let (out, _) = opt(
        vec![
            Insn::LoadI { dst: 5, imm: 1000 },
            Insn::Arith { op: ArithOp::Add, ty, dst: 1, a: 0, b: 5 },
            Insn::Ret { src: 1 },
        ],
        &[],
    );
    assert_eq!(out.len(), 3);
    // adding -3 becomes subtracting 3; the type's own interpretation of the bits is used
    let (out, _) = opt(
        vec![
            Insn::LoadI { dst: 5, imm: -3 },
            Insn::Arith { op: ArithOp::Add, ty, dst: 1, a: 0, b: 5 },
            Insn::Ret { src: 1 },
        ],
        &[],
    );
    assert_eq!(out[0], Insn::AddImm { ty, dst: 1, src: 0, imm: -3 });
    // u64 -1 is 2^64-1, not -1: no immediate
    let (out, _) = opt(
        vec![
            Insn::LoadI { dst: 5, imm: -1 },
            Insn::Arith { op: ArithOp::Add, ty: NumTy::U64, dst: 1, a: 0, b: 5 },
            Insn::Ret { src: 1 },
        ],
        &[],
    );
    assert_eq!(out.len(), 3);
}

#[test]
fn simplifies_jumps() {
    let (a, b) = (Label(0), Label(1));
    // jump to the next instruction; jump threading; unreachable code
    let (out, stats) = opt(
        vec![
            Insn::JmpIf { cond: 0, to: a },
            Insn::Jmp { to: b },
            Insn::LoadI { dst: 0, imm: 99 }, // unreachable
            Insn::Label(a),
            Insn::Jmp { to: b },
            Insn::Label(b),
            Insn::Ret { src: 0 },
        ],
        &[],
    );
    assert!(stats.jumps_removed > 0);
    assert!(!out.iter().any(|i| matches!(i, Insn::LoadI { imm: 99, .. })), "{out:?}");
    assert_eq!(out.last(), Some(&Insn::Ret { src: 0 }));
    assert!(out.len() <= 3, "{out:?}");
}

#[test]
fn unused_labels_are_dropped() {
    let (out, _) = opt(vec![Insn::Label(Label(9)), Insn::Ret { src: 0 }], &[]);
    assert_eq!(out, vec![Insn::Ret { src: 0 }]);
}

// ---- peephole: semantic equivalence on random programs -------------------------------------------------------------------------

/// A tiny interpreter for the subset of instructions the random programs use.
fn exec(insns: &[Insn], init: [u64; 8]) -> Result<u64, Trap> {
    let labels: HashMap<Label, usize> = insns
        .iter()
        .enumerate()
        .filter_map(|(i, x)| if let Insn::Label(l) = x { Some((*l, i)) } else { None })
        .collect();
    let mut r = init;
    let mut pc = 0;
    let mut steps = 0;
    while pc < insns.len() {
        steps += 1;
        assert!(steps < 10_000, "runaway program");
        let go = |l: &Label| labels[l];
        let mut next = pc + 1;
        match &insns[pc] {
            Insn::Label(_) => {}
            Insn::Move { dst, src } => r[*dst as usize] = r[*src as usize],
            Insn::LoadI { dst, imm } => r[*dst as usize] = *imm as i64 as u64,
            // registers hold raw bits here, so view the operands as canonical values of
            // the instruction's type (shift amounts are plain i64s)
            Insn::Arith { op, ty, dst, a, b } => {
                let x = numeric::canon_bits(*ty, r[*a as usize]);
                let y = if op.is_shift() { r[*b as usize] } else { numeric::canon_bits(*ty, r[*b as usize]) };
                r[*dst as usize] = numeric::arith(*op, *ty, x, y)?
            }
            Insn::Bitwise { op, dst, a, b } => {
                r[*dst as usize] = numeric::arith(*op, NumTy::I64, r[*a as usize], r[*b as usize])?
            }
            Insn::AddImm { ty, dst, src, imm } => {
                let v = numeric::decode_int(*ty, numeric::canon_bits(*ty, r[*src as usize])) + *imm as i128;
                r[*dst as usize] = numeric::encode_int(*ty, v).ok_or(Trap::Overflow)?;
            }
            Insn::SubImm { ty, dst, src, imm } => {
                let v = numeric::decode_int(*ty, numeric::canon_bits(*ty, r[*src as usize])) - *imm as i128;
                r[*dst as usize] = numeric::encode_int(*ty, v).ok_or(Trap::Overflow)?;
            }
            Insn::BoolNot { dst, src } => r[*dst as usize] = (r[*src as usize] == 0) as u64,
            Insn::Cmp { kind, dst, a, b } => r[*dst as usize] = eval_cmp(*kind, r[*a as usize], r[*b as usize]) as u64,
            Insn::Jmp { to } => next = go(to),
            Insn::JmpIf { cond, to } => {
                if r[*cond as usize] != 0 {
                    next = go(to)
                }
            }
            Insn::JmpIfNot { cond, to } => {
                if r[*cond as usize] == 0 {
                    next = go(to)
                }
            }
            Insn::CmpJmp { kind, a, b, to } => {
                if !eval_cmp(kind.cmp(), r[*a as usize], r[*b as usize]) {
                    next = go(to)
                }
            }
            Insn::Ret { src } => return Ok(r[*src as usize]),
            other => panic!("unsupported in the test interpreter: {other:?}"),
        }
        pc = next;
    }
    panic!("fell off the end");
}

fn eval_cmp(kind: CmpKind, a: u64, b: u64) -> bool {
    match kind {
        CmpKind::Eq => a == b,
        CmpKind::Ne => a != b,
        CmpKind::LtS => numeric::compare(CmpOp::Lt, NumTy::I64, a, b),
        CmpKind::LeS => numeric::compare(CmpOp::Le, NumTy::I64, a, b),
        CmpKind::LtU => a < b,
        CmpKind::LeU => a <= b,
        CmpKind::LtF64 => numeric::compare(CmpOp::Lt, NumTy::F64, a, b),
        CmpKind::LeF64 => numeric::compare(CmpOp::Le, NumTy::F64, a, b),
        CmpKind::EqF64 => numeric::compare(CmpOp::Eq, NumTy::F64, a, b),
        CmpKind::NeF64 => numeric::compare(CmpOp::Ne, NumTy::F64, a, b),
        other => panic!("unsupported comparison {other:?}"),
    }
}

fn random_program(r: &mut Rng) -> Vec<Insn> {
    const REGS: u64 = 7;
    let reg = |r: &mut Rng| r.below(REGS) as u8;
    let n = 4 + r.below(24);
    let mut insns = Vec::new();
    let mut pending: Vec<Label> = Vec::new();
    let mut next_label = 0u32;
    let kinds = [
        CmpKind::Eq,
        CmpKind::Ne,
        CmpKind::LtS,
        CmpKind::LeS,
        CmpKind::LtU,
        CmpKind::LeU,
        CmpKind::LtF64,
        CmpKind::EqF64,
    ];
    for _ in 0..n {
        // sometimes place a pending label (forward jumps only, so programs terminate)
        if !pending.is_empty() && r.below(4) == 0 {
            let i = r.below(pending.len() as u64) as usize;
            insns.push(Insn::Label(pending.remove(i)));
        }
        let mut new_target = |pending: &mut Vec<Label>| {
            let l = Label(next_label);
            next_label += 1;
            pending.push(l);
            l
        };
        match r.below(12) {
            0 => insns.push(Insn::Move { dst: reg(r), src: reg(r) }),
            1 => insns.push(Insn::LoadI { dst: reg(r), imm: (r.below(600) as i16) - 300 }),
            2 => insns.push(Insn::Arith {
                op: r.pick(&[ArithOp::Add, ArithOp::Sub, ArithOp::Mul]),
                ty: NumTy::I32,
                dst: reg(r),
                a: reg(r),
                b: reg(r),
            }),
            3 => {
                // an immediate-friendly pair
                let t = reg(r);
                insns.push(Insn::LoadI { dst: t, imm: (r.below(300) as i16) - 100 });
                insns.push(Insn::Arith {
                    op: r.pick(&[ArithOp::Add, ArithOp::Sub]),
                    ty: r.pick(&[NumTy::I32, NumTy::U8, NumTy::I64]),
                    dst: reg(r),
                    a: reg(r),
                    b: t,
                });
            }
            4 => insns.push(Insn::AddImm { ty: NumTy::I32, dst: reg(r), src: reg(r), imm: r.next() as i8 }),
            5 | 6 => {
                // a compare/branch pair
                let t = reg(r);
                insns.push(Insn::Cmp { kind: r.pick(&kinds), dst: t, a: reg(r), b: reg(r) });
                let to = new_target(&mut pending);
                insns.push(if r.below(2) == 0 { Insn::JmpIf { cond: t, to } } else { Insn::JmpIfNot { cond: t, to } });
            }
            7 => {
                let to = new_target(&mut pending);
                insns.push(Insn::CmpJmp {
                    kind: r.pick(&[FusedKind::Eq, FusedKind::LtS, FusedKind::LeU]),
                    a: reg(r),
                    b: reg(r),
                    to,
                });
            }
            8 => {
                let to = new_target(&mut pending);
                insns.push(Insn::Jmp { to });
            }
            9 => insns.push(Insn::Bitwise {
                op: r.pick(&[ArithOp::And, ArithOp::Or, ArithOp::Xor]),
                dst: reg(r),
                a: reg(r),
                b: reg(r),
            }),
            10 => insns.push(Insn::BoolNot { dst: reg(r), src: reg(r) }),
            _ => {
                // op into a temp then a move (coalescing bait)
                let t = reg(r);
                insns.push(Insn::Arith { op: ArithOp::Add, ty: NumTy::I32, dst: t, a: reg(r), b: reg(r) });
                insns.push(Insn::Move { dst: reg(r), src: t });
            }
        }
    }
    for l in pending {
        insns.push(Insn::Label(l));
    }
    insns.push(Insn::Ret { src: 0 });
    insns
}

#[test]
fn peephole_preserves_behavior_on_random_programs() {
    let mut r = Rng(0x9E3779B97F4A7C15);
    let mut total = super::peephole::PeepholeStats::default();
    for round in 0..4000 {
        let original = random_program(&mut r);
        let keep: Vec<u8> = if r.below(3) == 0 { vec![r.below(7) as u8] } else { vec![] };
        let mut it = items(original.clone());
        let always: BitSet = keep.iter().copied().collect();
        let stats = peephole(&mut it, &always);
        total.moves_removed += stats.moves_removed;
        total.dead_removed += stats.dead_removed;
        total.coalesced += stats.coalesced;
        total.fused += stats.fused;
        total.immediates += stats.immediates;
        total.jumps_removed += stats.jumps_removed;
        let optimized: Vec<Insn> = it.into_iter().map(|i| i.insn).collect();
        assemble(&optimized).unwrap_or_else(|e| panic!("round {round}: optimized program does not assemble: {e}"));
        for _ in 0..4 {
            let mut init = [0u64; 8];
            for v in init.iter_mut() {
                *v = match r.below(4) {
                    0 => 0,
                    1 => (r.below(10) as i64 - 5) as u64,
                    2 => r.next() % 100_000,
                    _ => r.next(),
                };
            }
            assert_eq!(
                exec(&original, init),
                exec(&optimized, init),
                "round {round}, init {init:?}\noriginal: {original:#?}\noptimized: {optimized:#?}"
            );
        }
    }
    // the generator really exercised every rule
    assert!(total.moves_removed > 50 && total.dead_removed > 50 && total.coalesced > 50, "{total:?}");
    assert!(total.fused > 50 && total.immediates > 50 && total.jumps_removed > 50, "{total:?}");
}
