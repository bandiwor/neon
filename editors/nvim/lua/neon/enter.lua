-- Insert-mode <CR> for Neon buffers.
--
-- Between a bracket pair, `{|}` + <Enter> opens a body:
--
--     fn new() -> Point {
--         |
--     }
--
-- The indentation of the middle line comes from the indent file. In every other place <CR> keeps
-- doing what it did before (plain newline, or the mapping of nvim-cmp / nvim-autopairs / ...).
local M = {}

local closing = { ["{"] = "}", ["("] = ")", ["["] = "]" }

local function keys(s)
  return vim.api.nvim_replace_termcodes(s, true, false, true)
end

--- The global insert-mode <CR> mapping this buffer-local one shadows, if any.
local function previous()
  for _, m in ipairs(vim.api.nvim_get_keymap("i")) do
    if m.lhs == "<CR>" then
      return m
    end
  end
end

function M.expr()
  local line = vim.api.nvim_get_current_line()
  local col = vim.fn.col(".")
  local open = line:sub(col - 1, col - 1)
  if vim.fn.pumvisible() == 0 and closing[open] and line:sub(col, col) == closing[open] then
    return keys("<CR><C-o>O")
  end
  local prev = previous()
  if prev then
    if prev.callback then
      local ok, result = pcall(prev.callback)
      if ok and prev.expr == 1 and type(result) == "string" then
        return keys(result)
      end
      return ""
    elseif prev.rhs and prev.rhs ~= "" then
      if prev.expr == 1 then
        local ok, result = pcall(vim.api.nvim_eval, prev.rhs)
        return ok and keys(tostring(result)) or keys("<CR>")
      end
      return keys(prev.rhs)
    end
  end
  return keys("<CR>")
end

return M
