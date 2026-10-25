-- :checkhealth neon
local M = {}

function M.check()
  local h = vim.health
  local start = h.start or h.report_start
  local ok = h.ok or h.report_ok
  local warn = h.warn or h.report_warn
  local err = h.error or h.report_error

  start("neon.nvim")
  if vim.fn.has("nvim-0.8") == 1 then
    ok("Neovim " .. tostring(vim.version()))
  else
    err("Neovim 0.8 or newer is required")
  end

  local cmd = require("neon").config.cmd
  if vim.fn.executable(cmd[1]) == 1 then
    local path = vim.fn.exepath(cmd[1])
    local version = vim.fn.system({ cmd[1], "--version" }):gsub("%s+$", "")
    ok(("`%s` found: %s (%s)"):format(cmd[1], path, version))
  else
    err(("`%s` is not executable. Run `cargo install --path .` in the neon repository (puts it in ~/.cargo/bin) "
      .. "and make sure that directory is in the $PATH Neovim sees."):format(cmd[1]))
  end

  if vim.fn.exists("#filetypedetect") == 1 and vim.filetype.match({ filename = "x.neon" }) == "neon" then
    ok("`*.neon` files are detected as filetype `neon`")
  else
    warn("`*.neon` is not detected as filetype `neon`; is the plugin's directory on the runtimepath?")
  end

  local clients = vim.lsp.get_clients and vim.lsp.get_clients({ name = "neon" }) or vim.lsp.get_active_clients({ name = "neon" })
  if #clients > 0 then
    ok(("language server running (%d client%s)"):format(#clients, #clients == 1 and "" or "s"))
  else
    warn("no Neon language server is attached to a buffer right now (open a `.neon` file)")
  end
end

return M
