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

  local detected = vim.filetype.match({ filename = "x.neon" }) == "neon"
  if not detected then
    local ok_get, found = pcall(vim.api.nvim_get_autocmds, { group = "filetypedetect", pattern = "*.neon" })
    detected = ok_get and #found > 0
  end
  if detected then
    ok("`*.neon` files are detected as filetype `neon`")
  else
    warn("`*.neon` is not detected as filetype `neon`; add `vim.filetype.add({ extension = { neon = \"neon\" } })` "
      .. "(needed when the plugin is lazy-loaded by file type) or put the plugin on the runtimepath")
  end

  -- the health buffer is not a neon buffer: look at the neon buffer the user came from
  local buf
  for _, b in ipairs(vim.api.nvim_list_bufs()) do
    if vim.api.nvim_buf_is_loaded(b) and vim.bo[b].filetype == "neon" then
      buf = b
      break
    end
  end
  if buf then
    local indentexpr = vim.bo[buf].indentexpr
    if indentexpr == "GetNeonIndent()" then
      ok(("indentation: indentexpr=GetNeonIndent(), shiftwidth=%d, expandtab=%s"):format(
        vim.bo[buf].shiftwidth, tostring(vim.bo[buf].expandtab)))
    else
      warn(("`indentexpr` of the neon buffer is `%s`, not `GetNeonIndent()`; another plugin replaced it"):format(indentexpr))
    end
    if vim.b[buf].neon_enter_attached then
      ok("Enter between brackets opens an indented body")
    elseif vim.g.neon_smart_enter == 0 then
      ok("smart Enter is disabled (g:neon_smart_enter = 0)")
    else
      warn("smart Enter is not active in the neon buffer (ftplugin/neon.vim did not run?)")
    end
  else
    ok("no `.neon` buffer is open; open one and run :checkhealth neon again to check indentation")
  end

  local clients = vim.lsp.get_clients and vim.lsp.get_clients({ name = "neon" }) or vim.lsp.get_active_clients({ name = "neon" })
  if #clients > 0 then
    ok(("language server running (%d client%s)"):format(#clients, #clients == 1 and "" or "s"))
  else
    warn("no Neon language server is attached to a buffer right now (open a `.neon` file)")
  end
end

return M
