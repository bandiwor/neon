" Vim filetype plugin
" Language: Neon

if exists('b:did_ftplugin')
  finish
endif
let b:did_ftplugin = 1

let s:cpo_save = &cpo
set cpo&vim

setlocal commentstring=//\ %s
setlocal comments=s1:/*,mb:*,ex:*/,://
setlocal formatoptions-=t
setlocal formatoptions+=croql
setlocal suffixesadd=.neon
setlocal expandtab
setlocal shiftwidth=4
setlocal softtabstop=4

let b:undo_ftplugin = 'setlocal commentstring< comments< formatoptions< suffixesadd< expandtab< shiftwidth< softtabstop<'

" <Enter> between a bracket pair opens a body: `{|}` becomes `{`, an indented line, `}`.
" Neovim watches the text (lua/neon/enter.lua), which composes with completion and auto-pairs
" plugins; Vim gets a plain <CR> mapping. Disable with `let g:neon_smart_enter = 0`.
if has('nvim-0.8')
  " keep our indentexpr even if a plugin (nvim-treesitter, ...) sets its own afterwards
  lua require('neon.enter').ensure_indent(vim.api.nvim_get_current_buf())
endif
if get(g:, 'neon_smart_enter', 1)
  if has('nvim-0.8')
    lua require('neon.enter').attach(0)
  else
    inoremap <buffer><expr><silent> <CR> NeonSmartEnter()
    let b:undo_ftplugin .= ' | silent! iunmap <buffer> <CR>'
  endif
endif

if !exists('*NeonSmartEnter')
  function! NeonSmartEnter() abort
    let l:line = getline('.')
    let l:col = col('.')
    let l:open = strpart(l:line, l:col - 2, 1)
    let l:close = strpart(l:line, l:col - 1, 1)
    if !pumvisible() && l:open !=# '' && stridx('{([', l:open) >= 0 && l:close ==# {'{': '}', '(': ')', '[': ']'}[l:open]
      return "\<CR>\<C-o>O"
    endif
    return "\<CR>"
  endfunction
endif

let &cpo = s:cpo_save
unlet s:cpo_save
