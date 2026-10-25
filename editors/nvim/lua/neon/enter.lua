-- Opens a body when Enter is pressed between a bracket pair.
--
--     fn new() -> Point {|}      <Enter>      fn new() -> Point {
--                                                 |
--                                             }
--
-- This watches the text instead of mapping <CR>, so it works together with every other plugin that
-- touches Enter (completion menus, auto-pairs, ...) and with whatever the user typed it with. The
-- default Enter leaves the closer on a line of its own with the cursor in front of it; here the
-- missing middle line is inserted and indented.
local M = {}

local closing = { ["{"] = "}", ["("] = ")", ["["] = "]" }

local state = {} ---@type table<integer, table>

local function snapshot(buf)
  local row, col = unpack(vim.api.nvim_win_get_cursor(0))
  local line = vim.api.nvim_get_current_line()
  return {
    row = row,
    lines = vim.api.nvim_buf_line_count(buf),
    before = line:sub(col, col),
    after = line:sub(col + 1, col + 1),
  }
end

local function indent_string(width)
  if vim.bo.expandtab then
    return string.rep(" ", width)
  end
  local ts = vim.bo.tabstop
  return string.rep("\t", math.floor(width / ts)) .. string.rep(" ", width % ts)
end

--- Compares the cursor surroundings with the previous call; expands the pair when a single new
--- line appeared right between an opener and its closer.
function M.check(buf)
  if vim.g.neon_smart_enter == 0 or vim.api.nvim_get_current_buf() ~= buf then
    return
  end
  local prev, cur = state[buf], snapshot(buf)
  state[buf] = cur
  if not (prev and cur.lines == prev.lines + 1 and cur.row == prev.row + 1) then
    return
  end
  if closing[prev.before] ~= prev.after then
    return
  end
  -- the closer must have moved down with the cursor: "<indent>|}"
  local col = vim.api.nvim_win_get_cursor(0)[2]
  local line = vim.api.nvim_get_current_line()
  if line:sub(col + 1, col + 1) ~= prev.after or not line:sub(1, col):match("^%s*$") then
    return
  end
  local width = vim.fn.indent(cur.row - 1) + vim.fn.shiftwidth()
  local text = indent_string(width)
  vim.api.nvim_buf_set_lines(buf, cur.row - 1, cur.row - 1, false, { text })
  vim.api.nvim_win_set_cursor(0, { cur.row, #text })
  state[buf] = snapshot(buf)
end

--- Starts watching `buf` (called from the filetype plugin).
function M.attach(buf)
  buf = (buf == nil or buf == 0) and vim.api.nvim_get_current_buf() or buf
  if vim.b[buf].neon_enter_attached then
    return
  end
  vim.b[buf].neon_enter_attached = true
  local group = vim.api.nvim_create_augroup("NeonEnter" .. buf, { clear = true })
  vim.api.nvim_create_autocmd({ "InsertEnter", "CursorMovedI", "TextChangedI" }, {
    group = group,
    buffer = buf,
    desc = "Neon: open a body when Enter is pressed between brackets",
    callback = function()
      M.check(buf)
    end,
  })
  vim.api.nvim_create_autocmd({ "InsertLeave", "BufUnload" }, {
    group = group,
    buffer = buf,
    callback = function()
      state[buf] = nil
    end,
  })
end

return M
