//! Format strings (`f"..."`) and format specifications: what they accept and what they report.

use neon::driver::{compile_source, Options};
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

#[test]
fn fields_are_evaluated_left_to_right_once() {
    let src = "var n = 0;\nfn next() -> i64 { n += 1; n }\nprintln(f\"{next()} {next()} {n}\");";
    assert_eq!(run(src), "1 2 2\n");
}

#[test]
fn a_field_can_hold_any_expression() {
    assert_eq!(run("let xs = [1, 2, 3];\nprintln(f\"{xs.len()} {xs[1] * 10} {if xs.len() > 2 { \"big\" } else { \"small\" }}\");"), "3 20 big\n");
    // `::` paths and the colon of a specifier are told apart
    assert_eq!(run("println(f\"{Option::Some(1)} {7:>3}\");"), "Option::Some(1)   7\n");
    // braces inside the expression
    assert_eq!(run("println(f\"{ { let a = 2; a * a } }\");"), "4\n");
}

#[test]
fn escapes_and_doubled_braces() {
    assert_eq!(run("println(f\"a\\tb {{}} \\\"q\\\" {1}\");"), "a\tb {} \"q\" 1\n");
}

#[test]
fn specifiers_with_fields_are_built_at_run_time() {
    assert_eq!(run("let w = 6;\nprintln(f\"[{1.5:{w}.2f}] [{\"x\":-<{w}}]\");"), "[  1.50] [x-----]\n");
}

#[test]
fn bad_specifiers_are_compile_errors() {
    assert!(errors("println(f\"{1:s}\");").contains("unknown format code `s`"));
    assert!(errors("println(f\"{\"a\":d}\");").contains("for a string"));
    assert!(errors("println(f\"{1.5:x}\");").contains("for a float"));
    assert!(errors("println(f\"{[1]:>5}\");").contains("cannot take a format specifier"));
    assert!(errors("println(f\"{1:5.}\");").contains("precision"));
}

#[test]
fn bad_format_strings_are_reported() {
    assert!(errors("println(f\"{}\");").contains("empty expression"));
    assert!(errors("println(f\"a } b\");").contains("single `}`"));
    assert!(errors("println(f\"{1\");").contains("unterminated"));
    assert!(errors("println(f\"{nope}\");").contains("nope"));
}

#[test]
fn a_bad_runtime_specifier_traps() {
    let c = compile_source(
        "main.neon",
        "let spec = \"s\";\nprintln(f\"{1:{spec}}\");",
        &MemVfs::new(),
        &Options::default(),
    );
    assert!(!c.diags.has_errors(), "{}", c.render_diagnostics());
    let mut io = BufferIo::new();
    let r = vm::run(&c.bytecode.unwrap(), &mut io, &VmConfig::default());
    assert!(format!("{:?}", r.result).contains("invalid format specifier"), "{:?}", r.result);
}

#[test]
fn constant_format_strings_fold_at_compile_time() {
    let c = compile_source("main.neon", "println(f\"{1 + 2}:{\"x\":>3}\");", &MemVfs::new(), &Options::default());
    assert!(!c.diags.has_errors(), "{}", c.render_diagnostics());
    let ir = c.ir.unwrap();
    let text = format!("{ir}");
    assert!(!text.contains("Format") && !text.contains("concat"), "{text}");
}
