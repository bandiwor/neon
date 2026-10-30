//! The command-line front end, written against traits so it can be tested
//! (and embedded) without a real terminal or file system.

use std::fmt::Write as _;
use std::io::{BufRead, Write};

use crate::compiler::CompileOptions;
use crate::driver::{self, Compiled, Options};
use crate::gc::GcConfig;
use crate::ir::OptConfig;
use crate::repl::{self, ReplConfig};
use crate::vfs::Vfs;
use crate::vm::{self, StreamIo, VmConfig};

pub const USAGE: &str = "\
Usage: neon <command> [options] [file]

Commands:
  run <file>      compile and run a program (the default for `neon <file>`)
  check <file>    report errors without running
  disasm <file>   print the compiled bytecode (--annotate adds pseudo-code to every line)
  c <file>        translate to one self-contained C file (stdout, or -o <out.c>)
  ir <file>       print the (optimized) intermediate representation (--effects: and what each
                  function can do: const / pure / impure, traps, allocates, ...)
  repl            start an interactive session
  fmt <file>...   format source files (prints; -w rewrites, --check verifies)
  lsp             run the language server on stdin/stdout

Options:
  --no-opt            disable IR optimizations (folding, DCE, inlining)
  --inline / --no-inline  force IR inlining on or off
  --no-peephole       disable the bytecode peephole optimizer
  -j, --threads <n>   compiler threads (0 = all cores, 1 = none; default: NEON_THREADS or all cores)
  --gc-limit <size>   cap the heap (e.g. 512k, 64m); exceeding it traps
  --gc-stress         collect garbage at every safe-point (for testing)
  --max-steps <n>     stop with a panic after n instructions
  --max-frames <n>    maximum call depth
  --stats             print collector statistics to stderr after the run
  -o, --output <file> (c) write the C source to a file instead of stdout
  -a, --annotate      (disasm) explain every instruction as `# r1 = r1 + 4`
  -w, --write         (fmt) rewrite the files in place
  --check             (fmt) exit with status 1 if a file is not formatted
  -h, --help          show this help
  -V, --version       show the version

Exit status: 0 success, 1 compile error, 2 runtime error, 64 usage error.
";

pub const EXIT_OK: i32 = 0;
pub const EXIT_COMPILE: i32 = 1;
pub const EXIT_RUNTIME: i32 = 2;
pub const EXIT_USAGE: i32 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Run,
    Check,
    Disasm,
    C,
    Ir,
    Repl,
    Lsp,
    Fmt,
}

#[derive(Debug)]
struct Args {
    command: Command,
    file: Option<String>,
    /// every file argument (`fmt` takes several)
    files: Vec<String>,
    write: bool,
    check: bool,
    annotate: bool,
    effects: bool,
    output: Option<String>,
    opts: Options,
    vm: VmConfig,
    stats: bool,
}

enum Parsed {
    Go(Args),
    Exit(String, i32),
}

/// `64k`, `2m`, `1g` or a plain number of bytes.
pub fn parse_size(s: &str) -> Option<usize> {
    let s = s.trim().to_ascii_lowercase();
    let (digits, mult) = match s.chars().last()? {
        'k' => (&s[..s.len() - 1], 1usize << 10),
        'm' => (&s[..s.len() - 1], 1 << 20),
        'g' => (&s[..s.len() - 1], 1 << 30),
        _ => (s.as_str(), 1),
    };
    digits.parse::<usize>().ok()?.checked_mul(mult)
}

fn parse_args(args: &[String]) -> Parsed {
    let usage = |msg: String| Parsed::Exit(format!("error: {msg}\n\n{USAGE}"), EXIT_USAGE);
    let mut command = None;
    let mut file = None;
    let mut files: Vec<String> = Vec::new();
    let mut write = false;
    let mut check = false;
    let mut annotate = false;
    let mut effects = false;
    let mut output = None;
    let mut inline = None;
    let mut opts = Options { opt: OptConfig::default(), compile: CompileOptions { peephole: true } };
    let mut vmcfg = VmConfig::default();
    let mut gc = GcConfig::default();
    let mut stats = false;
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        let mut value = |name: &str| -> Result<String, String> {
            it.next().cloned().ok_or_else(|| format!("`{name}` needs a value"))
        };
        match a.as_str() {
            "-h" | "--help" => return Parsed::Exit(USAGE.to_string(), EXIT_OK),
            "-V" | "--version" => return Parsed::Exit(format!("neon {}\n", env!("CARGO_PKG_VERSION")), EXIT_OK),
            "--no-opt" => opts.opt = OptConfig::none(),
            "--inline" => inline = Some(true),
            "--no-inline" => inline = Some(false),
            "--no-peephole" => opts.compile.peephole = false,
            "-j" | "--threads" => match value("--threads")
                .and_then(|v| v.parse::<usize>().map_err(|_| format!("bad thread count `{v}`")))
            {
                Ok(n) => crate::par::set_threads(n),
                Err(e) => return usage(e),
            },
            "--gc-stress" => {
                gc.stress = true;
                gc.reuse_slots = false;
            }
            "--stats" => stats = true,
            "-w" | "--write" => write = true,
            "--check" => check = true,
            "-a" | "--annotate" => annotate = true,
            "--effects" => effects = true,
            "-o" | "--output" => match value("--output") {
                Ok(v) => output = Some(v),
                Err(e) => return usage(e),
            },
            // editors launch `neon lsp --stdio`; stdio is the only transport
            "--stdio" => {}
            "--gc-limit" => match value("--gc-limit").and_then(|v| parse_size(&v).ok_or(format!("bad size `{v}`"))) {
                Ok(n) => gc.limit = Some(n),
                Err(e) => return usage(e),
            },
            "--max-steps" => {
                match value("--max-steps").and_then(|v| v.parse::<u64>().map_err(|_| format!("bad count `{v}`"))) {
                    Ok(n) => vmcfg.max_steps = Some(n),
                    Err(e) => return usage(e),
                }
            }
            "--max-frames" => {
                match value("--max-frames").and_then(|v| v.parse::<usize>().map_err(|_| format!("bad count `{v}`"))) {
                    Ok(n) => vmcfg.max_frames = n,
                    Err(e) => return usage(e),
                }
            }
            s if s.starts_with("--") || (s.starts_with('-') && s.len() > 1) => {
                return usage(format!("unknown option `{s}`"))
            }
            "run" if command.is_none() && file.is_none() => command = Some(Command::Run),
            "check" if command.is_none() && file.is_none() => command = Some(Command::Check),
            "disasm" if command.is_none() && file.is_none() => command = Some(Command::Disasm),
            "c" if command.is_none() && file.is_none() => command = Some(Command::C),
            "ir" if command.is_none() && file.is_none() => command = Some(Command::Ir),
            "repl" if command.is_none() && file.is_none() => command = Some(Command::Repl),
            "lsp" if command.is_none() && file.is_none() => command = Some(Command::Lsp),
            "fmt" if command.is_none() && file.is_none() => command = Some(Command::Fmt),

            path if command == Some(Command::Fmt) => files.push(path.to_string()),
            path => {
                if file.is_some() {
                    return usage(format!("unexpected argument `{path}`"));
                }
                file = Some(path.to_string());
            }
        }
    }
    vmcfg.gc = gc;
    let command = command.unwrap_or(if file.is_some() { Command::Run } else { Command::Repl });
    if let Some(on) = inline {
        opts.opt.inline = on;
    }
    if !matches!(command, Command::Repl | Command::Lsp | Command::Fmt) && file.is_none() {
        return usage("this command needs a file".to_string());
    }
    Parsed::Go(Args { command, file, files, write, check, annotate, effects, output, opts, vm: vmcfg, stats })
}

/// Reads the compiler's verdict: prints diagnostics and returns `None` on errors.
fn compile_or_report(c: Compiled, err: &mut dyn Write) -> Option<Compiled> {
    // the rendered text already ends with the "aborting due to N previous errors" summary
    let text = c.render_diagnostics();
    if !text.is_empty() {
        let _ = err.write_all(text.as_bytes());
    }
    if c.diags.has_errors() {
        None
    } else {
        Some(c)
    }
}

/// `neon fmt`: formats files (or stdin when there are none, or the name is `-`).
fn run_fmt<R: BufRead, W: Write, E: Write>(
    args: &Args,
    vfs: &dyn Vfs,
    mut stdin: R,
    stdout: &mut W,
    stderr: &mut E,
) -> i32 {
    if args.files.len() > 1 && !args.write && !args.check {
        let _ = writeln!(stderr, "error: formatting several files needs `--write` or `--check`\n\n{USAGE}");
        return EXIT_USAGE;
    }
    let mut status = EXIT_OK;
    let inputs: Vec<&str> =
        if args.files.is_empty() { vec!["-"] } else { args.files.iter().map(String::as_str).collect() };
    for name in inputs {
        let source = if name == "-" {
            let mut text = String::new();
            stdin.read_to_string(&mut text).map(|_| text)
        } else {
            vfs.read_to_string(name)
        };
        let source = match source {
            Ok(s) => s,
            Err(e) => {
                let _ = writeln!(stderr, "error: cannot read `{name}`: {e}");
                status = EXIT_COMPILE;
                continue;
            }
        };
        let formatted = match crate::fmt::format_source(&source) {
            Ok(f) => f,
            Err(e) => {
                let _ = writeln!(stderr, "error: {name}: {e}");
                status = EXIT_COMPILE;
                continue;
            }
        };
        // CRLF files are rewritten with LF, which counts as a difference
        let changed = formatted != source;
        if args.check {
            if changed {
                let _ = writeln!(stderr, "{name}: not formatted");
                status = EXIT_COMPILE;
            }
        } else if args.write && name != "-" {
            if changed {
                if let Err(e) = vfs.write(name, &formatted) {
                    let _ = writeln!(stderr, "error: cannot write `{name}`: {e}");
                    status = EXIT_COMPILE;
                }
            }
        } else {
            let _ = stdout.write_all(formatted.as_bytes());
        }
    }
    status
}

/// Runs the CLI with `args` (without the program name) and returns the exit status.
pub fn main_with<R: BufRead, W: Write, E: Write>(
    args: &[String],
    vfs: &dyn Vfs,
    stdin: R,
    stdout: W,
    stderr: &mut E,
    interactive: bool,
) -> i32 {
    let args = match parse_args(args) {
        Parsed::Go(a) => a,
        Parsed::Exit(text, code) => {
            let mut stdout = stdout;
            let _ = if code == EXIT_OK { write!(stdout, "{text}") } else { write!(stderr, "{text}") };
            return code;
        }
    };
    let mut stdout = stdout;
    if args.command == Command::Lsp {
        return crate::lsp::run(stdin, stdout, vfs);
    }
    if args.command == Command::Fmt {
        return run_fmt(&args, vfs, stdin, &mut stdout, stderr);
    }
    if args.command == Command::Repl {
        let cfg = ReplConfig { options: args.opts, vm: args.vm, prompt: interactive };
        let mut stdin = stdin;
        return match repl::run(&mut stdin, &mut stdout, vfs, &cfg) {
            Ok(()) => EXIT_OK,
            Err(e) => {
                let _ = writeln!(stderr, "error: {e}");
                EXIT_RUNTIME
            }
        };
    }
    let file = args.file.as_deref().unwrap();
    let compiled = driver::compile(file, vfs, &args.opts);
    let Some(compiled) = compile_or_report(compiled, stderr) else { return EXIT_COMPILE };
    match args.command {
        Command::Check => EXIT_OK,
        Command::C => {
            let ir = compiled.ir.as_ref().expect("ir");
            let text = match crate::cgen::emit_c(ir) {
                Ok(t) => t,
                Err(e) => {
                    let _ = writeln!(stderr, "error: {e}");
                    return EXIT_COMPILE;
                }
            };
            match &args.output {
                Some(path) => {
                    if let Err(e) = vfs.write(path, &text) {
                        let _ = writeln!(stderr, "error: cannot write {path}: {e}");
                        return EXIT_RUNTIME;
                    }
                }
                None => {
                    let _ = stdout.write_all(text.as_bytes());
                }
            }
            EXIT_OK
        }
        Command::Ir => {
            let ir = compiled.ir.as_ref().expect("ir");
            if args.effects {
                let fx = crate::ir::effects::analyze(ir);
                let _ =
                    writeln!(stdout, "// effects: const (only the arguments matter), pure (also reads memory), impure");
                for line in crate::ir::effects::report(ir, &fx).lines() {
                    let _ = writeln!(stdout, "// {line}");
                }
                let _ = writeln!(stdout);
            }
            let _ = write!(stdout, "{ir}");
            EXIT_OK
        }
        Command::Disasm => {
            let mut text = String::new();
            match crate::bytecode::disassemble_with(compiled.bytecode.as_ref().expect("bytecode"), args.annotate) {
                Ok(t) => text.push_str(&t),
                Err(e) => {
                    let _ = write!(text, "<disassembly failed: {e}>");
                }
            }
            let _ = stdout.write_all(text.as_bytes());
            EXIT_OK
        }
        Command::Run => {
            let bc = compiled.bytecode.as_ref().expect("bytecode");
            let mut io = StreamIo::new(stdin, std::io::BufWriter::new(stdout));
            let report = vm::run(bc, &mut io, &args.vm);
            let (_, mut w) = io.into_parts();
            let _ = w.flush();
            if args.stats {
                let g = &report.gc;
                let _ = writeln!(
                    stderr,
                    "gc: {} collections, {} allocations, {} objects freed, peak {} bytes",
                    g.collections, g.allocations, g.freed_objects, g.peak_bytes
                );
            }
            match report.result {
                Ok(()) => EXIT_OK,
                Err(e) => {
                    let _ = writeln!(stderr, "{e}");
                    EXIT_RUNTIME
                }
            }
        }
        Command::Repl | Command::Lsp | Command::Fmt => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::MemVfs;

    fn go(args: &[&str], files: &[(&str, &str)], input: &str) -> (i32, String, String) {
        let mut vfs = MemVfs::new();
        for (p, c) in files {
            vfs.insert(p, c);
        }
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = main_with(&args, &vfs, std::io::Cursor::new(input.as_bytes().to_vec()), &mut out, &mut err, false);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    #[test]
    fn ir_effects_lists_what_each_function_can_do() {
        let files = [(
            "e.neon",
            "fn sq(x: f64) -> f64 { x * x }\nfn show(x: f64) { println(x); }\nfn main() { show(sq(2.0)); }",
        )];
        let (code, out, err) = go(&["ir", "--effects", "--no-opt", "e.neon"], &files, "");
        assert_eq!((code, err.as_str()), (EXIT_OK, ""));
        assert!(out.contains("// f") && out.contains("sq"), "{out}");
        assert!(out.lines().any(|l| l.starts_with("//") && l.contains(" sq") && l.contains("const")), "{out}");
        assert!(
            out.lines().any(|l| l.starts_with("//") && l.contains(" show") && l.contains("impure") && l.contains("io")),
            "{out}"
        );
        // without the flag the IR is printed alone
        let (_, plain, _) = go(&["ir", "--no-opt", "e.neon"], &files, "");
        assert!(!plain.contains("// effects"), "{plain}");
        assert!(out.ends_with(&plain), "the IR follows the table unchanged");
    }

    #[test]
    fn fmt_prints_to_stdout_and_reads_stdin() {
        let files = [("a.neon", "fn main(){\nlet x=1;\n}\n")];
        let (code, out, err) = go(&["fmt", "a.neon"], &files, "");
        assert_eq!((code, err.as_str()), (EXIT_OK, ""));
        assert_eq!(out, "fn main() {\n    let x = 1;\n}\n");
        let (code, out, _) = go(&["fmt"], &[], "fn f(){}");
        assert_eq!((code, out.as_str()), (EXIT_OK, "fn f() {}\n"));
        let (code, out, _) = go(&["fmt", "-"], &[], "fn f(){}");
        assert_eq!((code, out.as_str()), (EXIT_OK, "fn f() {}\n"));
    }

    #[test]
    fn fmt_check_reports_unformatted_files_and_refuses_broken_ones() {
        let files = [("ok.neon", "fn f() {}\n"), ("bad.neon", "fn f(){}\n"), ("broken.neon", "fn f( {")];
        assert_eq!(go(&["fmt", "--check", "ok.neon"], &files, "").0, EXIT_OK);
        let (code, _, err) = go(&["fmt", "--check", "ok.neon", "bad.neon"], &files, "");
        assert_eq!(code, EXIT_COMPILE);
        assert!(err.contains("bad.neon: not formatted") && !err.contains("ok.neon"), "{err}");
        let (code, _, err) = go(&["fmt", "broken.neon"], &files, "");
        assert_eq!(code, EXIT_COMPILE);
        assert!(err.contains("cannot format"), "{err}");
        // several files need an explicit mode; a read-only file system cannot be written
        assert_eq!(go(&["fmt", "ok.neon", "bad.neon"], &files, "").0, EXIT_USAGE);
        let (code, _, err) = go(&["fmt", "--write", "bad.neon"], &files, "");
        assert_eq!(code, EXIT_COMPILE);
        assert!(err.contains("cannot write"), "{err}");
    }

    #[test]
    fn sizes() {
        assert_eq!(parse_size("100"), Some(100));
        assert_eq!(parse_size("64k"), Some(64 << 10));
        assert_eq!(parse_size("2M"), Some(2 << 20));
        assert_eq!(parse_size("1g"), Some(1 << 30));
        assert_eq!(parse_size("x"), None);
        assert_eq!(parse_size(""), None);
    }

    #[test]
    fn help_and_version() {
        let (code, out, _) = go(&["--help"], &[], "");
        assert_eq!(code, 0);
        assert!(out.contains("Usage: neon"));
        let (code, out, _) = go(&["-V"], &[], "");
        assert_eq!((code, out.starts_with("neon ")), (0, true));
    }

    #[test]
    fn usage_errors() {
        for args in [
            &["--bogus"][..],
            &["run"],
            &["--gc-limit"],
            &["--gc-limit", "lots", "a.neon"],
            &["a.neon", "b.neon"],
            &["check"],
        ] {
            let (code, _, err) = go(args, &[], "");
            assert_eq!(code, EXIT_USAGE, "{args:?}: {err}");
            assert!(err.starts_with("error:"), "{err}");
        }
    }

    #[test]
    fn runs_a_program_and_forwards_input() {
        let src = "let name = input(); println(\"hi \" + name); println(6 * 7);";
        let (code, out, err) = go(&["run", "p.neon"], &[("p.neon", src)], "Ada\n");
        assert_eq!((code, out.as_str(), err.as_str()), (0, "hi Ada\n42\n", ""));
        // the `run` command is the default
        let (code, out, _) = go(&["p.neon"], &[("p.neon", src)], "Bob\n");
        assert_eq!((code, out.as_str()), (0, "hi Bob\n42\n"));
    }

    #[test]
    fn the_c_command_prints_one_translation_unit() {
        let files = [("p.neon", "println(f\"{1 + 2}\");")];
        let (code, out, err) = go(&["c", "p.neon"], &files, "");
        assert_eq!((code, err.as_str()), (EXIT_OK, ""));
        assert!(out.starts_with("/* Generated by the Neon compiler") && out.contains("int main(void)"), "{out}");
        // `--no-opt` changes the code, a compile error still exits 1, a missing file is a usage error
        let (_, plain, _) = go(&["c", "--no-opt", "p.neon"], &files, "");
        assert_ne!(plain, out);
        let (code, _, err) = go(&["c", "bad.neon"], &[("bad.neon", "println(y);")], "");
        assert_eq!(code, EXIT_COMPILE);
        assert!(err.contains("error["), "{err}");
        let (code, _, _) = go(&["c"], &[], "");
        assert_eq!(code, EXIT_USAGE);
    }

    #[test]
    fn compile_errors_exit_1_with_rendered_diagnostics() {
        let (code, out, err) = go(&["run", "bad.neon"], &[("bad.neon", "let x: i32 = \"s\";\nprintln(y);")], "");
        assert_eq!(code, EXIT_COMPILE);
        assert!(out.is_empty());
        assert!(err.contains("error[") && err.contains("bad.neon:1"), "{err}");
        assert_eq!(err.matches("aborting due to").count(), 1, "{err}");
        let (code, _, err) = go(&["check", "nope.neon"], &[], "");
        assert_eq!(code, EXIT_COMPILE);
        assert!(err.contains("nope.neon"), "{err}");
    }

    #[test]
    fn runtime_errors_exit_2_after_flushing_output() {
        let (code, out, err) = go(&["run", "p.neon"], &[("p.neon", "println(1); let z = 0; println(1 / z);")], "");
        assert_eq!(code, EXIT_RUNTIME);
        assert_eq!(out, "1\n");
        assert!(err.contains("runtime error: division by zero"), "{err}");
    }

    #[test]
    fn check_ir_and_disasm() {
        let files = [("p.neon", "fn f(x: i64) -> i64 { x + 1 }\nprintln(f(2));")];
        let (code, out, _) = go(&["check", "p.neon"], &files, "");
        assert_eq!((code, out.as_str()), (0, ""));
        let (code, out, _) = go(&["ir", "p.neon"], &files, "");
        assert_eq!(code, 0);
        assert!(out.contains("fn"), "{out}");
        let (code, out, _) = go(&["disasm", "--no-opt", "p.neon"], &files, "");
        assert_eq!(code, 0);
        assert!(out.contains("== f") && out.contains("ret"), "{out}");
    }

    #[test]
    fn gc_options_and_stats() {
        let src = "var s = 0; for i in 0..2000 { let t = \"x\" + i.to_string(); s += t.len(); } println(s);";
        let (code, out, err) = go(&["run", "--gc-limit", "32k", "--stats", "p.neon"], &[("p.neon", src)], "");
        assert_eq!(code, 0, "{err}");
        assert_eq!(out, "8890\n");
        assert!(err.starts_with("gc: ") && !err.contains(" 0 collections"), "{err}");
        let (code, out, _) = go(&["run", "--gc-stress", "p.neon"], &[("p.neon", "println(\"a\" + \"b\");")], "");
        assert_eq!((code, out.as_str()), (0, "ab\n"));
        let leak = "var a: [string] = []; while true { a.push(\"keep me around\"); }";
        let (code, _, err) = go(&["run", "--gc-limit", "16k", "p.neon"], &[("p.neon", leak)], "");
        assert_eq!(code, EXIT_RUNTIME);
        assert!(err.contains("out of memory"), "{err}");
    }

    #[test]
    fn step_and_frame_limits() {
        let (code, _, err) = go(&["run", "--max-steps", "500", "p.neon"], &[("p.neon", "while true { }")], "");
        assert_eq!(code, EXIT_RUNTIME);
        assert!(err.contains("step limit"), "{err}");
        let src = "fn f(n: i64) -> i64 { f(n + 1) + 1 }\nprintln(f(0));";
        let (code, _, err) = go(&["run", "--max-frames", "30", "p.neon"], &[("p.neon", src)], "");
        assert_eq!(code, EXIT_RUNTIME);
        assert!(err.contains("stack overflow"), "{err}");
    }

    #[test]
    fn modules_are_found_through_the_vfs() {
        let files = [
            ("main.neon", "import { twice } from \"util\";\nprintln(twice(21));"),
            ("util.neon", "export fn twice(x: i64) -> i64 { x * 2 }"),
        ];
        let (code, out, err) = go(&["main.neon"], &files, "");
        assert_eq!((code, out.as_str()), (0, "42\n"), "{err}");
    }

    #[test]
    fn the_flags_do_not_change_program_behavior() {
        let src = "fn fib(n: i32) -> i32 { if n < 2 { n } else { fib(n - 1) + fib(n - 2) } }\nprintln(fib(15));";
        for flags in [&[][..], &["--no-opt"], &["--no-peephole"], &["--no-opt", "--no-peephole", "--gc-stress"]] {
            let mut args = vec!["run"];
            args.extend_from_slice(flags);
            args.push("p.neon");
            let (code, out, _) = go(&args, &[("p.neon", src)], "");
            assert_eq!((code, out.as_str()), (0, "610\n"), "{flags:?}");
        }
    }
}
