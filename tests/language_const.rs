//! `const` versus `let`: constants are computed by the compiler, inlined at
//! their uses, and checked at compile time; `let` globals are run-time values.

use neon::driver::{compile_source, Options};
use neon::vfs::MemVfs;
use neon::vm::{self, BufferIo, VmConfig};

fn compile(src: &str, vfs: &MemVfs) -> neon::driver::Compiled {
    compile_source("main.neon", src, vfs, &Options::default())
}

fn run(src: &str) -> String {
    let c = compile(src, &MemVfs::new());
    assert!(!c.diags.has_errors(), "{}", c.render_diagnostics());
    let mut io = BufferIo::new();
    let r = vm::run(&c.bytecode.unwrap(), &mut io, &VmConfig::default());
    assert!(r.result.is_ok(), "{:?}", r.result);
    io.output
}

fn errors(src: &str) -> String {
    let c = compile(src, &MemVfs::new());
    assert!(c.diags.has_errors(), "expected errors");
    c.render_diagnostics()
}

#[test]
fn constants_are_inlined_and_need_no_global_slot() {
    let src = "const LIMIT: i64 = 6 * 7;\nconst NAME = \"neon\" + \"!\";\nlet runtime = LIMIT + 1;\nprintln(LIMIT); println(NAME); println(runtime);";
    let c = compile(src, &MemVfs::new());
    assert!(!c.diags.has_errors(), "{}", c.render_diagnostics());
    let ir = c.ir.unwrap();
    let names: Vec<&str> = ir.globals.iter().map(|g| g.name.as_str()).collect();
    // constants never get a slot; the `let` may too be optimized away when nothing reads it later
    assert!(!names.contains(&"LIMIT") && !names.contains(&"NAME"), "{names:?}");
    assert!(names.len() <= 1, "{names:?}");
    assert_eq!(run(src), "42\nneon!\n43\n");
}

#[test]
fn constants_can_use_other_constants_casts_and_builtin_limits() {
    let out = run(
        "const A: u8 = 200;\nconst B = A as u32 + 100;\nconst C = i16::MAX as i64 * 2;\nconst D = (B > 250) && (C == 65534);\nconst E = -128i8;\n\
         println(B); println(C); println(D); println(E);",
    );
    assert_eq!(out, "300\n65534\ntrue\n-128\n");
}

#[test]
fn constants_are_shared_across_modules() {
    let vfs = MemVfs::new().with_file("cfg.neon", "export const WIDTH: i64 = 80;\nexport const DOUBLE = WIDTH * 2;");
    let c = compile("import { WIDTH, DOUBLE } from \"cfg\";\nprintln(WIDTH + DOUBLE);", &vfs);
    assert!(!c.diags.has_errors(), "{}", c.render_diagnostics());
    let mut io = BufferIo::new();
    vm::run(&c.bytecode.unwrap(), &mut io, &VmConfig::default()).result.unwrap();
    assert_eq!(io.output, "240\n");
}

#[test]
fn a_constant_that_cannot_be_computed_is_a_compile_error() {
    let text = errors("const BAD: i8 = 100 + 100;\nprintln(BAD);");
    assert!(text.contains("E0401") && text.contains("integer overflow") && text.contains("`BAD`"), "{text}");
    let text = errors("const ZERO = 1 / 0;");
    assert!(text.contains("division by zero"), "an unused constant is checked too: {text}");
}

#[test]
fn only_constant_expressions_are_allowed() {
    for (src, what) in [
        ("fn f() -> i64 { 3 }\nconst X = f();", "a call"),
        ("var n = 1;\nconst X = n + 1;", "a variable or function"),
        ("let n = 1;\nconst X = n;", "a variable or function"),
        ("const X = [1, 2];", "this expression"),
        ("const X = if true { 1 } else { 2 };", "this expression"),
    ] {
        let text = errors(src);
        assert!(text.contains("E0241") && text.contains(what), "{src}\n{text}");
    }
}

#[test]
fn let_is_a_runtime_value_so_it_may_call_functions() {
    let out = run("fn compute() -> i64 { 6 * 7 }\nlet answer = compute();\nlet items = [1, 2, 3];\nprintln(answer); println(items.len());");
    assert_eq!(out, "42\n3\n");
}
