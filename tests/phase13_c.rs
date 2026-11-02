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

/// The runtime (`runtime/neon.c`) compiled once per set of flags and shared by the tests of this file.
fn runtime_object(cc: &str, extra: &[&str]) -> PathBuf {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime");
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    let key = extra.join(" ");
    if let Some(p) = cache.get(&key) {
        return p.clone();
    }
    let dir = std::env::temp_dir().join(format!("neon-rt-{}-{}", std::process::id(), cache.len()));
    std::fs::create_dir_all(&dir).unwrap();
    let obj = dir.join("neon.o");
    let o = Command::new(cc)
        .args(["-std=c99", "-pedantic", "-Wall", "-Wextra", "-Werror", "-O1"])
        .args(extra)
        .arg("-I")
        .arg(&root)
        .arg("-c")
        .arg(root.join("neon.c"))
        .arg("-o")
        .arg(&obj)
        .output()
        .unwrap();
    assert!(o.status.success(), "neon.c: {}", String::from_utf8_lossy(&o.stderr));
    cache.insert(key, obj.clone());
    obj
}

/// Builds `c_source` (warnings are errors) and returns the executable.
fn build(cc: &str, dir: &Path, name: &str, c_source: &str, extra: &[&str]) -> PathBuf {
    let c = dir.join(format!("{name}.c"));
    let exe = dir.join(name);
    std::fs::write(&c, c_source).unwrap();
    let o = Command::new(cc)
        .args(["-std=c99", "-pedantic", "-Wall", "-Wextra", "-Werror", "-O1"])
        .args(extra)
        .arg("-I")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime"))
        .arg(&c)
        .arg(runtime_object(cc, extra))
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
fn names_from_the_source_never_clash_with_c() {
    let Some(cc) = c_compiler() else { return };
    // locals, parameters, fields and variants named like C keywords, runtime types, macros, library
    // functions and the generator's own variables; shadowing; a name that is not a C identifier
    let src = r#"
struct Obj { len: i64, int: i64, next: i64 }
enum Kind { FRAME(i64), AGG, Point_x }
struct Point { x: i64, Point_x: i64 }
fn work(int: i64, sizeof: i64, printf: i64, fr: i64, up: i64, o: i64, t: i64) -> i64 {
    let main = int + sizeof;
    let x = printf + fr;
    let x = x * up;
    let sin = o - t;
    main + x + sin
}
fn shape(k: Kind) -> i64 { match k { Kind::FRAME(n) => n, Kind::AGG => 7, Kind::Point_x => 9 } }
fn main() {
    let объект = Obj { len: 3, int: 4, next: 5 };
    var NULL = 0;
    for stdout in 0..3 { NULL += work(stdout, 2, 3, 4, 5, 6, 7) + объект.len; }
    let p = Point { x: 1, Point_x: 2 };
    var double = p.x + p.Point_x;
    double += shape(Kind::FRAME(10)) + shape(Kind::AGG) + shape(Kind::Point_x);
    println(NULL + double + объект.next + объект.int);
}
"#;
    for flags in [&[][..], &["--no-opt"]] {
        let dir = scratch("names");
        let p = dir.join("p.neon");
        std::fs::write(&p, src).unwrap();
        let c = translate(&p, flags);
        let exe = build(&cc, &dir, "p", &c, &[]);
        let native = run(&mut Command::new(exe), "");
        let vm = run(neon().arg("run").arg(&p), "");
        assert_eq!(vm.code, 0, "{}", vm.err);
        assert_eq!((native.code, &native.out), (0, &vm.out), "{flags:?}: {}", native.err);
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
}

#[test]
fn command_line_arguments_reach_the_program_like_in_the_vm() {
    let Some(cc) = c_compiler() else { return };
    let dir = scratch("args");
    let src = dir.join("p.neon");
    std::fs::write(
        &src,
        "import \"sys\" as sys;\nprintln(sys::args().len()); for a in sys::args() { println(f\"[{a}]\"); }",
    )
    .unwrap();
    let c = translate(&src, &[]);
    assert!(c.contains("int main(int argc, char **argv)"));
    let exe = build(&cc, &dir, "p", &c, &[]);
    for args in [&[][..], &["one"], &["a b", "", "ü", "--x"]] {
        let native = run(Command::new(&exe).args(args), "");
        let vm = run(neon().arg("run").arg(&src).arg("--").args(args), "");
        assert_eq!((native.code, &native.out), (0, &vm.out), "{args:?}");
    }
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
    // one include, the program's declarations, the program, `main`
    let order = [at("#include \"neon.h\""), at("/* ---- Declarations"), at("/* ---- Program ---"), at("int main(")];
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
    // the only include is neon.h, and the file holds no runtime: every function is the program's own
    assert_eq!(c.matches("#include").count(), 1);
    assert!(!c.contains("inline") && !c.contains("#if") && !c.contains("#define"));
    for line in c.lines().filter(|l| l.ends_with(") {") && !l.starts_with(' ')) {
        let ok = line.starts_with("static ") || line.starts_with("int main(") || line.starts_with("void user_roots(");
        assert!(ok, "{line}");
    }
    assert!(c.len() < 30_000, "the runtime must not be copied into every program ({} bytes)", c.len());
}

#[test]
fn the_runtime_files_come_from_the_project_and_can_be_written_out() {
    let dir = scratch("runtime");
    let o = neon().arg("runtime").arg(&dir).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime");
    for name in ["neon.h", "neon.c"] {
        assert_eq!(std::fs::read_to_string(dir.join(name)).unwrap(), std::fs::read_to_string(root.join(name)).unwrap());
    }
}

#[test]
fn the_runtime_compiles_alone_and_as_a_header_in_every_mode() {
    let Some(cc) = c_compiler() else { return };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime");
    // the header must be self-contained and warning-free under pedantic C99 and C11, with and without checks
    let dir = scratch("header");
    let user = dir.join("u.c");
    std::fs::write(&user, "#include \"neon.h\"\nint main(void) { return 0; }\n").unwrap();
    for std in ["-std=c99", "-std=c11"] {
        for mode in [&[][..], &["-DNDEBUG"], &["-DNEON_GC_STRESS"]] {
            let o = Command::new(&cc)
                .args([std, "-pedantic", "-Wall", "-Wextra", "-Werror", "-Wshadow", "-Wconversion", "-fsyntax-only"])
                .args(mode)
                .arg("-I")
                .arg(&root)
                .arg(&user)
                .output()
                .unwrap();
            let err = String::from_utf8_lossy(&o.stderr);
            // the checked arithmetic converts on purpose; only warnings that are not about conversions count
            assert!(o.status.success() || err.contains("-Wconversion"), "{std} {mode:?}: {err}");
        }
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

#[test]
fn the_interactive_paint_example_reacts_to_keys_the_same_in_c_and_the_vm() {
    let Some(cc) = c_compiler() else { return };
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/paint.neon")).unwrap();
    // every key acts at once, no Enter: move, change tool and size, stamp, change color, stamp, undo, an arrow
    // (an escape sequence), Ctrl+Down, quit. Not a terminal, so every frame is printed in full.
    let keys = "ddt++ ssssc www a  tu\x1b[C\x1b[1;5Bq";
    let dir = scratch("paint");
    let src = dir.join("p.neon");
    std::fs::write(&src, source).unwrap();
    let c = translate(&src, &[]);
    let exe = build(&cc, &dir, "p", &c, &[]);
    let native = run(Command::new(exe).arg("--plain").current_dir(&dir), keys);
    let vm = run(neon().arg("run").arg(&src).arg("--").arg("--plain").current_dir(&dir), keys);
    assert_eq!((native.code, vm.code), (0, 0), "{} {}", native.err, vm.err);
    assert_eq!(native.out, vm.out);
    // a frame at the start and after every key but the last
    assert_eq!(vm.out.matches("arrows move").count(), 24, "{}", vm.out);
    assert!(
        vm.out.contains("pos (26, 9)  tool dot  size 4  color rgb 250,240,200  brush covers 1  stamps 0"),
        "{}",
        vm.out
    );
    assert!(vm.out.contains("pos (25, 10)  tool dot  size 6  color eraser  brush covers 1  stamps 5"), "{}", vm.out);
    // the undo, then the arrow and Ctrl+Down (four rows)
    assert!(vm.out.contains("pos (25, 10)  tool line  size 6  color eraser  brush covers 12  stamps 4"), "{}", vm.out);
    assert!(vm.out.contains("pos (26, 14)  tool line  size 6  color eraser  brush covers 12  stamps 4"), "{}", vm.out);
    assert!(vm.out.ends_with("bye\n"), "{}", vm.out);
    // saving writes the picture next to the program
    let saved = run(neon().arg("run").arg(&src).arg("--").arg("--plain").current_dir(&dir), " o q");
    assert_eq!(saved.code, 0, "{}", saved.err);
    assert!(saved.out.contains("saved to paint.txt"), "{}", saved.out);
    let picture = std::fs::read_to_string(dir.join("paint.txt")).unwrap();
    assert_eq!(picture.lines().count(), 18);
    assert!(picture.lines().any(|l| l.contains('#')), "{picture}");
}
