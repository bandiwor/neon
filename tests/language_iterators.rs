//! Ranges as values and lazy iterator chains: what they compute, that they compile to plain loops
//! (no arrays between the steps, no closure objects), and what they refuse.

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

/// (allocations, closure objects and calls) in the whole program.
fn costs(p: &ir::IrProgram) -> (usize, usize) {
    let insts = p.funcs.iter().flat_map(|f| &f.blocks).flat_map(|b| &b.insts);
    let mut alloc = 0;
    let mut closures = 0;
    for i in insts {
        match i {
            Inst::NewStruct { .. }
            | Inst::NewEnum { .. }
            | Inst::NewArray { .. }
            | Inst::NewMap { .. }
            | Inst::NewSet { .. } => alloc += 1,
            Inst::MakeClosure { .. } | Inst::CallClosure { .. } => closures += 1,
            _ => {}
        }
    }
    (alloc, closures)
}

#[test]
fn a_chain_is_one_loop_without_allocations_or_closures() {
    let p = optimized(
        "fn main() { let n = input().parse_i64();\n\
         println((0..n).filter(|x| => x % 3 == 0).map(|x| => x * x).sum());\n\
         var total = 0; [1, 2, 3].iter().for_each(|x| => total += x * n); println(total); }",
    );
    // the array literal of the second line is the only thing built
    let (alloc, closures) = costs(&p);
    assert_eq!((alloc, closures), (1, 0), "{p}");
    assert_eq!(interp::run(&p, &["10"]).output, "126\n60\n");
}

#[test]
fn chains_over_arrays_do_not_copy_them() {
    let p = optimized(
        "fn main() { let xs = [5, 3, 8, 1]; let n = input().parse_i64();\n\
         println(xs.iter().map(|x| => x + n).filter(|x| => x > 6).count()); }",
    );
    let (alloc, closures) = costs(&p);
    assert_eq!((alloc, closures), (1, 0), "only the literal is built: {p}");
    assert_eq!(interp::run(&p, &["2"]).output, "2\n");
}

#[test]
fn laziness_follows_rust_for_side_effects() {
    // `take` does not pull one element too many; `inspect` sees exactly what flows through
    assert_eq!(run("println([1, 2, 3, 4].iter().inspect(|x| => print(f\"<{x}>\")).take(2).count());"), "<1><2>2\n");
    assert_eq!(run("println((1..10).inspect(|x| => print(f\"<{x}>\")).any(|x| => x == 3));"), "<1><2><3>true\n");
    // a chain bound to a variable is evaluated where it is used, every time
    assert_eq!(
        run("fn main() { var n = 0; let it = [1, 2, 3].iter().map(|x| => { n += 1; x }); it.count(); it.count(); println(n); }"),
        "6\n"
    );
    // closures see the variables as they are when the chain runs
    assert_eq!(
        run("fn main() { var k = 1; let it = [1, 2].iter().map(|x| => x * k); k = 10; println(it.sum()); }"),
        "30\n"
    );
}

#[test]
fn return_and_question_mark_leave_only_the_closure() {
    let src = "fn first_neg(xs: [i64]) -> Option<i64> { xs.iter().find(|x| => x < 0) }\n\
               fn f() -> i64 { let n = [1, 5, 9].iter().map(|x| => { if x > 4 { return 0; } x }).sum(); n + 100 }\n\
               fn g(xs: [string]) -> Result<i64, string> {\n\
                   let ok = xs.iter().map(|s| => s.try_parse_i64()).collect();\n\
                   Result::Ok(ok.len())\n\
               }\n\
               println(first_neg([1, -2, 3])); println(f()); println(g([\"1\", \"x\"]));";
    assert_eq!(run(src), "Option::Some(-2)\n101\nResult::Ok(2)\n");
}

#[test]
fn ranges_are_values() {
    assert_eq!(
        run("let r = 2..6; println(r.contains(5)); println(r.contains(6)); println(r.start + r.end);"),
        "true\nfalse\n8\n"
    );
    assert_eq!(
        run("let r = 1..=3; println(r.contains(3)); println(r.map(|x| => x * 2).collect());"),
        "true\n[2, 4, 6]\n"
    );
    assert_eq!(run("fn len(r: Range<i64>) -> i64 { r.end - r.start } println(len(3..10));"), "7\n");
    assert_eq!(run("println((250u8..).take(3).collect()); println((255u8..).count());"), "[250, 251, 252]\n1\n");
    assert_eq!(run("for i in (0..6).rev().step_by(2) { print(i); }"), "531");
    assert_eq!(run("let r = 0..3; for i in r { print(i); } for i in r { print(i); }"), "012012");
    assert_eq!(
        run("println((10..0).count()); println((0..=0).count()); println((i64::MAX - 1..=i64::MAX).count());"),
        "0\n1\n2\n"
    );
}

#[test]
fn zip_chain_flat_map_and_friends() {
    assert_eq!(run("println([1, 2, 3].iter().zip(10..).map(|p| => p.0 * p.1).collect());"), "[10, 22, 36]\n");
    assert_eq!(run("println((1..3).chain(7..9).chain(0..1).collect());"), "[1, 2, 7, 8, 0]\n");
    assert_eq!(run("println((1..4).flat_map(|x| => [x, x * 10]).take(5).collect());"), "[1, 10, 2, 20, 3]\n");
    assert_eq!(run("println((1..=4).skip_while(|x| => x < 3).collect());"), "[3, 4]\n");
    assert_eq!(run("println([3, 1, 2].iter().enumerate().max_by_key(|p| => p.1));"), "Option::Some((0, 3))\n");
    assert_eq!(run("println(([\"b\", \"a\"].iter().min()));"), "Option::Some(\"a\")\n");
    assert_eq!(
        run("fn main() { println([2, 3].iter().reduce(|a, b| => a * b)); let e: [i64] = []; println(e.iter().reduce(|a, b| => a * b)); }"),
        "Option::Some(6)\nOption::None\n"
    );
}

#[test]
fn what_cannot_be_fused_is_refused_with_a_location() {
    for (src, needle) in [
        ("fn f(it: Iter<i64>) -> i64 { 1 } println(f([1].iter()));", "lazy view"),
        ("fn f() -> Iter<i64> { [1].iter() } f();", "lazy view"),
        ("[1, 2].iter().map(|x| => x);", "lazy view"),
        ("let v = [[1].iter()];", "lazy view"),
        ("println([1, 2, 3].iter().take(2).rev().collect());", "`rev()` must come right after"),
        ("println([1].iter().zip([2].iter().filter(|x| => x > 0)).count());", "right side of `zip`"),
        ("println((0.5..2.5).iter().count());", "only ranges of integers"),
        ("let r = ..5;", "needs a start"),
    ] {
        let e = errors(src);
        assert!(e.contains(needle), "{src}: {e}");
        assert!(e.contains("main.neon:1:"), "has a location: {e}");
    }
}
