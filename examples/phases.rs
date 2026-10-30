use neon::bytecode::disassemble;
use neon::compiler::{compile_program, CompileOptions};
use neon::driver::compile_ir_source;
use neon::ir::{optimize, OptConfig};
use neon::vfs::MemVfs;
use std::time::Instant;

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

fn main() {
    if let Some(t) = std::env::var("NEON_THREADS").ok().and_then(|v| v.parse().ok()) {
        neon::par::set_threads(t);
    }
    let n: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(400);
    let src = big_program(n);
    let vfs = MemVfs::new();
    let mut best = [f64::MAX; 4];
    let mut hash = 0u64;
    for _ in 0..7 {
        let t0 = Instant::now();
        let c = compile_ir_source("big.neon", &src, &vfs);
        let t1 = Instant::now();
        let mut ir = c.ir.unwrap();
        optimize(&mut ir, &OptConfig::default());
        let t2 = Instant::now();
        let bc = compile_program(&ir, &CompileOptions::default()).unwrap();
        let t3 = Instant::now();
        for (k, (a, b)) in [(t0, t1), (t1, t2), (t2, t3), (t0, t3)].iter().enumerate() {
            best[k] = best[k].min((*b - *a).as_secs_f64() * 1000.0);
        }
        let text = disassemble(&bc).unwrap();
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        hash = h.finish();
    }
    println!("frontend+lower {:7.2} ms | optimize {:7.2} ms | codegen {:7.2} ms | total {:7.2} ms | bytecode hash {hash:016x}", best[0], best[1], best[2], best[3]);
}
