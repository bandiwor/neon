//! Compile-time benchmark: source -> bytecode for several program shapes, with a fingerprint of the
//! bytecode so two revisions can be compared for speed *and* identical output
//! (`scripts/compare_compile.sh <revision>`; `NEON_THREADS=n` picks the thread count).
use neon::driver::{compile_source, Options};
use neon::vfs::MemVfs;
use std::time::Instant;

fn big(n: usize) -> String {
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
    s + "println(t);\n"
}

fn wide(n: usize) -> String {
    let mut s = String::from("fn main() {\n    var acc = 0;\n");
    for i in 0..n {
        s += &format!("    let v{i} = acc + {i};\n    if v{i} % 3 == 0 {{ acc += v{i}; }} else {{ acc -= 1; }}\n    println(f\"line {i}: {{acc}}\");\n");
    }
    s + "}\n"
}

fn loops(n: usize) -> String {
    let mut s = String::from("fn main() {\n    var t = 0;\n");
    for i in 0..n {
        s += &format!("    var w{i} = 0;\n    while w{i} < {} {{ t += w{i}; w{i} += 1; }}\n", i % 9 + 2);
    }
    s + "    println(t);\n}\n"
}

fn main() {
    if let Some(t) = std::env::var("NEON_THREADS").ok().and_then(|v| v.parse().ok()) { neon::par::set_threads(t); }
    let programs: Vec<(&str, String)> = vec![
        ("hello (prelude only)", "println(\"hi\");".to_string()),
        ("10 functions", big(10)),
        ("100 functions", big(100)),
        ("400 functions", big(400)),
        ("main with 250 statements", wide(250)),
        ("main with 1000 statements", wide(1000)),
        ("400 small loops in main", loops(400)),
    ];
    let vfs = MemVfs::new();
    for (name, src) in &programs {
        let mut best = f64::MAX;
        let runs = if src.len() > 100_000 { 5 } else { 15 };
        for _ in 0..runs {
            let t = Instant::now();
            let c = compile_source("p.neon", src, &vfs, &Options::default());
            assert!(!c.diags.has_errors(), "{}", c.render_diagnostics());
            std::hint::black_box(&c.bytecode);
            best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        }
        let c = compile_source("p.neon", src, &vfs, &Options::default());
        let text = neon::bytecode::disassemble(c.bytecode.as_ref().unwrap()).unwrap();
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        println!("{name:28} {best:9.2} ms   bytecode {:08x} ({} KiB of listing)", h.finish() as u32, text.len() / 1024);
    }
}
