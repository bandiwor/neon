//! The standard modules `sys`, `fs` and `term`, end to end: the same program in the VM and as a native
//! executable (when there is a C compiler) must behave the same, against the real disk and with keys
//! arriving from a pipe.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn neon() -> Command {
    Command::new(env!("CARGO_BIN_EXE_neon"))
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("neon-std-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
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

fn c_compiler() -> Option<String> {
    let candidates = std::env::var("CC").into_iter().chain(["cc", "gcc", "clang"].map(String::from));
    candidates.into_iter().find(|c| {
        Command::new(c).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
    })
}

#[derive(Debug, PartialEq, Eq)]
struct Ran {
    code: i32,
    out: String,
}

fn run(program: &mut Command, stdin: &[u8], cwd: &Path) -> Ran {
    let mut child =
        program.current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    let o = child.wait_with_output().unwrap();
    Ran { code: o.status.code().unwrap_or(-1), out: String::from_utf8_lossy(&o.stdout).into_owned() }
}

/// Runs `source` in the VM and (if possible) natively, each in a fresh directory, and returns the VM's
/// result after checking that the native one is the same.
fn same_everywhere(tag: &str, source: &str, stdin: &[u8], args: &[&str]) -> Ran {
    let dir = scratch(tag);
    let src = dir.join("p.neon");
    std::fs::write(&src, source).unwrap();
    let work = |name: &str| {
        let d = dir.join(name);
        std::fs::create_dir_all(&d).unwrap();
        d
    };
    let vm = run(neon().arg("run").arg(&src).arg("--").args(args), stdin, &work("vm"));
    if let Some(cc) = c_compiler() {
        let c = neon().arg("c").arg(&src).output().unwrap();
        assert!(c.status.success(), "{}", String::from_utf8_lossy(&c.stderr));
        let cfile = dir.join("p.c");
        std::fs::write(&cfile, &c.stdout).unwrap();
        let exe = dir.join("p");
        let o = Command::new(&cc)
            .args(["-std=c99", "-pedantic", "-Wall", "-Wextra", "-Werror", "-O1"])
            .arg("-I")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime"))
            .arg(&cfile)
            .arg(runtime_object(&cc, &[]))
            .arg("-o")
            .arg(&exe)
            .arg("-lm")
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let native = run(Command::new(exe).args(args), stdin, &work("c"));
        assert_eq!(native, vm, "the native program differs from the VM");
    }
    vm
}

#[test]
fn sys_functions() {
    let r = same_everywhere(
        "sys",
        r#"import "sys" as sys;
import { env, platform, now_ms, ticks_ms, sleep } from "sys";

fn main() {
    println(sys::args());
    println(env("NEON_SURELY_NOT_SET_9").is_none());
    println(platform().len() > 2);
    println(now_ms() > 1600000000000);
    let t = ticks_ms();
    sleep(30);
    println(ticks_ms() - t >= 25);
    println("before exit");
    sys::exit(5);
    println("never");
}
"#,
        b"",
        &["one", "two words"],
    );
    assert_eq!(r.out, "[\"one\", \"two words\"]\ntrue\ntrue\ntrue\ntrue\nbefore exit\n");
    assert_eq!(r.code, 5);
}

#[test]
fn fs_functions() {
    let r = same_everywhere(
        "fs",
        r#"import "fs" as fs;

fn show(r: Result<(), string>) {
    match r {
        Result::Ok(_) => println("ok"),
        Result::Err(e) => println(f"err {e}"),
    }
}

fn main() {
    show(fs::create_dir("data"));
    show(fs::create_dir("data"));
    show(fs::create_dir_all("data/x/y"));
    show(fs::write_file("data/a.txt", "one\ntwo\r\nthree\n"));
    show(fs::append_file("data/a.txt", "four"));
    println(fs::read_lines("data/a.txt"));
    println(fs::read_file("data/a.txt").unwrap().len());
    println(fs::list_dir("data"));
    println(fs::list_dir("nowhere"));
    println(fs::list_dir("data/a.txt"));
    println(fs::read_file("data"));
    println(fs::read_file("nowhere/f"));
    println(f"{fs::exists("data")} {fs::is_dir("data")} {fs::is_file("data")} {fs::exists("data/a.txt")} {fs::is_file("data/a.txt")} {fs::exists("zzz")}");
    show(fs::copy("data/a.txt", "data/b.txt"));
    show(fs::rename("data/b.txt", "data/x/c.txt"));
    println(fs::list_dir("data/x"));
    show(fs::remove("data"));
    show(fs::write_lines("data/lines.txt", ["a", "b"]));
    println(fs::read_file("data/lines.txt"));
    show(fs::write_file("nowhere/f", "x"));
    show(fs::remove("data/x/c.txt"));
    show(fs::remove("data/x/y"));
    show(fs::remove("data/x"));
    show(fs::remove("data/lines.txt"));
    show(fs::remove("data/a.txt"));
    show(fs::remove("data"));
    println(fs::exists("data"));
    show(fs::remove("data"));
}
"#,
        b"",
        &[],
    );
    let want = "ok\nerr data: already exists\nok\nok\nok\n\
Result::Ok([\"one\", \"two\", \"three\", \"four\"])\n19\nResult::Ok([\"a.txt\", \"x\"])\n\
Result::Err(\"nowhere: not found\")\nResult::Err(\"data/a.txt: not a directory\")\n\
Result::Err(\"data: is a directory\")\nResult::Err(\"nowhere/f: not found\")\n\
true true false true true false\nok\nok\nResult::Ok([\"c.txt\", \"y\"])\nerr data: directory not empty\nok\n\
Result::Ok(\"a\\nb\\n\")\nerr nowhere/f: not found\nok\nok\nok\nok\nok\nok\nfalse\nerr data: not found\n";
    assert_eq!(r.out, want);
    assert_eq!(r.code, 0);
}

#[test]
fn keys_come_from_a_pipe_like_from_a_terminal() {
    let program = r#"import "term" as term;

fn main() {
    println(term::raw(true));
    println(term::is_tty());
    var seen: [string] = [];
    loop {
        match term::read_key() {
            Option::Some(k) => {
                seen.push(k);
                if k == "q" {
                    break;
                }
            },
            Option::None => {
                seen.push("<end>");
                break;
            },
        }
    }
    println(seen.join(" "));
    println(term::poll_key(5));
    println(term::raw(false));
    println(term::width() > 0 && term::height() > 0);
}
"#;
    let r = same_everywhere("keys", program, b"a Z\x1b[A\x1b[1;5C\r\t\x7f\x03\xd1\x8f\x1b[3~\x1bOP\x1b[24~xq", &[]);
    assert_eq!(
        r.out,
        "true\nfalse\na _ Z up ctrl+right enter tab backspace ctrl+c я delete f1 f12 x q\n\
         Option::Some(\"eof\")\ntrue\ntrue\n"
            .replace("a _ Z", "a   Z")
    );
    // the end of the input ends the loop; an escape alone is a key
    let r = same_everywhere("keys_end", program, b"\x1b", &[]);
    assert!(r.out.contains("esc <end>"), "{}", r.out);
}

#[test]
fn drawing_helpers_write_ansi_escapes() {
    let r = same_everywhere(
        "draw",
        r#"import "term" as term;

fn main() {
    term::clear();
    term::move_to(4, 2);
    print(term::paint("hi", 196));
    print(term::bold("!"));
    term::hide_cursor();
    term::show_cursor();
    term::enter_screen();
    term::leave_screen();
    println("");
}
"#,
        b"",
        &[],
    );
    assert_eq!(
        r.out,
        "\x1b[2J\x1b[H\x1b[3;5H\x1b[38;5;196mhi\x1b[0m\x1b[1m!\x1b[0m\x1b[?25l\x1b[?25h\x1b[?1049h\x1b[2J\x1b[H\x1b[?1049l\n"
    );
}
