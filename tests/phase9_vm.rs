//! Phase 9: the VM against the reference interpreter. Every corpus program
//! must print the same output and trap the same way under every optimization
//! setting, with a normal heap and with a collector that runs at every
//! safe-point (which catches any root the compiler's stack maps miss).

use neon::compiler::CompileOptions;
use neon::driver::{compile_source, Options};
use neon::gc::GcConfig;
use neon::ir::{interp, OptConfig};
use neon::runtime::Trap;
use neon::vfs::MemVfs;
use neon::vm::{self, BufferIo, VmConfig};

fn corpus() -> Vec<(String, String, String)> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/corpus");
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "neon") {
            let name = p.file_stem().unwrap().to_string_lossy().into_owned();
            out.push((
                name,
                std::fs::read_to_string(&p).unwrap(),
                std::fs::read_to_string(p.with_extension("out")).unwrap(),
            ));
        }
    }
    out.sort();
    out
}

fn settings() -> Vec<(&'static str, Options)> {
    let none = OptConfig::none();
    let full = OptConfig::default();
    vec![
        ("O0", Options { opt: none, compile: CompileOptions { peephole: false } }),
        ("O0+peep", Options { opt: none, compile: CompileOptions { peephole: true } }),
        ("O1", Options { opt: full, compile: CompileOptions { peephole: false } }),
        ("O1+peep", Options { opt: full, compile: CompileOptions { peephole: true } }),
    ]
}

struct Outcome {
    output: String,
    trap: Option<Trap>,
}

fn run_vm(src: &str, opts: &Options, cfg: &VmConfig, input: &[&str]) -> Outcome {
    let c = compile_source("main.neon", src, &MemVfs::new(), opts);
    assert!(!c.diags.has_errors(), "{}", c.render_diagnostics());
    let bc = c.bytecode.expect("bytecode");
    let mut io = BufferIo::with_input(input);
    let report = vm::run(&bc, &mut io, cfg);
    Outcome { output: io.output, trap: report.result.err().map(|e| e.trap) }
}

fn run_oracle(src: &str, input: &[&str]) -> Outcome {
    let c = compile_source("main.neon", src, &MemVfs::new(), &Options { opt: OptConfig::none(), ..Options::default() });
    let ir = c.ir.expect("ir");
    let r = interp::run(&ir, input);
    Outcome { output: r.output, trap: r.trap }
}

fn stress_cfg() -> VmConfig {
    VmConfig { gc: GcConfig { stress: true, reuse_slots: false, ..GcConfig::default() }, ..VmConfig::default() }
}

#[test]
fn corpus_matches_the_interpreter_under_every_setting() {
    for (name, src, expected) in corpus() {
        let oracle = run_oracle(&src, &[]);
        if oracle.trap.is_none() {
            assert_eq!(oracle.output, expected, "{name}: the corpus expectation");
        }
        for (label, opts) in settings() {
            let got = run_vm(&src, &opts, &VmConfig::default(), &[]);
            assert_eq!(got.output, oracle.output, "{name} [{label}] output");
            assert_eq!(got.trap, oracle.trap, "{name} [{label}] trap");
        }
    }
}

#[test]
fn corpus_survives_a_collection_at_every_safe_point() {
    for (name, src, _) in corpus() {
        let oracle = run_oracle(&src, &[]);
        for (label, opts) in settings() {
            let got = run_vm(&src, &opts, &stress_cfg(), &[]);
            assert_eq!(got.output, oracle.output, "{name} [{label}, gc stress] output");
            assert_eq!(got.trap, oracle.trap, "{name} [{label}, gc stress] trap");
        }
    }
}

fn run_default(src: &str, input: &[&str]) -> Outcome {
    run_vm(src, &Options::default(), &VmConfig::default(), input)
}

#[test]
fn input_print_and_println_go_through_vmio() {
    let src = r#"
print("name? "); let name = input(); println("hello, " + name + "!");
let n = input().parse_i64(); println(n * 2);
let rest = input(); println(rest.len());
print(1); print(' '); print(2.5); println();
"#;
    let got = run_default(src, &["Ada", "21"]);
    assert_eq!(got.trap, None);
    assert_eq!(got.output, "name? hello, Ada!\n42\n0\n1 2.5\n", "EOF reads as an empty string");
    // the stress collector sees the same
    let got = run_vm(src, &Options::default(), &stress_cfg(), &["Ada", "21"]);
    assert_eq!(got.output, "name? hello, Ada!\n42\n0\n1 2.5\n");
}

#[test]
fn runtime_traps_surface_with_their_kind() {
    let cases: Vec<(&str, Trap)> = vec![
        ("let a = 1; let b = 0; println(a / b);", Trap::DivideByZero),
        ("let x: u8 = 255; println(x + 1);", Trap::Overflow),
        ("let a = [1, 2, 3]; let i = 7; println(a[i]);", Trap::IndexOutOfBounds { index: 7, len: 3 }),
        ("let a = [1, 2, 3]; let i = -1; println(a[i]);", Trap::IndexOutOfBounds { index: -1, len: 3 }),
        ("let m = {\"a\": 1}; println(m[\"zz\"]);", Trap::KeyNotFound),
        ("let mut a: [i64] = []; a.pop();", Trap::EmptyArray),
        ("panic(\"boom\");", Trap::Panic("boom".into())),
        ("assert(1 > 2, \"nope\");", Trap::Panic("assertion failed: nope".into())),
        ("let s = 70; println(1 << s);", Trap::ShiftOverflow),
        ("println(\"abc\".substring(2, 9));", Trap::IndexOutOfBounds { index: 9, len: 3 }),
    ];
    for (src, expected) in cases {
        let got = run_default(src, &[]);
        assert_eq!(got.trap, Some(expected), "{src}");
        let oracle = run_oracle(src, &[]);
        assert_eq!(got.trap, oracle.trap, "{src}");
    }
}

#[test]
fn output_before_a_trap_is_kept() {
    let got = run_default("println(1); println(2); let z = 0; println(5 / z); println(3);", &[]);
    assert_eq!(got.output, "1\n2\n");
    assert_eq!(got.trap, Some(Trap::DivideByZero));
}

#[test]
fn runaway_recursion_is_a_stack_overflow_not_a_crash() {
    let src = "fn f(n: i64) -> i64 { f(n + 1) + 1 }\nprintln(f(0));";
    let got = run_default(src, &[]);
    assert_eq!(got.trap, Some(Trap::StackOverflow));
    let cfg = VmConfig { max_frames: 50, ..VmConfig::default() };
    let got = run_vm(src, &Options::default(), &cfg, &[]);
    assert_eq!(got.trap, Some(Trap::StackOverflow));
}

#[test]
fn deep_but_finite_recursion_works() {
    let src = "fn sum(n: i64) -> i64 { if n == 0 { 0 } else { n + sum(n - 1) } }\nprintln(sum(5000));";
    let got = run_default(src, &[]);
    assert_eq!(got.trap, None);
    assert_eq!(got.output, "12502500\n");
}

#[test]
fn many_arguments_and_nested_calls_use_the_window_correctly() {
    let src = r#"
fn g(a: i64) -> i64 { a * 10 }
fn f(a: i64, b: i64, c: i64, d: i64, e: i64, f: i64, g: i64, h: i64) -> i64 { a + 2 * b + 3 * c + 4 * d + 5 * e + 6 * f + 7 * g + 8 * h }
fn swap(a: i64, b: i64) -> [i64] { [b, a] }
let x = 1; let y = 2;
println(f(g(1), g(2), 3, g(4), 5, g(6), g(7), g(8)));
println(f(x, y, x, y, x, y, x, y));
println(swap(x, y)); println(swap(y, x)); println(swap(swap(x, y)[0], swap(x, y)[1]));
"#;
    let got = run_default(src, &[]);
    assert_eq!(got.trap, None, "{}", got.output);
    let oracle = run_oracle(src, &[]);
    assert_eq!(got.output, oracle.output);
    assert_eq!(got.output.lines().next(), Some("1734"));
    let g2 = run_vm(src, &Options::default(), &stress_cfg(), &[]);
    assert_eq!(g2.output, oracle.output);
}

#[test]
fn garbage_heavy_programs_run_in_a_small_heap() {
    let src = r#"
let mut keep: [string] = [];
let mut total = 0;
for i in 0..3000 {
    let s = "item " + i.to_string() + " padding padding padding";
    let parts = s.split(" ");
    total += parts.len();
    if i % 500 == 0 { keep.push(s); }
}
println(total); println(keep.len()); println(keep[3]);
"#;
    let cfg = VmConfig {
        gc: GcConfig { limit: Some(48 * 1024), initial_threshold: 4096, ..GcConfig::default() },
        ..VmConfig::default()
    };
    let c = compile_source("main.neon", src, &MemVfs::new(), &Options::default());
    let bc = c.bytecode.unwrap();
    let mut io = BufferIo::new();
    let report = vm::run(&bc, &mut io, &cfg);
    assert!(report.result.is_ok(), "{:?}", report.result);
    assert_eq!(io.output, "15000\n6\nitem 1500 padding padding padding\n");
    assert!(report.gc.collections > 20, "the limit forced many collections: {:?}", report.gc);
    assert!(report.gc.peak_bytes <= 48 * 1024);
}

#[test]
fn live_data_that_exceeds_the_limit_is_out_of_memory() {
    let src = "let mut a: [string] = []; let mut i = 0; while true { a.push(\"some text to keep alive \" + i.to_string()); i += 1; }";
    let cfg = VmConfig { gc: GcConfig { limit: Some(64 * 1024), ..GcConfig::default() }, ..VmConfig::default() };
    let got = run_vm(src, &Options::default(), &cfg, &[]);
    assert_eq!(got.trap, Some(Trap::OutOfMemory));
}

#[test]
fn collections_and_closures_under_the_stress_collector() {
    let src = r#"
fn make_adders() -> [fn(i64) -> i64] {
    let mut out: [fn(i64) -> i64] = [];
    for i in 0..5 { let k = i * 100; out.push(|x| => x + k + i); }
    out
}
let adders = make_adders();
let mut m = { "a": [1, 2, 3], "b": [4] };
m["c"] = [5, 6];
let mut acc = 0;
for f in adders { acc += f(1); }
for k in m.keys() { for v in m[k] { acc += v; } }
let mut s = set { "x", "y" };
s.insert("z");
println(acc); println(s.len()); println(m); println(s);
"#;
    let oracle = run_oracle(src, &[]);
    assert_eq!(oracle.trap, None);
    for (label, opts) in settings() {
        let got = run_vm(src, &opts, &stress_cfg(), &[]);
        assert_eq!(got.output, oracle.output, "[{label}]");
    }
}
