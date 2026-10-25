# neon.nvim

Neovim support for the Neon language: filetype detection, syntax highlighting,
indentation, the language server (`neon lsp`) and a few commands.

Requires Neovim ≥ 0.8 (tested with 0.10). It does **not** need `nvim-lspconfig`
or an LSP package from Mason.

## Why Mason does not help

Mason installs servers from its own registry; `neon` is not in it. The server is
simply the `neon` binary, which you build once with cargo:

```sh
git clone https://github.com/bandiwor/neon && cd neon
cargo install --path .          # installs ~/.cargo/bin/neon
neon --version                  # must work in the same shell that starts nvim
```

`~/.cargo/bin` has to be in the `$PATH` that Neovim sees (a GUI or a launcher may
have a different one). If you do not want to touch `$PATH`, give the full path in
`cmd` below.

## Installation with lazy.nvim

The plugin lives in the `editors/nvim` subdirectory of the repository, so lazy
cannot install it straight from GitHub — point it at your clone with `dir`:

```lua
-- ~/.config/nvim/lua/plugins/neon.lua
return {
  {
    dir = "~/src/neon/editors/nvim",     -- path to your clone
    name = "neon.nvim",
    ft = "neon",
    -- the plugin is lazy-loaded by file type, so the file type must be known before it loads
    init = function()
      vim.filetype.add({ extension = { neon = "neon" } })
    end,
    opts = {
      -- cmd = { "/home/me/.cargo/bin/neon", "lsp" },   -- if neon is not in $PATH
      keymaps = true,          -- gd, gr, K, <leader>rn, <leader>ds, <C-s> in insert mode
    },
  },
}
```

`opts` is passed to `require("neon").setup()`. Because of `ft = "neon"` the
plugin is loaded when the first `.neon` file is opened.

Use `dir` with a path to your clone; the repository is not a plugin by itself,
because the plugin sits in a subdirectory.

### Options

| option | default | meaning |
|---|---|---|
| `cmd` | `{ "neon", "lsp" }` | how to start the server |
| `lsp` | `true` | start the server for `neon` buffers |
| `root_markers` | `{ ".git", "neon.toml" }` | project root detection |
| `keymaps` | `false` | buffer-local `gd gr K <leader>rn <leader>ds` and `<C-s>` (signature help) |
| `semantic_tokens` | `true` | highlight with the server's semantic tokens on top of the regex syntax |
| `capabilities` | `nil` | extra client capabilities; `nvim-cmp` / `blink.cmp` are picked up automatically |
| `on_attach` | `nil` | `function(client, bufnr)` called after the built-in setup |
| `format_on_save` | `false` | format the buffer before every write |
| `commands` | `true` | define the commands below |

Neovim 0.10+ already maps `K` (hover) and `grn`/`gra`/`grr` (0.11) by default;
`keymaps = true` also maps `<leader>f` to the formatter, and is for older versions or if you want the classic mappings.

### Commands

| | |
|---|---|
| `:NeonRun [options]` | run the current file in a terminal split (`--gc-stress`, `--stats`, ...) |
| `:NeonCheck` | compile errors only |
| `:NeonDisasm`, `:NeonIr` | bytecode / optimized IR |
| `:NeonFormat` | format the buffer (`neon fmt`; through the server if attached, else by piping the text through `neon fmt -`) |
| `:NeonRepl` | the REPL |
| `:NeonLspRestart` | restart the server (e.g. after `cargo install` of a new build) |
| `:checkhealth neon` | checks the binary, the filetype and the server |

### Enter, indentation and formatting

* **Enter after `{`** (also `(` and `[`) indents the next line, and typing `}` dedents it.
* **Enter between a pair**: with the cursor in `fn new() -> Point {|}` pressing Enter gives

  ```
  fn new() -> Point {
      |
  }
  ```

  Neovim watches the text for this (no `<CR>` mapping), so it works with or
  without an auto-pairs plugin and next to completion plugins. Turn it off with
  `let g:neon_smart_enter = 0`. In plain Vim a buffer-local `<CR>` mapping does
  the same.
* **Formatting**: `:NeonFormat`, `vim.lsp.buf.format()`, `format_on_save = true`,
  or any formatter plugin: for conform.nvim use
  `formatters_by_ft = { neon = { "neon_fmt" } }` with
  `formatters.neon_fmt = { command = "neon", args = { "fmt", "-" }, stdin = true }`.
  Files with syntax errors are not touched.

## Using lspconfig instead

If you prefer to manage servers with `nvim-lspconfig`, register `neon` as a
custom one and set `lsp = false` in the plugin so it is not started twice:

```lua
local configs = require("lspconfig.configs")
if not configs.neon then
  configs.neon = {
    default_config = {
      cmd = { "neon", "lsp" },
      filetypes = { "neon" },
      root_dir = require("lspconfig.util").root_pattern(".git", "neon.toml"),
      single_file_support = true,
    },
  }
end
require("lspconfig").neon.setup({})
```

## Only syntax highlighting (no plugin manager)

```sh
mkdir -p ~/.config/nvim && cp -r editors/nvim/{ftdetect,ftplugin,indent,syntax,lua} ~/.config/nvim/
```

(Vim works too for `ftdetect`, `ftplugin`, `indent` and `syntax`; the smart Enter has a plain-Vimscript fallback there.)

## What the server supports

Diagnostics (compiler errors with their `help:` notes, on every change, also for
unsaved buffers), hover, go to definition, references, document highlight,
rename, document and workspace symbols, formatting, completion (locals, items, imports,
struct fields and methods after `.`, enum variants after `::`), signature help,
and semantic tokens. Rename and references work across the import graph of the
open document.

## Troubleshooting

* `:checkhealth neon` says the binary is not found → fix `$PATH` or set `cmd`.
* Enter does not indent → `:checkhealth neon` (with a `.neon` file open) shows the buffer's
  `indentexpr`; it must be `GetNeonIndent()`. If another plugin replaced it, set it again in a
  `FileType neon` autocmd.
* `:LspInfo` (or `:checkhealth vim.lsp`) shows whether the client is attached to
  the buffer; the server log is in `:lua vim.cmd.edit(vim.lsp.get_log_path())`.
* Run `neon lsp` by hand: it should sit silently waiting for input on stdin.
* No highlighting at all → `:set filetype?` must print `filetype=neon`.
