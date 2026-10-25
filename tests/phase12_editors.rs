//! Editor integration (`editors/nvim`): syntax groups, indentation, and the Neovim LSP client
//! talking to the real `neon lsp`. Each test skips (with a note) when vim/nvim is not installed.
//! Set `NEON_NVIM=/path/to/nvim` to use a Neovim that is not in `$PATH`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn editor_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("editors/nvim")
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("neon-editors-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn find(candidates: &[String]) -> Option<String> {
    candidates.iter().find(|c| Command::new(c).arg("--version").output().is_ok()).cloned()
}

fn vim() -> Option<String> {
    find(&["vim".into()])
}

fn nvim() -> Option<String> {
    find(&[std::env::var("NEON_NVIM").unwrap_or_else(|_| "nvim".into())])
}

fn run_vim(vim: &str, script: &Path, file: &Path, dir: &Path) -> String {
    let status = Command::new(vim)
        .args(["-u", "NONE", "-i", "NONE", "-N", "-es"])
        .arg("--cmd")
        .arg(format!("set runtimepath^={}", editor_dir().display()))
        .arg("-S")
        .arg(script)
        .arg(file)
        .current_dir(dir)
        .status()
        .expect("vim runs");
    assert!(status.success() || std::fs::metadata(dir.join("out.txt")).is_ok(), "vim failed");
    std::fs::read_to_string(dir.join("out.txt")).expect("script writes out.txt")
}

#[test]
fn syntax_groups() {
    let Some(vim) = vim() else {
        eprintln!("skipped: vim not installed");
        return;
    };
    let dir = scratch("syntax");
    let src = "\
// a comment TODO
const LIMIT: i64 = 10;
fn add(a: i32, b: i32) -> i32 {
    var total = a + b;
    if total > 0x1F { total = 1.5e3 as i32; } elif total == 0 { total = 'x' as i32; } else { return 0; }
    let s = \"tab\\t\\q\";
    let r = parse(s)?;
    Shape::Circle(r)
}
";
    let file = dir.join("t.neon");
    std::fs::write(&file, src).unwrap();
    // (line, text to find on that line, expected group suffix after "neon" for its first character)
    let expect: &[(usize, &str, &str)] = &[
        (1, "//", "LineComment"),
        (1, "TODO", "Todo"),
        (2, "const", "Declaration"),
        (2, "LIMIT", "Constant"),
        (2, "i64", "Type"),
        (2, "10", "Number"),
        (3, "fn", "Declaration"),
        (3, "add", "Function"),
        (3, "i32", "Type"),
        (3, "->", "Operator"),
        (4, "var", "Declaration"),
        (4, "+", "Operator"),
        (5, "if", "Conditional"),
        (5, "0x1F", "Number"),
        (5, "1.5e3", "Float"),
        (5, "elif", "Conditional"),
        (5, "'x'", "Character"),
        (5, "else", "Conditional"),
        (5, "return", "Statement"),
        (6, "\"tab", "String"),
        (6, "\\t", "Escape"),
        (6, "\\q", "EscapeError"),
        (7, "parse", "Function"),
        (7, "?", "Try"),
        (8, "Circle", "Variant"),
    ];
    let lines: Vec<&str> = src.lines().collect();
    let mut vimscript = String::from("let g:bad = []\n");
    for (l, needle, g) in expect {
        let c = lines[l - 1].find(needle).unwrap_or_else(|| panic!("`{needle}` not on line {l}")) + 1;
        vimscript += &format!(
            "let n = synIDattr(synID({l}, {c}, 1), 'name')\nif n !=# 'neon{g}' | call add(g:bad, '{l}:{c} want neon{g}, got ' . n) | endif\n"
        );
    }
    vimscript += "call writefile(g:bad, 'out.txt')\nqa!\n";
    let script = dir.join("check.vim");
    // filetype detection + syntax come from the plugin's own runtime files
    std::fs::write(&script, format!("filetype plugin indent on\nsyntax on\nset filetype=neon\n{vimscript}")).unwrap();
    let out = run_vim(&vim, &script, &file, &dir);
    assert!(out.trim().is_empty(), "syntax mismatches:\n{out}");
}

#[test]
fn indentation() {
    let Some(vim) = vim() else {
        eprintln!("skipped: vim not installed");
        return;
    };
    let dir = scratch("indent");
    let flat = "fn main() {\nlet a = [\n1,\n2,\n];\nmatch a {\n_ => {\nprint(1);\n}\n}\n}\n";
    let want = "fn main() {\n    let a = [\n        1,\n        2,\n    ];\n    match a {\n        _ => {\n            print(1);\n        }\n    }\n}\n";
    let file = dir.join("t.neon");
    std::fs::write(&file, flat).unwrap();
    let script = dir.join("check.vim");
    std::fs::write(
        &script,
        "filetype plugin indent on\nsyntax on\nset filetype=neon\nnormal! gg=G\ncall writefile(getline(1, '$'), 'out.txt')\nqa!\n",
    )
    .unwrap();
    let out = run_vim(&vim, &script, &file, &dir);
    assert_eq!(out, want);
}

#[test]
fn nvim_lsp_attaches_and_reports() {
    let Some(nvim) = nvim() else {
        eprintln!("skipped: nvim not installed");
        return;
    };
    let dir = scratch("lsp");
    let file = dir.join("t.neon");
    std::fs::write(&file, "fn add(a: i64, b: i64) -> i64 {\n    return a + b;\n}\n\nfn main() {\n    let y: i64 = \"oops\";\n    print(add(1, 2));\n}\n").unwrap();
    let lua = format!(
        r#"
vim.opt.runtimepath:prepend({editor:?})
vim.cmd("filetype plugin indent on | syntax on")
require("neon").setup({{ cmd = {{ {bin:?}, "lsp" }} }})
vim.cmd("edit " .. vim.fn.fnameescape({file:?}))
local out = {{}}
out[#out + 1] = "ft=" .. vim.bo.filetype
local attached = vim.wait(10000, function() return #vim.lsp.get_clients({{ bufnr = 0 }}) > 0 end)
out[#out + 1] = "attached=" .. tostring(attached)
vim.wait(10000, function() return #vim.diagnostic.get(0) > 0 end)
for _, d in ipairs(vim.diagnostic.get(0)) do out[#out + 1] = "diag=" .. d.lnum .. ":" .. d.message end
vim.api.nvim_win_set_cursor(0, {{ 7, 12 }})
local res = vim.lsp.buf_request_sync(0, "textDocument/hover", vim.lsp.util.make_position_params(), 5000)
for _, r in pairs(res or {{}}) do out[#out + 1] = "hover=" .. (r.result and r.result.contents.value or "none") end
local def = vim.lsp.buf_request_sync(0, "textDocument/definition", vim.lsp.util.make_position_params(), 5000)
for _, r in pairs(def or {{}}) do out[#out + 1] = "def=" .. (r.result and r.result[1] and r.result[1].range.start.line or "none") end
vim.fn.writefile(out, {outfile:?})
vim.cmd("qa!")
"#,
        editor = editor_dir().display().to_string(),
        bin = env!("CARGO_BIN_EXE_neon"),
        file = file.display().to_string(),
        outfile = dir.join("out.txt").display().to_string(),
    );
    let script = dir.join("check.lua");
    std::fs::write(&script, lua).unwrap();
    let output = Command::new(&nvim)
        .args(["--headless", "-u", "NONE", "-i", "NONE", "-l"])
        .arg(&script)
        .env("HOME", &dir)
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .output()
        .expect("nvim runs");
    let out = std::fs::read_to_string(dir.join("out.txt"))
        .unwrap_or_else(|_| panic!("no result file; nvim said: {}", String::from_utf8_lossy(&output.stderr)));
    assert!(out.contains("ft=neon"), "{out}");
    assert!(out.contains("attached=true"), "{out}");
    assert!(out.contains("diag=5:mismatched types"), "{out}");
    assert!(out.contains("fn add(a: i64, b: i64) -> i64"), "{out}");
    assert!(out.contains("def=0"), "{out}");
}
