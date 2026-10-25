//! Phase 6: public-API round trip through assembler, decoder, disassembler and peephole.

use neon::bytecode::*;
use neon::gc::BitSet;
use neon::runtime::numeric::{ArithOp, NumTy};

fn counting_loop() -> Vec<Item> {
    let (head, exit) = (Label(0), Label(1));
    [
        Insn::LoadI { dst: 1, imm: 0 },
        Insn::Label(head),
        Insn::Cmp { kind: CmpKind::LtS, dst: 3, a: 1, b: 0 },
        Insn::JmpIfNot { cond: 3, to: exit },
        Insn::LoadI { dst: 4, imm: 1 },
        Insn::Arith { op: ArithOp::Add, ty: NumTy::I64, dst: 5, a: 1, b: 4 },
        Insn::Move { dst: 1, src: 5 },
        Insn::Jmp { to: head },
        Insn::Label(exit),
        Insn::Ret { src: 1 },
    ]
    .into_iter()
    .map(Item::new)
    .collect()
}

#[test]
fn peephole_shrinks_a_loop_and_the_result_still_round_trips() {
    let mut items = counting_loop();
    let before = assemble(&items.iter().map(|i| i.insn.clone()).collect::<Vec<_>>()).unwrap().code.len();
    let stats = neon::bytecode::peephole::peephole(&mut items, &BitSet::new());
    let insns: Vec<Insn> = items.into_iter().map(|i| i.insn).collect();
    let asm = assemble(&insns).unwrap();
    assert!(asm.code.len() < before, "{} !< {before}", asm.code.len());
    assert!(stats.fused >= 1 && stats.immediates >= 1, "{stats:?}");
    let decoded = decode(&asm.code).unwrap();
    assert!(decoded.iter().any(|i| matches!(i, Insn::CmpJmp { .. })));
    assert!(decoded.iter().any(|i| matches!(i, Insn::AddImm { imm: 1, .. })));
    let chunk = Chunk {
        name: "loop".into(),
        code: asm.code,
        params: 1,
        frame_size: 6,
        param_refs: vec![false],
        upvals: vec![],
        stack_maps: vec![],
    };
    let text = disassemble_chunk(&chunk).unwrap();
    assert!(text.contains("lt.s.jf") && text.contains("addi.i64"), "{text}");
}
