//! Tuples: `(a, b)` is a one-variant enum of the prelude, so they are matched, printed and optimized
//! like any enum; these tests pin down the syntax, the errors and the optimizer's treatment.

use neon::driver::{compile_ir_source, compile_source, Options};
use neon::ir::{self, interp, Inst};
use neon::vfs::MemVfs;
use neon::vm::{self, BufferIo, VmConfig};

fn run(src: &str) -> String {
    let c = compile_source("main.neon", src, &MemVfs::new(), &Options::default());
    assert!(!c.diags.has_errors(), "{}", c.render_diagnostics());
    let mut io = BufferIo::new();
    let r = vm::run(&c.bytecode.unwrap(), &mut io, &VmConfig::default());
    assert!(r.result.is_ok(), "{:?}", r.result);
    io.output
}

fn errors(src: &str) -> String {
    let c = compile_source("main.neon", src, &MemVfs::new(), &Options::default());
    assert!(c.diags.has_errors(), "expected errors for {src}");
    c.render_diagnostics()
}

fn optimized(src: &str) -> ir::IrProgram {
    let c = compile_ir_source("main.neon", src, &MemVfs::new());
    assert!(!c.diags.has_errors(), "{}", c.render_diagnostics());
    let mut p = c.ir.unwrap();
    ir::optimize(&mut p, &ir::OptConfig::default());
    ir::verify_program(&p).unwrap();
    p
}

fn allocations_in(p: &ir::IrProgram, func: &str) -> usize {
    let f = p.funcs.iter().find(|f| f.name == func).unwrap_or_else(|| panic!("no function {func}"));
    f.blocks
        .iter()
        .flat_map(|b| &b.insts)
        .filter(|i| matches!(i, Inst::NewEnum { .. } | Inst::NewStruct { .. }))
        .count()
}

#[test]
fn syntax_and_types() {
    assert_eq!(run("println((1, \"a\", 2.5));"), "(1, \"a\", 2.5)\n");
    assert_eq!(run("let t: (i64, (bool, char)) = (1, (true, 'x'));\nprintln(t.1.0);\nprintln(t.1.1);"), "true\nx\n");
    assert_eq!(run("fn f() -> (i64, i64) { (1, 2) }\nprintln(f().1);"), "2\n");
    assert_eq!(run("println((1, 2,));"), "(1, 2)\n", "a trailing comma is fine");
    assert_eq!(run("println((1 + 2));"), "3\n", "parentheses alone are not a tuple");
    assert_eq!(run("let xs = [(1, 2), (3, 4)];\nprintln(xs[1].0);\nprintln(xs);"), "3\n[(1, 2), (3, 4)]\n");
    assert_eq!(run("println(Option::Some((1, 2)));"), "Option::Some((1, 2))\n");
    assert_eq!(run("let t = (1, \"x\");\nprintln(f\"<{t}>\");"), "<(1, \"x\")>\n");
}

#[test]
fn patterns() {
    let src = "fn f(t: (i64, i64)) -> i64 { match t { (0, y) => y, (x, 0) => x, (x, y) => x * y } }\n\
               println(f((0, 5))); println(f((4, 0))); println(f((3, 3)));";
    assert_eq!(run(src), "5\n4\n9\n");
    assert_eq!(run("for (a, b) in [(1, 2), (3, 4)] { println(a + b); }"), "3\n7\n");
    assert_eq!(run("fn g() { let ((a, b), c) = ((1, 2), 3); println(a + b + c); }\ng();"), "6\n");
    // exhaustiveness sees through tuples
    let e = errors("fn f(t: (bool, bool)) -> i64 { match t { (true, _) => 1, (false, true) => 2 } }");
    assert!(e.contains("E0240") && e.contains("(false, false)") || e.contains("Tuple2::T(false, false)"), "{e}");
}

#[test]
fn errors_are_reported_in_tuple_syntax() {
    let e = errors("let t = (1, 2);\nlet x: string = t;");
    assert!(e.contains("expected `string`, found `({integer}, {integer})`"), "{e}");
    let e = errors("let t = (1, 2);\nprintln(t.2);");
    assert!(e.contains("has 2 elements"), "{e}");
    let e = errors("var t = (1, 2);\nt.0 = 5;");
    assert!(e.contains("cannot assign to an element of a tuple"), "{e}");
    let e = errors("let t = (1, 2, 3, 4, 5, 6, 7, 8, 9);");
    assert!(e.contains("at most 8"), "{e}");
    let e = errors("fn f() { let (a,) = (1, 2); }");
    assert!(e.contains("at least two"), "{e}");
    let e = errors("let (a, b) = (1, 2);");
    assert!(e.contains("E0230"), "top-level lets still bind one name: {e}");
}

#[test]
fn tuples_that_do_not_escape_cost_nothing() {
    let p = optimized(
        "fn f(a: i64, b: i64) -> i64 { let (x, y) = (a + 1, b * 2); x + y }\n\
         fn minmax(a: i64, b: i64) -> (i64, i64) { if a < b { (a, b) } else { (b, a) } }\n\
         fn g(a: i64, b: i64) -> i64 { let (lo, hi) = minmax(a, b); hi - lo }\n\
         fn main() { let n = input().parse_i64(); println(f(n, 2)); println(g(n, 7)); }",
    );
    // everything is inlined into the entry; neither the literal pair nor the merged result is allocated
    let total: usize = p.funcs.iter().map(|f| allocations_in(&p, &f.name)).sum();
    assert_eq!(total, 0, "{p}");
    let r = interp::run(&p, &["7"]);
    assert_eq!(r.output, "12\n0\n");
}

#[test]
fn optional_values_merging_at_a_join_are_not_allocated_either() {
    let p = optimized(
        "fn main() { let n = input().parse_i64();\n\
         let o = if n > 3 { Option::Some(n * 2) } else { Option::None };\n\
         match o { Option::Some(v) => println(v), Option::None => println(\"none\") } }",
    );
    let total: usize = p.funcs.iter().map(|f| allocations_in(&p, &f.name)).sum();
    assert_eq!(total, 0, "{p}");
    assert_eq!(interp::run(&p, &["5"]).output, "10\n");
    assert_eq!(interp::run(&p, &["1"]).output, "none\n");
}
