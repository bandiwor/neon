//! Phase 5 end-to-end: source -> IR -> optimized IR, executed by the
//! reference interpreter. Optimization must never change observable behavior.

use neon::driver::compile_ir;
use neon::ir::{interp, optimize, verify_program, IrProgram, OptConfig};
use neon::runtime::Trap;
use neon::vfs::MemVfs;

fn build(vfs: &MemVfs) -> IrProgram {
    let c = compile_ir("main.neon", vfs);
    assert!(!c.diags.has_errors(), "{}", c.render_diagnostics());
    let ir = c.ir.unwrap();
    verify_program(&ir).unwrap();
    ir
}

fn both(ir: &IrProgram, input: &[&str]) -> interp::RunResult {
    let plain = interp::run(ir, input);
    let mut opt = ir.clone();
    optimize(&mut opt, &OptConfig::default());
    verify_program(&opt).unwrap_or_else(|e| panic!("{e}\n{opt}"));
    let fast = interp::run(&opt, input);
    assert_eq!(plain.output, fast.output);
    assert_eq!(plain.trap, fast.trap);
    plain
}

#[test]
fn multi_module_program_with_generics_closures_and_traits() {
    let vfs = MemVfs::new()
        .with_file(
            "main.neon",
            r#"
import { Stack, Shape, area } from "lib/data";
import "lib/data" as data;

let mut s = Stack::new();
for i in 1..=5 { s.push(i * i); }
let mut total = 0;
while s.size() > 0 { total += s.pop(); }
println(total);

let shapes = [Shape::Circle(1.0), Shape::Rect(2.0, 3.0), Shape::Empty];
let mut sum = 0.0;
for sh in shapes { sum += area(sh); }
println(sum);

let adders = [1, 2, 3].len();
let mk = |n: i64| => { let mut acc = n; || => { acc += n; acc } };
let a = mk(5); a(); println(a());
println(data::answer());
"#,
        )
        .with_file(
            "lib/data.neon",
            r#"
export struct Stack<T> { items: [T] }
impl<T> Stack<T> {
    fn new() -> Stack<T> { Stack { items: [] } }
    fn push(mut self, x: T) { self.items.push(x); }
    fn pop(mut self) -> T { self.items.pop() }
    fn size(self) -> i64 { self.items.len() }
}
export enum Shape { Circle(f64), Rect(f64, f64), Empty }
export fn area(s: Shape) -> f64 {
    match s { Shape::Circle(r) => 3.0 * r * r, Shape::Rect(w, h) => w * h, Shape::Empty => 0.0 }
}
export fn answer() -> i32 { 42 }
"#,
        );
    let ir = build(&vfs);
    let r = both(&ir, &[]);
    assert!(r.trap.is_none(), "{:?}", r.trap);
    assert_eq!(r.output, "55\n9.0\n15\n42\n");
}

#[test]
fn allocation_heavy_program_runs_identically_when_optimized() {
    let vfs = MemVfs::new().with_file(
        "main.neon",
        r#"
struct Node { value: i64, next: [Node] }
fn build(n: i64) -> [Node] {
    let mut out = [];
    let mut i = 0;
    while i < n { out.push(Node { value: i, next: [] }); i += 1; }
    out
}
fn total(nodes: [Node]) -> i64 { let mut t = 0; for n in nodes { t += n.value; } t }
let mut grand = 0;
for round in 0..50 { grand += total(build(100)); }
println(grand);
"#,
    );
    let r = both(&build(&vfs), &[]);
    assert_eq!(r.output, "247500\n");
}

#[test]
fn trap_behaviour_is_preserved_by_optimization() {
    let vfs = MemVfs::new()
        .with_file("main.neon", "fn f(n: i32) -> i32 { if n == 0 { 100 / n } else { f(n - 1) } }\nprintln(f(3));");
    let r = both(&build(&vfs), &[]);
    assert_eq!(r.trap, Some(Trap::DivideByZero));
}

#[test]
fn optimizer_statistics() {
    let vfs = MemVfs::new().with_file(
        "main.neon",
        "fn sq(x: i32) -> i32 { x * x }\nfn go() -> i32 { sq(3) + sq(4) + 2 * 5 }\nprintln(go());",
    );
    let mut ir = build(&vfs);
    let before: usize = ir.funcs.iter().map(|f| f.size()).sum();
    let stats = optimize(&mut ir, &OptConfig::default());
    let after: usize = ir.funcs.iter().map(|f| f.size()).sum();
    assert!(stats.inlined_calls >= 2 && stats.folded > 0 && stats.removed_funcs > 0, "{stats:?}");
    assert!(after < before);
    // 3*3 + 4*4 + 10 folds all the way to 35
    assert_eq!(interp::run(&ir, &[]).output, "35\n");
    assert_eq!(ir.funcs.len(), 1);
}
