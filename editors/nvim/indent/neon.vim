" Vim indent file
" Language: Neon
"
" Brackets drive the indentation: a line that ends with `{`, `(` or `[`
" indents the next one, a line that starts with `}`, `)` or `]` dedents itself.

if exists('b:did_indent')
  finish
endif
let b:did_indent = 1

setlocal indentexpr=GetNeonIndent()
setlocal indentkeys=0{,0},0),0],!^F,o,O
setlocal nosmartindent
setlocal nocindent

let b:undo_indent = 'setlocal indentexpr< indentkeys< smartindent< cindent<'

if exists('*GetNeonIndent')
  finish
endif

function! GetNeonIndent() abort
  let l:prev = prevnonblank(v:lnum - 1)
  if l:prev == 0
    return 0
  endif
  " ignore a trailing line comment on the previous line
  let l:text = substitute(getline(l:prev), '\s*//.*$', '', '')
  let l:ind = indent(l:prev)
  if l:text =~# '[{(\[]\s*$'
    let l:ind += shiftwidth()
  endif
  if getline(v:lnum) =~# '^\s*[})\]]'
    let l:ind -= shiftwidth()
  endif
  return l:ind < 0 ? 0 : l:ind
endfunction
