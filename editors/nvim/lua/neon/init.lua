-- neon.nvim: Neovim integration for the Neon language.
--
--   require("neon").setup({})
--
-- Filetype detection, syntax highlighting and indentation come from the plugin's runtime files
-- (ftdetect/, syntax/, indent/, ftplugin/). `setup` adds the language server (`neon lsp`) and a few
-- commands. It needs only Neovim >= 0.8; it does not depend on nvim-lspconfig or mason.

local M = {}

---@class NeonConfig
---@field cmd string[]            command that starts the server (default: { "neon", "lsp" })
---@field lsp boolean             start the language server for `neon` buffers (default: true)
---@field root_markers string[]   files/directories that mark a project root
---@field on_attach fun(client: table, bufnr: integer)|nil
---@field capabilities table|nil  LSP client capabilities (completion plugins' are merged in automatically)
---@field semantic_tokens boolean use the server's semantic highlighting (default: true)
---@field keymaps boolean         set buffer-local LSP keymaps (default: false)
---@field commands boolean        define :NeonRun, :NeonCheck, ... (default: true)
M.defaults = {
  cmd = { "neon", "lsp" },
  lsp = true,
  root_markers = { ".git", "neon.toml" },
  on_attach = nil,
  capabilities = nil,
  semantic_tokens = true,
  keymaps = false,
  commands = true,
}

M.config = vim.deepcopy(M.defaults)

local group = vim.api.nvim_create_augroup("NeonNvim", { clear = true })

local function warn(msg)
  vim.notify("neon.nvim: " .. msg, vim.log.levels.WARN)
end

--- Client capabilities, extended by nvim-cmp / blink.cmp when they are installed.
local function capabilities()
  local caps = vim.lsp.protocol.make_client_capabilities()
  local ok, cmp = pcall(require, "cmp_nvim_lsp")
  if ok then
    caps = vim.tbl_deep_extend("force", caps, cmp.default_capabilities())
  end
  local ok_blink, blink = pcall(require, "blink.cmp")
  if ok_blink and blink.get_lsp_capabilities then
    caps = blink.get_lsp_capabilities(caps)
  end
  return M.config.capabilities and vim.tbl_deep_extend("force", caps, M.config.capabilities) or caps
end

local function find_root(bufnr)
  local name = vim.api.nvim_buf_get_name(bufnr)
  local dir = name ~= "" and vim.fs.dirname(name) or vim.fn.getcwd()
  local found = vim.fs.find(M.config.root_markers, { path = dir, upward = true })[1]
  return found and vim.fs.dirname(found) or dir
end

local function set_keymaps(bufnr)
  local function map(lhs, rhs, desc)
    vim.keymap.set("n", lhs, rhs, { buffer = bufnr, desc = "Neon: " .. desc })
  end
  map("gd", vim.lsp.buf.definition, "go to definition")
  map("gr", vim.lsp.buf.references, "references")
  map("K", vim.lsp.buf.hover, "hover")
  map("<leader>rn", vim.lsp.buf.rename, "rename")
  map("<leader>ds", vim.lsp.buf.document_symbol, "document symbols")
  vim.keymap.set("i", "<C-s>", vim.lsp.buf.signature_help, { buffer = bufnr, desc = "Neon: signature help" })
end

local function on_attach(client, bufnr)
  if not M.config.semantic_tokens then
    client.server_capabilities.semanticTokensProvider = nil
  end
  if M.config.keymaps then
    set_keymaps(bufnr)
  end
  if M.config.on_attach then
    M.config.on_attach(client, bufnr)
  end
end

--- Starts (or reuses) the language server for a buffer.
function M.start(bufnr)
  bufnr = bufnr or vim.api.nvim_get_current_buf()
  local cmd = M.config.cmd
  if vim.fn.executable(cmd[1]) ~= 1 then
    warn(("`%s` was not found in $PATH. Install it with `cargo install --path .` in the neon repository, "
      .. "or set `cmd = { '/full/path/to/neon', 'lsp' }`."):format(cmd[1]))
    return nil
  end
  return vim.lsp.start({
    name = "neon",
    cmd = cmd,
    root_dir = find_root(bufnr),
    capabilities = capabilities(),
    on_attach = on_attach,
  }, { bufnr = bufnr })
end

--- Runs `neon <subcommand> <file>` in a terminal split.
local function run_in_terminal(subcommand, extra)
  local file = vim.api.nvim_buf_get_name(0)
  if file == "" then
    return warn("save the buffer first")
  end
  if vim.bo.modified then
    vim.cmd("silent write")
  end
  local args = { M.config.cmd[1], subcommand }
  vim.list_extend(args, extra or {})
  table.insert(args, file)
  local quoted = vim.tbl_map(vim.fn.shellescape, args)
  vim.cmd("botright 14split | terminal " .. table.concat(quoted, " "))
end

local function define_commands()
  vim.api.nvim_create_user_command("NeonRun", function(o)
    run_in_terminal("run", o.fargs)
  end, { nargs = "*", desc = "Run the current Neon file (extra arguments: neon run options)" })
  vim.api.nvim_create_user_command("NeonCheck", function()
    run_in_terminal("check")
  end, { desc = "Report compile errors of the current Neon file" })
  vim.api.nvim_create_user_command("NeonDisasm", function()
    run_in_terminal("disasm")
  end, { desc = "Show the bytecode of the current Neon file" })
  vim.api.nvim_create_user_command("NeonIr", function()
    run_in_terminal("ir")
  end, { desc = "Show the optimized IR of the current Neon file" })
  vim.api.nvim_create_user_command("NeonRepl", function()
    vim.cmd("botright 14split | terminal " .. vim.fn.shellescape(M.config.cmd[1]) .. " repl")
    vim.cmd("startinsert")
  end, { desc = "Open the Neon REPL in a terminal split" })
  vim.api.nvim_create_user_command("NeonLspRestart", function()
    for _, client in ipairs(vim.lsp.get_clients and vim.lsp.get_clients({ name = "neon" }) or vim.lsp.get_active_clients({ name = "neon" })) do
      client.stop()
    end
    vim.defer_fn(function()
      for _, buf in ipairs(vim.api.nvim_list_bufs()) do
        if vim.api.nvim_buf_is_loaded(buf) and vim.bo[buf].filetype == "neon" then
          M.start(buf)
        end
      end
    end, 300)
  end, { desc = "Restart the Neon language server" })
end

---@param opts NeonConfig|nil
function M.setup(opts)
  M.config = vim.tbl_deep_extend("force", vim.deepcopy(M.defaults), opts or {})
  vim.api.nvim_clear_autocmds({ group = group })
  if M.config.lsp then
    vim.api.nvim_create_autocmd("FileType", {
      group = group,
      pattern = "neon",
      desc = "Start the Neon language server",
      callback = function(args)
        M.start(args.buf)
      end,
    })
    -- the buffer that triggered lazy-loading is already a neon buffer
    for _, buf in ipairs(vim.api.nvim_list_bufs()) do
      if vim.api.nvim_buf_is_loaded(buf) and vim.bo[buf].filetype == "neon" then
        M.start(buf)
      end
    end
  end
  if M.config.commands then
    define_commands()
  end
end

return M
