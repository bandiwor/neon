//! Compile-time benchmark of the type system: many impls, many instantiations, deep generic types.
//! `cargo run --release --example generics_bench [which]`
use neon::driver::compile_ir_source;
use neon::vfs::MemVfs;
use std::time::Instant;

/// `n` structs with two traits and inherent methods each; every one is used through a generic function.
fn impls(n: usize) -> String {
    let mut s = String::from("trait Area { fn area(self) -> i64; }\ntrait Scale { fn scale(self, k: i64) -> Self; }\n");
    for i in 0..n {
        s += &format!(
            "struct S{i} {{ a: i64, b: i64 }}\nimpl Area for S{i} {{ fn area(self) -> i64 {{ self.a * self.b + {i} }} }}\nimpl Scale for S{i} {{ fn scale(self, k: i64) -> S{i} {{ S{i} {{ a: self.a * k, b: self.b * k }} }} }}\nimpl S{i} {{ fn sum(self) -> i64 {{ self.a + self.b }} fn make(a: i64) -> S{i} {{ S{i} {{ a: a, b: a + 1 }} }} }}\n"
        );
    }
    s += "fn total<T: Area + Scale>(x: T) -> i64 { x.scale(2).area() + x.area() }\nfn main() {\n    var t = 0;\n";
    for i in 0..n {
        s += &format!("    let v{i} = S{i}::make({i});\n    t += total(v{i}) + v{i}.sum() + v{i}.area();\n");
    }
    s + "    println(t);\n}\n"
}

/// One generic function instantiated at `n` different nested types.
fn nested(n: usize) -> String {
    let mut s = String::from(
        "fn wrap<T>(x: T) -> [T] { [x] }\nfn size<T>(x: [T]) -> i64 { x.len() }\nfn main() {\n    var t = 0;\n",
    );
    for i in 0..n {
        let mut e = format!("{i}");
        for _ in 0..(i % 6) {
            e = format!("wrap({e})");
        }
        s += &format!("    t += size(wrap({e}));\n");
    }
    s + "    println(t);\n}\n"
}

/// A chain of blanket impls: `[[[...[i64]...]]]` shows through `n` levels.
fn blanket(n: usize) -> String {
    let mut s = String::from("trait Show { fn show(self) -> string; }\nimpl Show for i64 { fn show(self) -> string { \"i\" } }\nimpl<T: Show> Show for [T] { fn show(self) -> string { \"[\" + self.map(|x| => x.show()).join(\",\") + \"]\" } }\nimpl<T: Show> Show for Option<T> { fn show(self) -> string { match self { Option::Some(v) => v.show(), Option::None => \"-\" } } }\nfn main() {\n");
    for i in 0..n {
        let mut e = "1".to_string();
        for k in 0..(i % 12) {
            e = if k % 2 == 0 { format!("[{e}]") } else { format!("Option::Some({e})") };
        }
        s += &format!("    println({e}.show());\n");
    }
    s + "}\n"
}

fn main() {
    let which = std::env::args().nth(1).unwrap_or_default();
    let programs: Vec<(String, String)> = vec![
        ("impls 100".into(), impls(100)),
        ("impls 400".into(), impls(400)),
        ("impls 1600".into(), impls(1600)),
        ("nested 300".into(), nested(300)),
        ("nested 1200".into(), nested(1200)),
        ("blanket 100".into(), blanket(100)),
        ("blanket 400".into(), blanket(400)),
    ];
    let vfs = MemVfs::new();
    for (name, src) in programs.iter().filter(|(n, _)| which.is_empty() || n.starts_with(&which)) {
        let mut best = f64::MAX;
        for _ in 0..3 {
            let t = Instant::now();
            let c = compile_ir_source("p.neon", src, &vfs);
            assert!(!c.diags.has_errors(), "{name}: {}", c.render_diagnostics());
            std::hint::black_box(&c.ir);
            best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        }
        println!("{name:<14} {:>8.1} KB source  {best:>9.1} ms  (sema + IR lowering)", src.len() as f64 / 1024.0);
    }
}
