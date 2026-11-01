//! The standard modules: Neon source embedded in the compiler. `import "sys" as sys;` finds them before
//! it looks at the project's files, so their names are taken. A function without a body is *native*: it
//! is bound, by the name `module.function`, to a [`Builtin`](crate::builtin::Builtin) that the virtual
//! machine and the C translation implement. Everything else in them is ordinary Neon.

/// The name a standard module is registered under in the source map (and shown in diagnostics).
pub fn display_name(module: &str) -> String {
    format!("<std:{module}>")
}

/// The module name behind a registered path, if it is one of the standard modules.
pub fn module_of(path: &str) -> Option<&str> {
    path.strip_prefix("<std:")?.strip_suffix('>')
}

/// The source of the standard module `name`.
pub fn source(name: &str) -> Option<&'static str> {
    MODULES.iter().find(|(n, _)| *n == name).map(|(_, s)| *s)
}

/// Every standard module, with its source.
pub const MODULES: &[(&str, &str)] =
    &[("sys", include_str!("sys.neon")), ("fs", include_str!("fs.neon")), ("term", include_str!("term.neon"))];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::{compile_source, Options};
    use crate::host::MemFiles;
    use crate::vfs::MemVfs;
    use crate::vm::{run, BufferIo, VmConfig};

    /// Compiles and runs `src` with the given host; returns (output, status of the run).
    fn exec(src: &str, io: &mut BufferIo) -> Result<(), String> {
        let c = compile_source("t.neon", src, &MemVfs::new(), &Options::default());
        assert!(!c.diags.has_errors(), "{}", c.render_diagnostics());
        run(c.bytecode.as_ref().unwrap(), io, &VmConfig::default()).result.map_err(|e| e.trap.to_string())
    }

    #[test]
    fn registered_paths_name_their_module() {
        assert_eq!(module_of(&display_name("fs")), Some("fs"));
        assert_eq!(module_of("fs.neon"), None);
        assert!(source("sys").unwrap().contains("fn args"));
        assert!(source("nope").is_none());
    }

    #[test]
    fn the_modules_run_against_a_host_in_memory() {
        let files = MemFiles::new().with_file("notes/a.txt", "x\ny\n");
        let mut io = BufferIo::new().with_files(files).with_env("HOME", "/home/me").with_keys(b"a\x1b[B");
        let src = r#"import "fs" as fs;
import "sys" as sys;
import "term" as term;

println(fs::read_lines("notes/a.txt"));
println(fs::write_file("notes/b.txt", "z"));
println(fs::list_dir("notes"));
println(fs::read_file("notes/zzz"));
println(sys::env("HOME"));
println(sys::platform());
println(term::read_key());
println(term::read_key());
println(term::read_key());
"#;
        exec(src, &mut io).unwrap();
        assert_eq!(
            io.output,
            "Result::Ok([\"x\", \"y\"])\nResult::Ok(())\nResult::Ok([\"a.txt\", \"b.txt\"])\n\
             Result::Err(\"notes/zzz: not found\")\nOption::Some(\"/home/me\")\ntest\n\
             Option::Some(\"a\")\nOption::Some(\"down\")\nOption::None\n"
        );
        assert_eq!(io.files.unwrap().read("notes/b.txt").unwrap(), "z");
    }

    #[test]
    fn a_host_without_files_says_so_and_exit_is_a_trap_with_the_code() {
        let mut io = BufferIo::new();
        exec("import \"fs\" as fs;\nprintln(fs::exists(\"a\"));\nprintln(fs::read_file(\"a\"));", &mut io).unwrap();
        assert_eq!(io.output, "false\nResult::Err(\"the file system is not available here\")\n");
        let mut io = BufferIo::new();
        let r = exec("import \"sys\" as sys;\nsys::exit(4);", &mut io);
        assert_eq!(r, Err("exit 4".to_string()));
    }

    #[test]
    fn user_modules_may_not_shadow_a_standard_one_and_may_import_it() {
        let vfs = MemVfs::new()
            .with_file("sys.neon", "export fn args() -> i64 { 1 }")
            .with_file("lib/util.neon", "import \"sys\" as sys;\nexport fn argc() -> i64 { sys::args().len() }");
        let c = compile_source(
            "t.neon",
            "import \"sys\" as sys;\nimport { argc } from \"lib/util\";\nprintln(sys::args().len() + argc());",
            &vfs,
            &Options::default(),
        );
        assert!(!c.diags.has_errors(), "{}", c.render_diagnostics());
        let mut io = BufferIo::new().with_args(&["a", "b"]);
        run(c.bytecode.as_ref().unwrap(), &mut io, &VmConfig::default()).result.unwrap();
        assert_eq!(io.output, "4\n");
    }
}
