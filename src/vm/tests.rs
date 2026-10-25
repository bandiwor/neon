use super::*;
use crate::bytecode::{assemble, Chunk, Insn, Label};
use crate::gc::StackMap;
use crate::ir::{DescId, GlobalInfo, IrTy};
use crate::runtime::numeric::{ArithOp, NumTy};

fn chunk(name: &str, insns: &[Insn], params: u8, frame: u16) -> Chunk {
    let asm = assemble(insns).unwrap();
    let stack_maps = insns
        .iter()
        .enumerate()
        .filter(|(_, i)| i.is_safe_point())
        .map(|(i, _)| StackMap { pc: asm.pcs[i], live_roots: Default::default() })
        .collect();
    Chunk {
        name: name.into(),
        code: asm.code,
        params,
        frame_size: frame,
        param_refs: vec![false; params as usize],
        upvals: vec![],
        stack_maps,
    }
}

fn program(chunks: Vec<Chunk>) -> BytecodeProgram {
    BytecodeProgram { chunks, ..Default::default() }
}

fn exec(p: &BytecodeProgram) -> (Result<(), RuntimeError>, BufferIo) {
    let mut io = BufferIo::new();
    let r = run(p, &mut io, &VmConfig::default()).result;
    (r, io)
}

fn add(dst: u8, a: u8, b: u8) -> Insn {
    Insn::Arith { op: ArithOp::Add, ty: NumTy::I64, dst, a, b }
}

#[test]
fn runs_a_hand_assembled_loop() {
    // sum = 0; i = 0; while i < 10 { sum += i; i += 1 }; globals[0] = sum
    let (head, exit) = (Label(0), Label(1));
    let main = chunk(
        "main",
        &[
            Insn::LoadI { dst: 0, imm: 0 },
            Insn::LoadI { dst: 1, imm: 0 },
            Insn::LoadI { dst: 2, imm: 10 },
            Insn::Label(head),
            Insn::CmpJmp { kind: crate::bytecode::FusedKind::LtS, a: 1, b: 2, to: exit },
            add(0, 0, 1),
            Insn::AddImm { ty: NumTy::I64, dst: 1, src: 1, imm: 1 },
            Insn::Jmp { to: head },
            Insn::Label(exit),
            Insn::SetGlobal { src: 0, g: 0 },
            Insn::Ret { src: 0 },
        ],
        0,
        3,
    );
    let mut p = program(vec![main]);
    p.globals = vec![GlobalInfo { name: "sum".into(), ty: IrTy::Num(NumTy::I64), desc: DescId(0) }];
    p.descs = vec![crate::ir::TypeDesc::Num(NumTy::I64)];
    let mut io = BufferIo::new();
    let mut vm = Vm::new(&p, &mut io, &VmConfig::default());
    vm.run().unwrap();
    assert_eq!(vm.globals[0], 45);
}

#[test]
fn calls_use_the_sliding_window_and_return_in_the_base_register() {
    // f1(x) = x + x ; main: r0 = 21; r1 = r0 (arg); call f1 at base 1 -> result in r1; ret r1
    let double = chunk("double", &[add(1, 0, 0), Insn::Ret { src: 1 }], 1, 2);
    let main = chunk(
        "main",
        &[
            Insn::LoadI { dst: 0, imm: 21 },
            Insn::Move { dst: 1, src: 0 },
            Insn::Call { base: 1, func: 1, nargs: 1 },
            Insn::SetGlobal { src: 1, g: 0 },
            Insn::Ret { src: 0 },
        ],
        0,
        2,
    );
    let mut p = program(vec![main, double]);
    p.globals = vec![GlobalInfo { name: "r".into(), ty: IrTy::Num(NumTy::I64), desc: DescId(0) }];
    let mut io = BufferIo::new();
    let mut vm = Vm::new(&p, &mut io, &VmConfig::default());
    vm.run().unwrap();
    assert_eq!(vm.globals[0], 42);
}

#[test]
fn traps_carry_a_backtrace() {
    let inner = chunk(
        "inner",
        &[
            Insn::LoadI { dst: 1, imm: 0 },
            Insn::Arith { op: ArithOp::Div, ty: NumTy::I64, dst: 1, a: 0, b: 1 },
            Insn::Ret { src: 1 },
        ],
        1,
        2,
    );
    let outer = chunk("outer", &[Insn::Call { base: 0, func: 2, nargs: 1 }, Insn::Ret { src: 0 }], 1, 1);
    let main = chunk(
        "main",
        &[Insn::LoadI { dst: 0, imm: 5 }, Insn::Call { base: 0, func: 1, nargs: 1 }, Insn::Ret { src: 0 }],
        0,
        1,
    );
    let (r, _) = exec(&program(vec![main, outer, inner]));
    let e = r.unwrap_err();
    assert_eq!(e.trap, Trap::DivideByZero);
    assert_eq!(e.backtrace, ["inner", "outer", "main"]);
    let text = e.to_string();
    assert!(text.starts_with("runtime error: division by zero") && text.contains("at outer"), "{text}");
}

#[test]
fn invalid_opcodes_and_unreachable_code_trap() {
    let mut bad = chunk("main", &[Insn::Ret { src: 0 }], 0, 1);
    bad.code = vec![0xFE00_0000];
    assert!(matches!(exec(&program(vec![bad])).0.unwrap_err().trap, Trap::Panic(m) if m.contains("invalid opcode")));
    let un = chunk("main", &[Insn::Unreachable], 0, 1);
    assert!(matches!(exec(&program(vec![un])).0.unwrap_err().trap, Trap::Panic(m) if m.contains("unreachable")));
}

#[test]
fn recursion_beyond_the_frame_limit_overflows_the_stack() {
    // f() calls itself forever
    let f = chunk("f", &[Insn::Call { base: 0, func: 1, nargs: 0 }, Insn::Ret { src: 0 }], 0, 1);
    let main = chunk("main", &[Insn::Call { base: 0, func: 1, nargs: 0 }, Insn::Ret { src: 0 }], 0, 1);
    let p = program(vec![main, f]);
    let mut io = BufferIo::new();
    let cfg = VmConfig { max_frames: 100, ..VmConfig::default() };
    let r = run(&p, &mut io, &cfg).result.unwrap_err();
    assert_eq!(r.trap, Trap::StackOverflow);
    assert_eq!(r.backtrace.len(), 100);
    // and by stack size, when frames are big
    let cfg = VmConfig { max_stack_words: 64, ..VmConfig::default() };
    let r = run(&p, &mut io, &cfg).result.unwrap_err();
    assert_eq!(r.trap, Trap::StackOverflow);
}

#[test]
fn the_step_limit_stops_infinite_loops() {
    let l = Label(0);
    let main = chunk("main", &[Insn::Label(l), Insn::Jmp { to: l }], 0, 1);
    let mut io = BufferIo::new();
    let cfg = VmConfig { max_steps: Some(1000), ..VmConfig::default() };
    let r = run(&program(vec![main]), &mut io, &cfg);
    assert!(matches!(r.result.unwrap_err().trap, Trap::Panic(m) if m.contains("step limit")));
    assert_eq!(r.steps, 1001);
}

#[test]
fn wide_jumps_and_constants_execute() {
    let (skip, _) = (Label(0), ());
    let mut insns = vec![Insn::LoadK { dst: 0, k: 70_000 }, Insn::Jmp { to: skip }];
    insns.extend((0..40_000).map(|_| Insn::LoadI { dst: 1, imm: 99 }));
    insns.push(Insn::Label(skip));
    insns.push(Insn::SetGlobal { src: 0, g: 0 });
    insns.push(Insn::Ret { src: 0 });
    let mut p = program(vec![chunk("main", &insns, 0, 2)]);
    p.constants = vec![0; 70_001];
    p.constants[70_000] = 0xDEAD_BEEF_CAFE;
    p.globals = vec![GlobalInfo { name: "g".into(), ty: IrTy::Num(NumTy::U64), desc: DescId(0) }];
    let mut io = BufferIo::new();
    let mut vm = Vm::new(&p, &mut io, &VmConfig::default());
    vm.run().unwrap();
    assert_eq!(vm.globals[0], 0xDEAD_BEEF_CAFE);
}

#[test]
fn op_table_covers_every_opcode() {
    let t = op_table();
    for &op in Opcode::ALL {
        assert!(!matches!(t[op as usize], OpKind::Invalid), "{op:?}");
    }
    assert!(matches!(t[Opcode::ALL.len()], OpKind::Invalid));
    assert!(matches!(t[EXT_ESCAPE as usize], OpKind::Invalid));
}

#[test]
fn cast_codes_decode_to_type_pairs() {
    for (i, &from) in NumTy::ALL.iter().enumerate() {
        for (j, &to) in NumTy::ALL.iter().enumerate() {
            assert_eq!(cast_types((i * 10 + j) as u8), (from, to));
        }
    }
}
