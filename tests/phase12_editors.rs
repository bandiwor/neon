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

/// Under valgrind a `fork` before `exec` reports leaks of the (soon replaced) child; the server itself
/// is covered by `phase11_lsp`, so these process-driving tests step aside there.
fn under_valgrind() -> bool {
    std::env::var("LD_PRELOAD").is_ok_and(|v| v.contains("vgpreload"))
}

fn find(candidates: &[String]) -> Option<String> {
    candidates.iter().find(|c| Command::new(c).arg("--version").output().is_ok()).cloned()
}

fn vim() -> Option<String> {
    if under_valgrind() {
        return None;
    }
    find(&["vim".into()])
}

fn nvim() -> Option<String> {
    if under_valgrind() {
        return None;
    }
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

/// Runs a Lua script in headless nvim with the plugin on the runtimepath; returns what it wrote to `out.txt`.
fn run_nvim_script(nvim: &str, dir: &Path, body: &str) -> String {
    let lua = format!(
        "vim.opt.runtimepath:prepend({editor:?})\nvim.cmd('filetype plugin indent on | syntax on')\nlocal out = {{}}\n{body}\nvim.fn.writefile(out, {outfile:?})\nvim.cmd('qa!')\n",
        editor = editor_dir().display().to_string(),
        outfile = dir.join("out.txt").display().to_string(),
    );
    let script = dir.join("check.lua");
    std::fs::write(&script, lua).unwrap();
    let output = Command::new(nvim)
        .args(["--headless", "-u", "NONE", "-i", "NONE", "-l"])
        .arg(&script)
        .env("HOME", dir)
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .output()
        .expect("nvim runs");
    std::fs::read_to_string(dir.join("out.txt"))
        .unwrap_or_else(|_| panic!("no result file; nvim said: {}", String::from_utf8_lossy(&output.stderr)))
}

/// Runs `body` inside a live Neovim main loop (so insert-mode events fire between key presses, as for
/// a person typing). `body` gets `finish(lines)`, which writes `out.txt` and quits.
fn run_nvim_interactive(nvim: &str, dir: &Path, body: &str) -> String {
    let lua = format!(
        r#"
vim.opt.runtimepath:prepend({editor:?})
vim.cmd('filetype plugin indent on | syntax on')
local function finish(lines)
  vim.fn.writefile(lines, {outfile:?})
  vim.cmd('qa!')
end
vim.defer_fn(function() finish({{ 'timeout' }}) end, 20000)
-- types each key list into a fresh neon buffer and collects the buffer text (lines joined by `|`)
local function type_cases(cases, done)
  local out, ci, ki = {{}}, 1, 0
  local function step()
    if ci > #cases then return done(out) end
    local keys = cases[ci]
    if ki == 0 then
      vim.cmd('enew! | setfiletype neon')
      vim.api.nvim_input('i')
      ki = 1
    elseif ki <= #keys then
      vim.api.nvim_input(keys[ki])
      ki = ki + 1
    else
      vim.api.nvim_input('<Esc>')
      vim.defer_fn(function()
        out[#out + 1] = table.concat(vim.api.nvim_buf_get_lines(0, 0, -1, false), '|')
        ci, ki = ci + 1, 0
        step()
      end, 40)
      return
    end
    vim.defer_fn(step, 25)
  end
  vim.defer_fn(step, 100)
end
{body}
"#,
        editor = editor_dir().display().to_string(),
        outfile = dir.join("out.txt").display().to_string(),
    );
    let script = dir.join("check.lua");
    std::fs::write(&script, lua).unwrap();
    let _ = std::fs::remove_file(dir.join("out.txt"));
    let output = Command::new(nvim)
        .args(["--headless", "-u", "NONE", "-i", "NONE", "-S"])
        .arg(&script)
        .env("HOME", dir)
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .output()
        .expect("nvim runs");
    std::fs::read_to_string(dir.join("out.txt")).unwrap_or_else(|_| {
        panic!(
            "no result file; nvim said: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// A Lua list of key presses: every character of `typed`, then the `keys` as given.
fn keys(typed: &str, keys: &[&str]) -> String {
    let mut list: Vec<String> = typed.chars().map(|c| format!("{:?}", c.to_string())).collect();
    list.extend(keys.iter().map(|k| format!("{k:?}")));
    format!("{{ {} }}", list.join(", "))
}

#[test]
fn nvim_enter_opens_bodies_with_indentation() {
    let Some(nvim) = nvim() else {
        eprintln!("skipped: nvim not installed");
        return;
    };
    let dir = scratch("enter");
    let body = format!(
        r#"
vim.g.neon_smart_enter = 1
type_cases({{
  {c1},
  {c2},
  {c3},
  {c4},
  {c5},
  {c6},
}}, function(out)
  vim.g.neon_smart_enter = 0
  type_cases({{ {c7} }}, function(out2)
    out[#out + 1] = out2[1]
    finish(out)
  end)
end)
"#,
        // after `{` + Enter: plain indentation
        c1 = keys("fn new() -> Point {", &["<CR>", "x"]),
        // the case of the report: the closer is already there
        c2 = keys("fn new() -> Point {}", &["<Left>", "<CR>", "x"]),
        c3 = keys("foo([])", &["<Left>", "<Left>", "<CR>", "1"]),
        c4 = keys("fn f() {", &["<CR>", "let a = 1;", "<CR>", "}"]),
        c5 = keys("ab", &["<CR>", "c"]),
        // nested: the outer body is indented, so the new line is indented further
        c6 = keys("fn f() {", &["<CR>", "if x {}", "<Left>", "<CR>", "y"]),
        c7 = keys("fn g() {}", &["<Left>", "<CR>", "x"]),
    );
    let out = run_nvim_interactive(&nvim, &dir, &body);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "fn new() -> Point {|    x");
    assert_eq!(lines[1], "fn new() -> Point {|    x|}");
    assert_eq!(lines[2], "foo([|    1|])");
    assert_eq!(lines[3], "fn f() {|    let a = 1;|}");
    assert_eq!(lines[4], "ab|c");
    assert_eq!(lines[5], "fn f() {|    if x {|        y|    }", "nested: {out}");
    assert_eq!(lines[6], "fn g() {|x}", "disabled: a plain newline");
}

#[test]
fn nvim_format_command_with_and_without_the_server() {
    let Some(nvim) = nvim() else {
        eprintln!("skipped: nvim not installed");
        return;
    };
    let dir = scratch("format");
    let messy = "fn main(){\\nlet x=1+2;\\n      println(x);\\n}\\n";
    let body = |lsp: bool| {
        format!(
            r#"
require("neon").setup({{ cmd = {{ {bin:?}, "lsp" }}, lsp = {lsp} }})
vim.cmd("enew! | setfiletype neon")
vim.api.nvim_buf_set_lines(0, 0, -1, false, vim.split("{messy}", "\n", {{ plain = true }}))
if {lsp} then
  vim.lsp.start({{ name = "neon", cmd = {{ {bin:?}, "lsp" }} }}, {{ bufnr = 0 }})
  vim.wait(10000, function() return #vim.lsp.get_clients({{ bufnr = 0 }}) > 0 end)
end
vim.cmd("NeonFormat")
out[#out + 1] = table.concat(vim.api.nvim_buf_get_lines(0, 0, -1, false), "|")
-- broken source is left alone
vim.api.nvim_buf_set_lines(0, 0, -1, false, {{ "fn main( {{" }})
vim.cmd("silent! NeonFormat")
out[#out + 1] = table.concat(vim.api.nvim_buf_get_lines(0, 0, -1, false), "|")
"#,
            bin = env!("CARGO_BIN_EXE_neon"),
        )
    };
    let want = "fn main() {|    let x = 1 + 2;|    println(x);|}";
    for lsp in [false, true] {
        let out = run_nvim_script(&nvim, &dir, &body(lsp));
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], want, "lsp={lsp}");
        assert_eq!(lines[1], "fn main( {", "lsp={lsp}");
    }
}
