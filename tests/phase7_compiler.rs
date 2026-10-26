//! Phase 7: IR -> bytecode. Every program of the shared corpus must compile
//! under every optimization setting into bytecode that passes the structural
//! verifier; the reference interpreter confirms the corpus' expected output.

use neon::bytecode::{disassemble, verify_program, BytecodeProgram, Insn};
use neon::compiler::CompileOptions;
use neon::driver::{compile_source, Options};
use neon::ir::{interp, OptConfig};
use neon::vfs::MemVfs;

fn corpus() -> Vec<(String, String, String)> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/corpus");
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "neon") {
            let name = p.file_stem().unwrap().to_string_lossy().into_owned();
            let src = std::fs::read_to_string(&p).unwrap();
            let exp = std::fs::read_to_string(p.with_extension("out")).unwrap();
            out.push((name, src, exp));
        }
    }
    out.sort();
    out
}

fn configs() -> Vec<(&'static str, Options)> {
    let none = OptConfig::none();
    let full = OptConfig::default();
    vec![
        ("O0/raw", Options { opt: none, compile: CompileOptions { peephole: false } }),
        ("O0/peep", Options { opt: none, compile: CompileOptions { peephole: true } }),
        ("O1/raw", Options { opt: full, compile: CompileOptions { peephole: false } }),
        ("O1/peep", Options { opt: full, compile: CompileOptions { peephole: true } }),
    ]
}

fn bytecode(src: &str, opts: &Options) -> BytecodeProgram {
    let c = compile_source("main.neon", src, &MemVfs::new(), opts);
    assert!(!c.diags.has_errors(), "{}", c.render_diagnostics());
    c.bytecode.expect("bytecode")
}

#[test]
fn the_corpus_compiles_to_verified_bytecode_under_every_setting() {
    let programs = corpus();
    assert!(programs.len() >= 25, "corpus is missing: {}", programs.len());
    for (name, src, _) in &programs {
        for (cfg, opts) in configs() {
            let bc = bytecode(src, opts_ref(&opts));
            verify_program(&bc).unwrap_or_else(|e| panic!("{name} [{cfg}]: {e}\n{}", disassemble(&bc).unwrap()));
            for c in &bc.chunks {
                assert!(c.frame_size <= 256, "{name} [{cfg}] {}: frame {}", c.name, c.frame_size);
            }
        }
    }
}

fn opts_ref(o: &Options) -> &Options {
    o
}

#[test]
fn corpus_expectations_hold_in_the_reference_interpreter() {
    for (name, src, expected) in corpus() {
        let c = compile_source("main.neon", &src, &MemVfs::new(), &Options::default());
        let ir = c.ir.expect("ir");
        let r = interp::run(&ir, &[]);
        if name == "integer_boundaries_trap" {
            continue; // intentionally ends in a trap after printing
        }
        assert!(r.trap.is_none(), "{name}: {:?}", r.trap);
        assert_eq!(r.output, expected, "{name}");
    }
}

#[test]
fn peephole_makes_code_smaller_without_changing_stack_maps_invariants() {
    let (_, src, _) = corpus().into_iter().find(|(n, _, _)| n == "functions_recursion_and_fib").unwrap();
    let raw = bytecode(&src, &configs()[2].1);
    let peep = bytecode(&src, &configs()[3].1);
    let size = |p: &BytecodeProgram| p.chunks.iter().map(|c| c.code.len()).sum::<usize>();
    assert!(size(&peep) < size(&raw), "{} !< {}", size(&peep), size(&raw));
}

#[test]
fn recursive_function_uses_fused_compare_and_the_sliding_window() {
    let bc = bytecode(
        "fn fib(n: i32) -> i32 { if n < 2 { n } else { fib(n - 1) + fib(n - 2) } }\nprintln(fib(10));",
        &Options { opt: OptConfig::none(), compile: CompileOptions::default() },
    );
    let text = disassemble(&bc).unwrap();
    let fib = bc.chunks.iter().position(|c| c.name.starts_with("fib")).unwrap();
    let insns = neon::bytecode::decode(&bc.chunks[fib].code).unwrap();
    assert!(insns.iter().any(|i| matches!(i, Insn::CmpJmp { .. })), "{text}");
    let calls = insns.iter().filter(|i| matches!(i, Insn::Call { .. })).count();
    assert_eq!(calls, 2, "{text}");
    assert!(bc.chunks[fib].frame_size <= 8, "a tiny function needs a tiny frame: {}", bc.chunks[fib].frame_size);
    assert_eq!(bc.chunks[fib].params, 1);
}

#[test]
fn values_live_across_a_call_stay_below_its_window() {
    // `keep` must survive the call, so the window starts above its register
    let bc = bytecode(
        "fn id(x: i64) -> i64 { x }\nfn f(a: i64, b: i64) -> i64 { let keep = a * b; let r = id(a + 1); keep + r }\nprintln(f(3, 4));",
        &Options { opt: OptConfig::none(), compile: CompileOptions::default() },
    );
    let f = bc.chunks.iter().find(|c| c.name.starts_with('f')).unwrap();
    let insns = neon::bytecode::decode(&f.code).unwrap();
    let mul_dst = insns
        .iter()
        .find_map(|i| match i {
            Insn::Arith { op: neon::runtime::numeric::ArithOp::Mul, dst, .. } => Some(*dst),
            _ => None,
        })
        .unwrap();
    let base = insns
        .iter()
        .find_map(|i| match i {
            Insn::Call { base, .. } => Some(*base),
            _ => None,
        })
        .unwrap();
    assert!(
        base > mul_dst,
        "window r{base} must sit above the live value in r{mul_dst}\n{}",
        disassemble(&bc).unwrap()
    );
}

#[test]
fn closures_compile_with_upvalue_metadata() {
    let bc = bytecode(
        "fn make() -> fn() -> i32 { var n = 0; || => { n += 1; n } }\nlet c = make(); println(c()); println(c());",
        &Options::default(),
    );
    verify_program(&bc).unwrap();
    let closure = bc.chunks.iter().find(|c| !c.upvals.is_empty()).expect("a closure chunk");
    assert_eq!(closure.upvals, [false], "the captured counter is an integer");
    let all: Vec<Insn> = bc.chunks.iter().flat_map(|c| neon::bytecode::decode(&c.code).unwrap()).collect();
    assert!(all.iter().any(|i| matches!(i, Insn::MakeClosure { .. })));
    assert!(all.iter().any(|i| matches!(i, Insn::GetUpval { .. })));
    assert!(all.iter().any(|i| matches!(i, Insn::Close { .. })), "scope exit closes the upvalue");
}

#[test]
fn reference_parameters_and_locals_appear_in_stack_maps() {
    let bc = bytecode(
        "fn f(a: [i32]) -> i64 { let b = [1, 2, 3]; let n = a.len() + b.len(); println(n); a.len() }\nprintln(f([1]));",
        &Options { opt: OptConfig::none(), compile: CompileOptions::default() },
    );
    let f = bc.chunks.iter().find(|c| c.name.starts_with('f')).unwrap();
    // at some safe-point the parameter array (r0) is a root
    assert!(f.stack_maps.iter().any(|m| m.live_roots.contains(0)), "{:?}", f.stack_maps);
    // and integers never are: the map holds only the two arrays
    assert!(f.stack_maps.iter().all(|m| m.live_roots.count() <= 2), "{:?}", f.stack_maps);
}

#[test]
fn large_literals_are_built_incrementally() {
    let elems: Vec<String> = (0..600).map(|i| i.to_string()).collect();
    let src = format!("let a = [{}];\nprintln(a.len());\nprintln(a[599]);", elems.join(", "));
    let bc = bytecode(&src, &Options::default());
    verify_program(&bc).unwrap();
    let all: Vec<Insn> = bc.chunks.iter().flat_map(|c| neon::bytecode::decode(&c.code).unwrap()).collect();
    let pushes = all.iter().filter(|i| matches!(i, Insn::Intrinsic { b: neon::builtin::Builtin::ArrPush, .. })).count();
    assert!(pushes >= 400, "{pushes}");
}

#[test]
fn constants_are_pooled_and_small_ones_are_immediates() {
    let bc = bytecode(
        "let a = 7; let b = 100000; let c = 100000; let d = 3.5; let e = 3.5; println(a + b + c); println(d + e);",
        &Options { opt: OptConfig::none(), compile: CompileOptions { peephole: false } },
    );
    // 100000 and 3.5 are each stored once
    assert_eq!(bc.constants.iter().filter(|&&k| k == 100000).count(), 1);
    assert_eq!(bc.constants.iter().filter(|&&k| k == 3.5f64.to_bits()).count(), 1);
    assert!(!bc.constants.contains(&7));
}

#[test]
fn a_function_needing_more_than_256_registers_is_a_diagnostic() {
    let mut src = String::from("fn f() -> i64 {\n");
    for i in 0..300 {
        src += &format!("    let v{i} = {i} * 3;\n");
    }
    src += "    var s = 0;\n";
    for i in 0..300 {
        src += &format!("    s += v{i};\n");
    }
    src += "    s\n}\nprintln(f());\n";
    let c =
        compile_source("main.neon", &src, &MemVfs::new(), &Options { opt: OptConfig::none(), ..Options::default() });
    assert!(c.diags.has_errors());
    let text = c.render_diagnostics();
    assert!(text.contains("256 registers") && text.contains("`f"), "{text}");
    assert!(c.bytecode.is_none());
}

#[test]
fn long_literals_keep_their_meaning_in_the_ir() {
    let elems: Vec<String> = (0..200).map(|i| i.to_string()).collect();
    let entries: Vec<String> = (0..120).map(|i| format!("{i}: {}", i * 2)).collect();
    let src = format!(
        "let a = [{}]; let s = set {{ {} }}; let m = {{ {} }};\nprintln(a.len()); println(a[0] + a[47] + a[48] + a[199]); println(s.len()); println(s.contains(150));\nprintln(m.len()); println(m[119]);",
        elems.join(", "),
        elems.join(", "),
        entries.join(", ")
    );
    let c = compile_source("main.neon", &src, &MemVfs::new(), &Options::default());
    assert!(!c.diags.has_errors(), "{}", c.render_diagnostics());
    let r = interp::run(c.ir.as_ref().unwrap(), &[]);
    assert!(r.trap.is_none(), "{:?}", r.trap);
    assert_eq!(r.output, "200\n294\n200\ntrue\n120\n238\n");
}

#[test]
fn constant_operands_of_additions_become_immediates_wherever_the_constant_was_computed() {
    // the constants are loaded far from their additions (hoisted, shared, or defined by a guard)
    let src = "fn f(n: i64) -> i64 { var i = 0; var j = 1; while i < n { i += 3; j = j - 7 + i; } i + j + 100 }\nprintln(f(input().parse_i64()));";
    for (cfg, opts) in configs() {
        let bc = bytecode(src, opts_ref(&opts));
        let text = disassemble(&bc).unwrap();
        if cfg.starts_with("O1") {
            assert!(text.matches("addi").count() >= 3, "[{cfg}]\n{text}");
        }
        let mut io = neon::vm::BufferIo::with_input(&["10"]);
        let r = neon::vm::run(&bc, &mut io, &neon::vm::VmConfig::default());
        assert!(r.result.is_ok(), "[{cfg}] {:?}", r.result);
        assert_eq!(io.output, "115\n", "[{cfg}]");
    }
}
