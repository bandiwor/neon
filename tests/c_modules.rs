//! C modules: `import "file.c";` plus `extern fn` declarations. The C file becomes part of the translation
//! unit, so the C compiler checks every call; the VM, which cannot run C, reports it. (Skipped without a
//! C compiler.)

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn neon() -> Command {
    Command::new(env!("CARGO_BIN_EXE_neon"))
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("neon-cmod-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn c_compiler() -> Option<String> {
    let candidates = std::env::var("CC").into_iter().chain(["cc", "gcc", "clang"].map(String::from));
    candidates.into_iter().find(|c| {
        Command::new(c).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
    })
}

struct Ran {
    code: i32,
    out: String,
    err: String,
}

fn run(program: &mut Command) -> Ran {
    let mut child = program.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"").unwrap();
    let o = child.wait_with_output().unwrap();
    Ran {
        code: o.status.code().unwrap_or(-1),
        out: String::from_utf8_lossy(&o.stdout).into_owned(),
        err: String::from_utf8_lossy(&o.stderr).into_owned(),
    }
}

/// Writes the files, translates `prog.neon`, and tries to build the result: the C compiler's output on failure.
fn build(cc: &str, dir: &Path, files: &[(&str, &str)], flags: &[&str]) -> Result<PathBuf, String> {
    for (name, text) in files {
        std::fs::write(dir.join(name), text).unwrap();
    }
    let c = dir.join("prog.c");
    let t = neon().arg("c").arg(dir.join("prog.neon")).arg("-o").arg(&c).output().unwrap();
    if !t.status.success() {
        return Err(String::from_utf8_lossy(&t.stderr).into_owned());
    }
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime");
    let exe = dir.join("prog");
    let o = Command::new(cc)
        .args(["-std=c99", "-pedantic", "-Wall", "-Wextra", "-Werror", "-O1"])
        .args(flags)
        .arg("-I")
        .arg(&runtime)
        .arg("-I")
        .arg(dir)
        .arg(&c)
        .arg(runtime.join("neon.c"))
        .arg("-o")
        .arg(&exe)
        .arg("-lm")
        .output()
        .unwrap();
    if o.status.success() {
        Ok(exe)
    } else {
        Err(String::from_utf8_lossy(&o.stderr).into_owned())
    }
}

const MATHLIB: &str = r#"
#include "neon.h"
#include <ctype.h>

static i64 calls;

i64 add(i64 a, i64 b) { calls++; return a + b; }
f64 hypot2(f64 x, f64 y) { return sqrt(x * x + y * y); }
i32 clamp32(i32 v, i32 lo, i32 hi) { return v < lo ? lo : v > hi ? hi : v; }
u8 low_byte(u64 v) { return (u8)(v & 0xFF); }
bool is_even(i64 n) { return n % 2 == 0; }
chr next_char(chr c) { return c + 1; }
i64 call_count(void) { return calls; }
void note(i64 n) { printf("[C] note %lld\n", (long long)n); }
i64 length(const char *s) { return (i64)strlen(s); }
const char *shout(const char *text) {
    static char buf[64];
    size_t i, n = strlen(text);
    if (n > 60) n = 60;
    for (i = 0; i < n; i++) buf[i] = (char)toupper((unsigned char)text[i]);
    buf[n] = 0;
    return buf;
}
const char *nothing(void) { return NULL; }
const char *broken(void) { return "ok \xff\xfe bad \xc3 end"; }
"#;

const PROGRAM: &str = r#"
import "mathlib.c";

extern fn add(a: i64, b: i64) -> i64;
extern fn hypot2(x: f64, y: f64) -> f64;
extern fn clamp32(v: i32, lo: i32, hi: i32) -> i32;
extern fn low_byte(v: u64) -> u8;
extern fn is_even(n: i64) -> bool;
extern fn next_char(c: char) -> char;
extern fn call_count() -> i64;
extern fn note(n: i64);
extern fn length(s: string) -> i64;
extern fn shout(text: string) -> string;

fn main() {
    println(add(2, 40));
    println(hypot2(3.0, 4.0));
    println(clamp32(500, -10, 100));
    println(low_byte(0x1234));
    println(is_even(10));
    println(next_char('a'));
    println(shout("hello, мир"));
    println(length("мир"));
    note(7);
    var total = 0;
    for i in 0..1000 { total += add(i, 1); }
    println(total);
    println(call_count());
}
"#;

const EXPECTED: &str = "42\n5.0\n100\n52\ntrue\nb\nHELLO, мир\n6\n[C] note 7\n500500\n1001\n";

#[test]
fn c_functions_are_called_from_the_translation() {
    let Some(cc) = c_compiler() else { return };
    let dir = scratch("basic");
    // optimized, unoptimized, release, and with the collector at every safe-point
    for (i, flags) in [&[][..], &["-DNDEBUG"], &["-DNEON_GC_STRESS"]].into_iter().enumerate() {
        let exe = build(&cc, &dir, &[("prog.neon", PROGRAM), ("mathlib.c", MATHLIB)], flags)
            .unwrap_or_else(|e| panic!("{e}"));
        let r = run(&mut Command::new(exe));
        assert_eq!((r.code, r.out.as_str()), (0, EXPECTED), "{flags:?} ({i}): {}", r.err);
    }
    // the same with the IR optimizer off (nothing may inline or drop a call to C)
    std::fs::write(dir.join("prog.neon"), PROGRAM).unwrap();
    let t = neon().arg("c").arg("--no-opt").arg(dir.join("prog.neon")).output().unwrap();
    assert!(t.status.success());
    // the generated file includes the C module and checks its declarations against the definitions
    let c = std::fs::read_to_string(dir.join("prog.c")).unwrap();
    assert!(c.contains("#include \"mathlib.c\"") && c.contains("i64 add(i64 a, i64 b);"), "{c}");
}

#[test]
fn the_vm_reports_that_c_functions_only_exist_in_the_translation() {
    let dir = scratch("vm");
    std::fs::write(dir.join("prog.neon"), PROGRAM).unwrap();
    std::fs::write(dir.join("mathlib.c"), MATHLIB).unwrap();
    // `check` is fine with it
    let ok = neon().arg("check").arg(dir.join("prog.neon")).output().unwrap();
    assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));
    let r = run(neon().arg("run").arg(dir.join("prog.neon")));
    assert_eq!(r.code, 2, "{}", r.err);
    assert!(r.err.contains("`add` is an extern function") && r.err.contains("neon c"), "{}", r.err);
}

#[test]
fn a_declaration_that_disagrees_with_the_c_definition_does_not_compile() {
    let Some(cc) = c_compiler() else { return };
    let dir = scratch("mismatch");
    let prog = "import \"m.c\";\nextern fn add(a: i64, b: i64) -> i64;\nfn main() { println(add(1, 2)); }\n";
    // right
    assert!(build(
        &cc,
        &dir,
        &[("prog.neon", prog), ("m.c", "#include \"neon.h\"\ni64 add(i64 a, i64 b) { return a + b; }\n")],
        &[]
    )
    .is_ok());
    // a `static` definition is fine: the declaration takes its linkage, and only the module's own file sees it
    let st = "#include \"neon.h\"\nstatic i64 add(i64 a, i64 b) { return a + b; }\n";
    assert!(build(&cc, &dir, &[("prog.neon", prog), ("m.c", st)], &[]).is_ok());
    // wrong parameter type, wrong result type, wrong arity, missing definition: all errors, never a silent ABI mismatch
    for (why, c) in [
        ("parameter", "#include \"neon.h\"\ni64 add(i32 a, i64 b) { return a + b; }\n"),
        ("result", "#include \"neon.h\"\nf64 add(i64 a, i64 b) { return (f64)(a + b); }\n"),
        ("arity", "#include \"neon.h\"\ni64 add(i64 a) { return a; }\n"),
        ("missing", "#include \"neon.h\"\n"),
    ] {
        let r = build(&cc, &dir, &[("prog.neon", prog), ("m.c", c)], &[]);
        assert!(r.is_err(), "{why}: a mismatch must not build");
    }
}

#[test]
fn strings_from_c_are_checked_and_made_valid() {
    let Some(cc) = c_compiler() else { return };
    let dir = scratch("strings");
    let prog = "import \"mathlib.c\";\nextern fn broken() -> string;\nextern fn nothing() -> string;\nextern fn length(s: string) -> i64;\n\
                fn main() { let b = broken(); println(b); println(b.len()); println(length(b)); println(nothing()); }\n";
    let exe = build(&cc, &dir, &[("prog.neon", prog), ("mathlib.c", MATHLIB)], &[]).unwrap_or_else(|e| panic!("{e}"));
    let r = run(&mut Command::new(exe));
    // invalid UTF-8 became U+FFFD (3 bytes each); NULL is a run-time error, not a crash
    assert_eq!(r.out.lines().next(), Some("ok \u{fffd}\u{fffd} bad \u{fffd} end"), "{}", r.out);
    assert_eq!(r.code, 2, "{}", r.err);
    assert!(r.err.contains("`nothing` returned NULL"), "{}", r.err);
}

#[test]
fn the_compiler_rejects_what_cannot_cross_to_c() {
    let dir = scratch("errors");
    let prog = "import \"nothere.c\";\nimport \"x.txt\";\nextern fn f(a: [i64]) -> i64;\nextern fn g<T>(a: i64) -> i64;\n\
                extern fn h() -> [i64];\nfn nobody(a: i64) -> i64;\nextern fn withbody() -> i64 { 1 }\nfn main() { println(1); }\n";
    std::fs::write(dir.join("prog.neon"), prog).unwrap();
    let o = neon().arg("check").arg(dir.join("prog.neon")).output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    let err = String::from_utf8_lossy(&o.stderr);
    for code in ["E0306", "E0238", "E0239", "E0112"] {
        assert!(err.contains(code), "{code} missing:\n{err}");
    }
    assert!(err.contains("cannot find the C file `nothere.c`") && err.contains("is not a C source file"), "{err}");
    assert!(err.contains("takes numbers") && err.contains("cannot be generic") && err.contains("has no body"), "{err}");
}

#[test]
fn names_owned_by_the_runtime_cannot_be_declared_extern() {
    let dir = scratch("reserved");
    std::fs::write(dir.join("m.c"), "#include \"neon.h\"\n").unwrap();
    std::fs::write(
        dir.join("prog.neon"),
        "import \"m.c\";\nextern fn str_new(a: i64) -> i64;\nfn main() { println(str_new(1)); }\n",
    )
    .unwrap();
    let o = neon().arg("c").arg(dir.join("prog.neon")).output().unwrap();
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("`str_new`"), "{}", String::from_utf8_lossy(&o.stderr));
}

#[test]
fn the_formatter_keeps_extern_fn_and_c_imports() {
    let dir = scratch("fmt");
    let p = dir.join("prog.neon");
    std::fs::write(&p, "import   \"m.c\" ;\nextern   fn  f( a:i64 )->i64 ;\nexport extern fn g();\n").unwrap();
    let o = neon().arg("fmt").arg(&p).output().unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.contains("import \"m.c\";") && out.contains("extern fn f(a: i64) -> i64;"), "{out}");
    assert!(out.contains("export extern fn g();"), "{out}");
}
