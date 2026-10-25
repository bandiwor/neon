//! Benchmarks (std only, `harness = false`): `cargo bench` or
//! `cargo bench -- <filter>`; set `NEON_BENCH_QUICK=1` for a fast smoke run.
//!
//! Three groups:
//! * `run/*`     — programs on the VM (optimized and unoptimized) and on the
//!   IR reference interpreter, with VM instructions per second;
//! * `gc/*`      — allocation-heavy programs with and without a heap limit;
//! * `compile/*` — front end + IR + code generation throughput.

use std::time::{Duration, Instant};

use neon::compiler::CompileOptions;
use neon::driver::{compile_source, Options};
use neon::gc::GcConfig;
use neon::ir::{interp, OptConfig};
use neon::vfs::MemVfs;
use neon::vm::{self, BufferIo, VmConfig};

struct Workload {
    name: &'static str,
    source: String,
    /// Also run on the IR interpreter (it is much slower, so only for small ones).
    interp: bool,
    /// Heap limit for this workload, if it is about the collector.
    gc_limit: Option<usize>,
}

fn workloads(scale: u64) -> Vec<Workload> {
    let w = |name, source: String, interp, gc_limit| Workload { name, source, interp, gc_limit };
    vec![
        w("run/fib", format!("fn fib(n: i64) -> i64 {{ if n < 2 {{ n }} else {{ fib(n - 1) + fib(n - 2) }} }}\nprintln(fib({}));", 20 + scale.min(10)), true, None),
        w(
            "run/loop-sum",
            format!("fn sum(n: i64) -> i64 {{ var s = 0; var i = 0; while i < n {{ s += i % 7; i += 1; }} s }}\nprintln(sum({}));", 200_000 * scale),
            true,
            None,
        ),
        w(
            "run/sieve",
            format!(
                "fn count(limit: i64) -> i64 {{\n  var p: [bool] = [];\n  for i in 0..limit {{ p.push(i >= 2); }}\n  var i = 2;\n  while i * i < limit {{ if p[i] {{ var m = i * i; while m < limit {{ p[m] = false; m += i; }} }} i += 1; }}\n  var c = 0;\n  for i in 0..limit {{ if p[i] {{ c += 1; }} }}\n  c\n}}\nprintln(count({}));",
                20_000 * scale
            ),
            true,
            None,
        ),
        w(
            "run/float-loop",
            format!("fn f(n: i64) -> f64 {{ var x = 0.0; var i = 0; while i < n {{ x = x * 0.999 + (i as f64).sqrt(); i += 1; }} x }}\nprintln(f({}));", 100_000 * scale),
            true,
            None,
        ),
        w(
            "run/closures",
            format!("fn run(n: i64) -> i64 {{ var total = 0; let add = |k: i64| => {{ total += k; total }}; var i = 0; while i < n {{ add(i); i += 1; }} total }}\nprintln(run({}));", 100_000 * scale),
            true,
            None,
        ),
        w(
            "run/structs-enums",
            format!(
                "enum Shape {{ Circle(f64), Rect(f64, f64), Empty }}\nstruct P {{ x: f64, y: f64 }}\nfn area(s: Shape) -> f64 {{ match s {{ Shape::Circle(r) => 3.14 * r * r, Shape::Rect(w, h) => w * h, Shape::Empty => 0.0 }} }}\nfn run(n: i64) -> f64 {{ var t = 0.0; var i = 0; while i < n {{ let p = P {{ x: i as f64, y: 2.0 }}; let s = if i % 3 == 0 {{ Shape::Circle(p.y) }} elif i % 3 == 1 {{ Shape::Rect(p.x, p.y) }} else {{ Shape::Empty }}; t += area(s); i += 1; }} t }}\nprintln(run({}));",
                50_000 * scale
            ),
            true,
            None,
        ),
        w(
            "run/map-int",
            format!("fn run(n: i64) -> i64 {{ var m: {{i64: i64}} = {{}}; for i in 0..n {{ m[i] = i * 2; }} var s = 0; for i in 0..n {{ s += m[i]; }} s }}\nprintln(run({}));", 20_000 * scale),
            false,
            None,
        ),
        w(
            "run/strings",
            format!("fn run(n: i64) -> i64 {{ var total = 0; for i in 0..n {{ let s = \"item-\" + i.to_string() + \"-end\"; let parts = s.split(\"-\"); total += parts.len() + s.len(); }} total }}\nprintln(run({}));", 10_000 * scale),
            false,
            None,
        ),
        w(
            "gc/garbage",
            format!("fn run(n: i64) -> i64 {{ var keep = 0; for i in 0..n {{ let a = [i, i + 1, i + 2, i + 3]; let s = \"g\" + i.to_string(); keep += a.len() + s.len(); }} keep }}\nprintln(run({}));", 30_000 * scale),
            false,
            None,
        ),
        w(
            "gc/garbage-64k-limit",
            format!("fn run(n: i64) -> i64 {{ var keep = 0; for i in 0..n {{ let a = [i, i + 1, i + 2, i + 3]; let s = \"g\" + i.to_string(); keep += a.len() + s.len(); }} keep }}\nprintln(run({}));", 30_000 * scale),
            false,
            Some(64 * 1024),
        ),
        w(
            "gc/live-tree",
            format!(
                "enum T {{ Leaf, Node(T, i64, T) }}\nfn build(d: i64) -> T {{ if d == 0 {{ T::Leaf }} else {{ T::Node(build(d - 1), d, build(d - 1)) }} }}\nfn sum(t: T) -> i64 {{ match t {{ T::Leaf => 0, T::Node(l, v, r) => sum(l) + v + sum(r) }} }}\nfn run(rounds: i64) -> i64 {{ var s = 0; for _ in 0..rounds {{ s += sum(build(10)); }} s }}\nprintln(run({}));",
                2 * scale
            ),
            false,
            Some(512 * 1024),
        ),
    ]
}

/// Runs `f` repeatedly for about `budget`; returns the sorted per-run times.
fn sample(budget: Duration, min_runs: usize, mut f: impl FnMut()) -> Vec<Duration> {
    f(); // warm-up
    let mut times = Vec::new();
    let start = Instant::now();
    while times.len() < min_runs || (start.elapsed() < budget && times.len() < 200) {
        let t = Instant::now();
        f();
        times.push(t.elapsed());
    }
    times.sort();
    times
}

fn fmt_dur(d: Duration) -> String {
    let us = d.as_secs_f64() * 1e6;
    if us >= 1e6 {
        format!("{:.2} s", us / 1e6)
    } else if us >= 1e3 {
        format!("{:.2} ms", us / 1e3)
    } else {
        format!("{us:.1} µs")
    }
}

struct Stats {
    min: Duration,
    median: Duration,
    mean: Duration,
}

fn stats(times: &[Duration]) -> Stats {
    let total: Duration = times.iter().sum();
    Stats { min: times[0], median: times[times.len() / 2], mean: total / times.len() as u32 }
}

fn report(name: &str, label: &str, s: &Stats, extra: &str) {
    println!(
        "{name:<24} {label:<12} min {:>10}   median {:>10}   mean {:>10}  {extra}",
        fmt_dur(s.min),
        fmt_dur(s.median),
        fmt_dur(s.mean)
    );
}

fn opts(opt: bool) -> Options {
    if opt {
        Options::default()
    } else {
        Options { opt: OptConfig::none(), compile: CompileOptions { peephole: false } }
    }
}

fn bench_workload(w: &Workload, budget: Duration, min_runs: usize) {
    let vfs = MemVfs::new();
    let cfg = VmConfig {
        gc: GcConfig {
            limit: w.gc_limit,
            initial_threshold: if w.gc_limit.is_some() { 4096 } else { 1 << 20 },
            ..GcConfig::default()
        },
        ..VmConfig::default()
    };
    let mut expected = None;
    for (label, optimize) in [("vm", true), ("vm-noopt", false)] {
        let c = compile_source("bench.neon", &w.source, &vfs, &opts(optimize));
        assert!(!c.diags.has_errors(), "{}: {}", w.name, c.render_diagnostics());
        let bc = c.bytecode.unwrap();
        // one counted run for the instruction count and the output check
        let counted = {
            let mut io = BufferIo::new();
            let r = vm::run(&bc, &mut io, &VmConfig { max_steps: Some(u64::MAX), ..cfg });
            assert!(r.result.is_ok(), "{}: {:?}", w.name, r.result);
            match &expected {
                None => expected = Some(io.output.clone()),
                Some(e) => assert_eq!(*e, io.output, "{}: optimizer changed the output", w.name),
            }
            (r.steps, r.gc)
        };
        let times = sample(budget, min_runs, || {
            let mut io = BufferIo::new();
            let r = vm::run(&bc, &mut io, &cfg);
            assert!(r.result.is_ok());
            std::hint::black_box(&io.output);
        });
        let s = stats(&times);
        let mips = counted.0 as f64 / s.median.as_secs_f64() / 1e6;
        let extra = if w.gc_limit.is_some() || w.name.starts_with("gc/") {
            format!("{:.1} M insn/s, {} collections", mips, counted.1.collections)
        } else {
            format!("{mips:.1} M insn/s ({} insns)", counted.0)
        };
        report(w.name, label, &s, &extra);
    }
    if w.interp {
        let c = compile_source("bench.neon", &w.source, &vfs, &opts(false));
        let ir = c.ir.unwrap();
        let times = sample(budget, min_runs.min(3), || {
            let r = interp::run(&ir, &[]);
            assert!(r.trap.is_none());
            std::hint::black_box(&r.output);
        });
        report(w.name, "ir-interp", &stats(&times), "");
    }
}

/// A module-heavy program with `n` functions, for compile-time throughput.
fn big_program(n: usize) -> String {
    let mut s = String::new();
    for i in 0..n {
        s += &format!(
            "struct S{i} {{ a: i64, b: f64 }}\nfn work{i}(x: i64, y: f64) -> f64 {{\n    let s = S{i} {{ a: x * {i}, b: y + 1.5 }};\n    var acc = 0.0;\n    for k in 0..x {{ if k % 3 == 0 {{ acc += s.b * (k as f64); }} else {{ acc -= 1.0; }} }}\n    match s.a {{ 0 => acc, 1..=9 => acc + 1.0, _ => acc * 2.0 }}\n}}\n"
        );
    }
    s += "var t = 0.0;\n";
    for i in 0..n {
        s += &format!("t += work{i}({}, 1.0);\n", i % 7);
    }
    s += "println(t);\n";
    s
}

fn bench_compile(budget: Duration, min_runs: usize, filter: &dyn Fn(&str) -> bool) {
    let vfs = MemVfs::new();
    for (name, n) in [("compile/small", 10usize), ("compile/medium", 100), ("compile/large", 400)] {
        if !filter(name) {
            continue;
        }
        let src = big_program(n);
        let kb = src.len() as f64 / 1024.0;
        let times = sample(budget, min_runs.min(5), || {
            let c = compile_source("big.neon", &src, &vfs, &Options::default());
            assert!(!c.diags.has_errors(), "{}", c.render_diagnostics());
            std::hint::black_box(&c.bytecode);
        });
        let s = stats(&times);
        report(
            name,
            "source→bytecode",
            &s,
            &format!("{n} functions, {kb:.0} KiB, {:.0} KiB/s", kb / s.median.as_secs_f64()),
        );
    }
    // each phase on its own, for the large program
    if filter("compile/phases") {
        let src = big_program(200);
        let t = |f: &mut dyn FnMut()| stats(&sample(budget, min_runs.min(5), f));
        let lex = t(&mut || {
            let mut map = neon::diag::SourceMap::new();
            let file = map.add_file("big.neon", &src).unwrap();
            let mut interner = neon::syntax::Interner::new();
            let mut diags = neon::diag::DiagBag::new();
            std::hint::black_box(neon::syntax::parse_source(file, &src, &mut interner, &mut diags));
        });
        report("compile/phases", "parse", &lex, "200 functions");
        let whole = t(&mut || {
            std::hint::black_box(compile_source(
                "big.neon",
                &src,
                &vfs,
                &Options { opt: OptConfig::none(), compile: CompileOptions { peephole: false } },
            ));
        });
        report("compile/phases", "all, no opts", &whole, "");
        let whole = t(&mut || {
            std::hint::black_box(compile_source("big.neon", &src, &vfs, &Options::default()));
        });
        report("compile/phases", "all, optimized", &whole, "");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).filter(|a| !a.starts_with("--")).collect();
    let quick = std::env::var_os("NEON_BENCH_QUICK").is_some() || std::env::args().any(|a| a == "--quick");
    let (budget, min_runs, scale) =
        if quick { (Duration::from_millis(100), 2, 1) } else { (Duration::from_secs(2), 5, 5) };
    let filter = |name: &str| args.is_empty() || args.iter().any(|a| name.contains(a.as_str()));
    println!(
        "neon benchmarks ({}; {})",
        if quick { "quick" } else { "full" },
        if cfg!(debug_assertions) { "DEBUG build — use `cargo bench`" } else { "release build" }
    );
    println!("{:-<120}", "");
    for w in workloads(scale).iter().filter(|w| filter(w.name)) {
        bench_workload(w, budget, min_runs);
    }
    bench_compile(budget, min_runs, &filter);
}
