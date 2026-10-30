//! Phase 13: the C backend end to end. Every corpus program is translated with `neon c`, built with
//! the system C compiler (skipped when there is none) and must print what the VM prints; runtime
//! errors must end the same way; the layout of the generated file is checked as well.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn neon() -> Command {
    Command::new(env!("CARGO_BIN_EXE_neon"))
}

fn corpus_files() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "neon"))
        .collect();
    v.sort();
    v
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("neon-c-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The first C compiler that answers `--version`.
fn c_compiler() -> Option<String> {
    let candidates = std::env::var("CC").into_iter().chain(["cc", "gcc", "clang"].map(String::from));
    candidates.into_iter().find(|c| {
        Command::new(c).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
    })
}

/// `neon c` for a source file; panics with the compiler's message when it fails.
fn translate(path: &Path, flags: &[&str]) -> String {
    let o = neon().arg("c").args(flags).arg(path).output().unwrap();
    assert!(o.status.success(), "{}: {}", path.display(), String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout).unwrap()
}

/// Builds `c_source` (warnings are errors) and returns the executable.
fn build(cc: &str, dir: &Path, name: &str, c_source: &str, extra: &[&str]) -> PathBuf {
    let c = dir.join(format!("{name}.c"));
    let exe = dir.join(name);
    std::fs::write(&c, c_source).unwrap();
    let o = Command::new(cc)
        .args(["-std=c99", "-pedantic", "-Wall", "-Wextra", "-Werror", "-O1"])
        .args(extra)
        .arg(&c)
        .arg("-o")
        .arg(&exe)
        .arg("-lm")
        .output()
        .unwrap();
    assert!(o.status.success(), "{name}: {}", String::from_utf8_lossy(&o.stderr));
    exe
}

struct Ran {
    code: i32,
    out: String,
    err: String,
}

fn run(program: &mut Command, stdin: &str) -> Ran {
    let mut child = program.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    let o = child.wait_with_output().unwrap();
    Ran {
        code: o.status.code().unwrap_or(-1),
        out: String::from_utf8_lossy(&o.stdout).into_owned(),
        err: String::from_utf8_lossy(&o.stderr).into_owned(),
    }
}

/// Compiles a snippet to C, builds it and runs it next to the VM.
fn both(cc: &str, tag: &str, source: &str, stdin: &str) -> (Ran, Ran) {
    let dir = scratch(tag);
    let src = dir.join("p.neon");
    std::fs::write(&src, source).unwrap();
    let c = translate(&src, &[]);
    let exe = build(cc, &dir, "p", &c, &[]);
    let native = run(&mut Command::new(exe), stdin);
    let vm = run(neon().arg("run").arg(&src), stdin);
    (native, vm)
}

#[test]
fn every_corpus_program_prints_the_same_as_the_vm() {
    let Some(cc) = c_compiler() else {
        eprintln!("no C compiler: skipping");
        return;
    };
    let dir = scratch("corpus");
    for path in corpus_files() {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let expected = std::fs::read_to_string(path.with_extension("out")).unwrap();
        // optimized and unoptimized IR, with the collector running at every safe-point
        for (i, (flags, extra)) in [(&[][..], &[][..]), (&["--no-opt"], &["-DNEON_GC_STRESS"])].into_iter().enumerate()
        {
            let c = translate(&path, flags);
            let exe = build(&cc, &dir, &format!("{name}{i}"), &c, extra);
            let r = run(&mut Command::new(exe), "");
            assert_eq!(r.code, 0, "{name} {flags:?}: {}", r.err);
            assert_eq!(r.out, expected, "{name} {flags:?}");
        }
    }
}

#[test]
fn runtime_errors_end_like_the_vms() {
    let Some(cc) = c_compiler() else { return };
    let cases = [
        ("index", "let a = [1, 2, 3]; let i = 5; println(a[i]);"),
        ("parse", "println(\"12x\".parse_i64());"),
        ("key", "var m = { \"a\": 1 }; println(m[\"b\"]);"),
        ("overflow", "let x: i32 = 2147483647; let y = 1; println(x + y);"),
        ("assert", "assert(1 + 1 == 3, \"math\");"),
        ("substring", "println(\"a\".substring(0, 3));"),
        ("pop", "var a: [i64] = []; println(a.pop());"),
        ("panic", "println(1); panic(\"boom\");"),
        ("div", "let z = 0; println(10 / z);"),
        ("shift", "let n = 70; println(1i64 << n);"),
        ("neg", "let m: i8 = -128; println(-m);"),
        ("deep", "fn d(n: i64) -> i64 { if n == 0 { 0 } else { 1 + d(n - 1) } } println(d(1000000));"),
    ];
    for (tag, src) in cases {
        let (native, vm) = both(&cc, tag, src, "");
        assert_eq!(native.code, 2, "{tag}: {}", native.err);
        assert_eq!(native.code, vm.code, "{tag}");
        assert_eq!(native.out, vm.out, "{tag}");
        // the VM adds a backtrace after the first line
        assert_eq!(native.err.lines().next(), vm.err.lines().next(), "{tag}");
    }
}

#[test]
fn release_builds_drop_the_arithmetic_checks_and_debug_builds_keep_them() {
    let Some(cc) = c_compiler() else { return };
    let dir = scratch("release");
    let src = dir.join("p.neon");
    std::fs::write(&src, "let x: i32 = 2147483647; let y = 1; println(x + y); println(7 / (y - 1 + 1));").unwrap();
    let c = translate(&src, &[]);
    let debug = run(&mut Command::new(build(&cc, &dir, "dbg", &c, &[])), "");
    assert_eq!(debug.code, 2);
    assert_eq!(debug.err.lines().next(), Some("runtime error: integer overflow"));
    let release = run(&mut Command::new(build(&cc, &dir, "rel", &c, &["-DNDEBUG"])), "");
    assert_eq!((release.code, release.out.as_str()), (0, "-2147483648\n7\n"), "{}", release.err);
    // -DNEON_CHECKS=1 wins over -DNDEBUG
    let forced = run(&mut Command::new(build(&cc, &dir, "forced", &c, &["-DNDEBUG", "-DNEON_CHECKS=1"])), "");
    assert_eq!(forced.code, 2);
    // after the preprocessor a release build contains no trap for arithmetic
    let pre = Command::new(&cc).args(["-E", "-P", "-DNDEBUG"]).arg(dir.join("rel.c")).output();
    if let Ok(pre) = pre {
        let text = String::from_utf8_lossy(&pre.stdout);
        assert!(!text.contains("trap_overflow") && !text.contains("OVF_"), "{text}");
    }
}

#[test]
fn command_line_arguments_reach_the_program_like_in_the_vm() {
    let Some(cc) = c_compiler() else { return };
    let dir = scratch("args");
    let src = dir.join("p.neon");
    std::fs::write(&src, "println(sys.args.len()); for a in sys.args { println(f\"[{a}]\"); }").unwrap();
    let c = translate(&src, &[]);
    assert!(c.contains("int main(int argc, char **argv)"));
    let exe = build(&cc, &dir, "p", &c, &[]);
    for args in [&[][..], &["one"], &["a b", "", "ü", "--x"]] {
        let native = run(Command::new(&exe).args(args), "");
        let vm = run(neon().arg("run").arg(&src).arg("--").args(args), "");
        assert_eq!((native.code, &native.out), (0, &vm.out), "{args:?}");
    }
    // programs that do not look at the arguments keep a plain `main(void)`
    let plain = dir.join("q.neon");
    std::fs::write(&plain, "println(1);").unwrap();
    assert!(translate(&plain, &[]).contains("int main(void)"));
}

#[test]
fn input_and_end_of_input_behave_like_the_vm() {
    let Some(cc) = c_compiler() else { return };
    let src = "let name = input(); println(\"hi \" + name);\n\
               let n = input().parse_i64(); var t = 0; for _ in 0..n { t += input().parse_i64(); } println(t);\n\
               while true { match read_line() { Option::Some(s) => println(f\"[{s}]\"), Option::None => break } }\n\
               println(input() == \"\");";
    let (native, vm) = both(&cc, "input", src, "Ann\n3\n10\r\n20\n30\nrest\n\nlast without newline");
    assert_eq!(native.code, 0, "{}", native.err);
    assert_eq!(native.out, vm.out);
    assert!(native.out.contains("[last without newline]") && native.out.contains("[]"));
}

#[test]
fn the_file_has_the_requested_layout() {
    let src = corpus_files().into_iter().find(|p| p.ends_with("collections.neon")).unwrap();
    let c = translate(&src, &[]);
    let at = |needle: &str| c.find(needle).unwrap_or_else(|| panic!("`{needle}` is missing"));
    // includes, runtime declarations, program declarations, program, main, runtime implementation
    let order = [
        at("#include <stdint.h>"),
        at("/* ---- Runtime: declarations"),
        at("/* ---- Program: declarations"),
        at("/* ---- Program ---"),
        at("int main(void)"),
        at("/* ---- Runtime: implementation"),
    ];
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
    // every function is static, none is inline
    assert!(!c.contains("inline"));
    let main_at = at("int main(void)");
    for (i, line) in c.lines().enumerate() {
        let defines_function =
            line.ends_with(") {") && !line.starts_with(' ') && !line.starts_with("if") && !line.starts_with("for");
        if defines_function && !line.starts_with("int main(void)") {
            assert!(line.starts_with("static "), "line {}: {line}", i + 1);
        }
    }
    assert!(main_at > 0);
}

#[test]
fn unused_runtime_functions_are_left_out() {
    let dir = scratch("unused");
    let src = dir.join("hello.neon");
    std::fs::write(&src, "println(1 + 2);").unwrap();
    let c = translate(&src, &[]);
    // no collector, no maps, no strings, no format machinery for a program that prints a number
    for name in ["gc_collect", "map_put", "str_cat", "fmt_value", "arr_push", "parse_i64", "cmp_f64"] {
        assert!(!c.contains(name), "{name} should not be in the output:\n{c}");
    }
    // ... while the functions it does use are there, with a prototype and a body
    assert!(c.matches("static void out_sb(int newline);").count() == 1);
    assert!(c.matches("static void out_sb(int newline) {").count() == 1);
    // and every static function is referenced beyond its prototype and definition
    for line in c.lines().filter(|l| l.starts_with("static ") && l.ends_with(") {")) {
        let name =
            line.split('(').next().unwrap().rsplit(|ch: char| !(ch.is_alphanumeric() || ch == '_')).next().unwrap();
        assert!(c.matches(name).count() >= 3, "{name} is never called");
    }
}

#[test]
fn output_is_deterministic_and_can_go_to_a_file() {
    let src = corpus_files().into_iter().find(|p| p.ends_with("closures_counters_and_composition.neon")).unwrap();
    let a = translate(&src, &[]);
    assert_eq!(a, translate(&src, &["-j", "1"]));
    let dir = scratch("out");
    let out = dir.join("x.c");
    let st = neon().arg("c").arg(&src).arg("-o").arg(&out).status().unwrap();
    assert!(st.success());
    assert_eq!(std::fs::read_to_string(out).unwrap(), a);
}
