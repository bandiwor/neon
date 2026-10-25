//! Phase 10: the `neon` binary end to end — every corpus program, the REPL,
//! error recovery, exit statuses and the collector limits.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn neon() -> Command {
    Command::new(env!("CARGO_BIN_EXE_neon"))
}

struct Run {
    code: i32,
    out: String,
    err: String,
}

fn run_with(args: &[&str], stdin: &str) -> Run {
    let mut child =
        neon().args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    let o = child.wait_with_output().unwrap();
    Run {
        code: o.status.code().unwrap_or(-1),
        out: String::from_utf8(o.stdout).unwrap(),
        err: String::from_utf8(o.stderr).unwrap(),
    }
}

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus")
}

fn corpus_files() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(corpus_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "neon"))
        .collect();
    v.sort();
    v
}

#[test]
fn every_corpus_program_prints_its_expected_output() {
    for path in corpus_files() {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let expected = std::fs::read_to_string(path.with_extension("out")).unwrap();
        for flags in [&[][..], &["--no-opt", "--no-peephole"], &["--gc-stress"]] {
            let mut args = vec!["run"];
            args.extend_from_slice(flags);
            args.push(path.to_str().unwrap());
            let r = run_with(&args, "");
            assert_eq!(r.code, 0, "{name} {flags:?}: {}", r.err);
            assert_eq!(r.out, expected, "{name} {flags:?}");
        }
    }
}

#[test]
fn the_bare_file_form_runs_a_program() {
    let r = run_with(&[corpus_dir().join("sample_fizzbuzz.neon").to_str().unwrap()], "");
    assert_eq!(r.code, 0);
    assert!(r.out.starts_with("1\n2\nFizz\n4\nBuzz\n"));
}

#[test]
fn trapping_programs_exit_2_and_report_to_stderr() {
    let dir = scratch("trap");
    let f = dir.join("trap.neon");
    std::fs::write(&f, "println(\"before\"); let x: i8 = 127; let y = x + 1; println(y);").unwrap();
    let r = run_with(&["run", f.to_str().unwrap()], "");
    assert_eq!(r.code, 2, "{}", r.err);
    assert_eq!(r.out, "before\n");
    assert!(r.err.starts_with("runtime error: integer overflow"), "{}", r.err);
}

#[test]
fn input_flows_from_stdin() {
    let dir = scratch("input");
    let f = dir.join("greet.neon");
    std::fs::write(
        &f,
        "print(\"name: \"); let n = input(); println(\"hello, \" + n);\nlet k = input().parse_i64(); println(k + 1);",
    )
    .unwrap();
    let r = run_with(&["run", f.to_str().unwrap()], "world\n41\n");
    assert_eq!((r.code, r.out.as_str()), (0, "name: hello, world\n42\n"), "{}", r.err);
    // end of input reads as an empty string
    let r = run_with(&["run", f.to_str().unwrap()], "");
    assert_eq!(r.code, 2, "parsing the empty string traps");
    assert!(r.out.starts_with("name: hello, \n"), "{}", r.out);
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("neon-cli-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn multiple_modules_resolve_relative_to_the_importing_file() {
    let dir = scratch("modules");
    std::fs::create_dir_all(dir.join("lib")).unwrap();
    std::fs::write(
        dir.join("main.neon"),
        "import { area } from \"lib/shapes\";\nimport \"lib/shapes\" as s;\nprintln(area(s::unit()));",
    )
    .unwrap();
    std::fs::write(dir.join("lib/shapes.neon"), "export struct Sq { side: f64 }\nexport fn unit() -> Sq { Sq { side: 1.5 } }\nexport fn area(q: Sq) -> f64 { q.side * q.side }").unwrap();
    let r = run_with(&["run", dir.join("main.neon").to_str().unwrap()], "");
    assert_eq!((r.code, r.out.as_str()), (0, "2.25\n"), "{}", r.err);
    // a missing import is a compile error that names the file
    std::fs::write(dir.join("broken.neon"), "import { x } from \"nothing\";").unwrap();
    let r = run_with(&["check", dir.join("broken.neon").to_str().unwrap()], "");
    assert_eq!(r.code, 1);
    assert!(r.err.contains("nothing"), "{}", r.err);
}

#[test]
fn the_parser_recovers_and_every_error_is_reported_in_one_run() {
    let dir = scratch("recovery");
    let f = dir.join("broken.neon");
    std::fs::write(
        &f,
        "fn ok1() -> i64 { 1 }\n\
         fn bad1( { 2 }\n\
         let x = ;\n\
         fn ok2() -> i64 { ok1() + 1 }\n\
         struct S { a: i64, , b: i64 }\n\
         fn wrong() -> i64 { \"not a number\" }\n\
         println(ok2());\n",
    )
    .unwrap();
    let r = run_with(&["check", f.to_str().unwrap()], "");
    assert_eq!(r.code, 1);
    assert!(r.out.is_empty());
    let errors = r.err.matches("error[").count();
    assert!(errors >= 4, "expected several independent errors, got {errors}:\n{}", r.err);
    // the type error inside the *valid* function after the syntax errors is still found
    assert!(r.err.contains("mismatched types"), "{}", r.err);
    assert!(
        r.err.contains("broken.neon:2") && r.err.contains("broken.neon:3") && r.err.contains("broken.neon:5"),
        "{}",
        r.err
    );
    assert_eq!(r.err.matches("aborting due to").count(), 1);
}

#[test]
fn collector_limits_apply_from_the_command_line() {
    let dir = scratch("gc");
    let churn = dir.join("churn.neon");
    std::fs::write(
        &churn,
        "var n = 0; for i in 0..20000 { let s = \"garbage \" + i.to_string(); n += s.len(); } println(n);",
    )
    .unwrap();
    let r = run_with(&["run", "--gc-limit", "24k", "--stats", churn.to_str().unwrap()], "");
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.out, "248890\n");
    let collections: u64 = r.err.split_whitespace().nth(1).unwrap().parse().unwrap();
    assert!(collections > 50, "{}", r.err);
    let hog = dir.join("hog.neon");
    std::fs::write(&hog, "var a: [[i64]] = []; while true { a.push([1, 2, 3, 4, 5, 6, 7, 8]); }").unwrap();
    let r = run_with(&["run", "--gc-limit", "64k", hog.to_str().unwrap()], "");
    assert_eq!(r.code, 2);
    assert!(r.err.contains("out of memory"), "{}", r.err);
    // restricted limits and a collection at every safe-point together
    let r = run_with(
        &["run", "--gc-limit", "64k", "--gc-stress", corpus_dir().join("sample_words.neon").to_str().unwrap()],
        "",
    );
    assert_eq!(r.code, 0, "{}", r.err);
}

#[test]
fn the_repl_reads_from_a_pipe() {
    let r =
        run_with(&["repl"], "let a = 6;\nlet b = 7;\na * b\nfn sq(n: i64) -> i64 {\n  n * n\n}\nsq(a + b)\n:quit\n");
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.out, "42\n169\n");
    // with no arguments at all, a non-terminal stdin also starts the REPL
    let r = run_with(&[], "1 + 2\n");
    assert_eq!((r.code, r.out.as_str()), (0, "3\n"));
}

#[test]
fn usage_errors_and_help() {
    let r = run_with(&["--nonsense"], "");
    assert_eq!(r.code, 64);
    assert!(r.err.contains("unknown option"));
    let r = run_with(&["--help"], "");
    assert_eq!(r.code, 0);
    assert!(r.out.contains("Usage: neon"));
    let r = run_with(&["run", "/definitely/not/here.neon"], "");
    assert_eq!(r.code, 1);
    assert!(r.err.contains("here.neon"), "{}", r.err);
}

#[test]
fn disassembly_and_ir_dumps_are_available() {
    let f = corpus_dir().join("functions_recursion_and_fib.neon");
    let r = run_with(&["disasm", f.to_str().unwrap()], "");
    assert_eq!(r.code, 0);
    assert!(r.out.contains("== fib") && r.out.contains("call"), "{}", r.out);
    let r = run_with(&["ir", f.to_str().unwrap()], "");
    assert_eq!(r.code, 0);
    assert!(r.out.contains("fib"), "{}", r.out);
}

#[test]
fn fmt_rewrites_files_in_place_and_every_corpus_program_is_already_formatted() {
    let dir = std::env::temp_dir().join(format!("neon-fmt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("m.neon");
    let path = file.to_str().unwrap();
    std::fs::write(&file, "fn main(){\nprintln( 1+2 );\n}").unwrap();
    assert_eq!(run_with(&["fmt", "--check", path], "").code, 1);
    assert_eq!(run_with(&["fmt", "--write", path], "").code, 0);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "fn main() {\n    println(1 + 2);\n}\n");
    assert_eq!(run_with(&["fmt", "--check", path], "").code, 0);
    // formatting does not change what the program does
    assert_eq!(run_with(&["run", path], "").out, "3\n");

    // the checked-in programs are in the formatter's style
    for entry in std::fs::read_dir(corpus_dir()).unwrap() {
        let p = entry.unwrap().path();
        if p.extension().is_some_and(|e| e == "neon") {
            let r = run_with(&["fmt", "--check", p.to_str().unwrap()], "");
            assert!(r.code == 0 || r.err.contains("cannot format"), "{}: {}", p.display(), r.err);
        }
    }
}
