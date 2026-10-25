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

let &cpo = s:cpo_save
unlet s:cpo_save
