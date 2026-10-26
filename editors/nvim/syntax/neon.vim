" Vim syntax file
" Language: Neon
" Works in Vim and Neovim; semantic highlighting from `neon lsp` adds to it.

if exists('b:current_syntax')
  finish
endif

let s:cpo_save = &cpo
set cpo&vim

syn case match

" ---- comments (line comments and nestable block comments) -----------------------------
syn keyword neonTodo contained TODO FIXME XXX NOTE HACK
syn region  neonLineComment  start="//" end="$" contains=neonTodo,@Spell keepend
syn region  neonBlockComment start="/\*" end="\*/" contains=neonBlockComment,neonTodo,@Spell

" ---- keywords ---------------------------------------------------------------------------
syn keyword neonDeclaration  let var const fn struct enum trait impl
syn keyword neonInclude      import export as
syn keyword neonConditional  if elif else match
syn keyword neonRepeat       while loop for in
syn keyword neonStatement    return break continue
syn keyword neonBoolean      true false
syn keyword neonSelf         self
syn keyword neonType         i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 bool char string
syn keyword neonSelfType     Self

" ---- literals ---------------------------------------------------------------------------
" integers (decimal, hex, binary, octal), `_` separators and type suffixes
syn match neonNumber "\<\d[0-9_]*\%(i8\|i16\|i32\|i64\|u8\|u16\|u32\|u64\)\=\>"
syn match neonNumber "\<0x[0-9a-fA-F_]\+\%(i8\|i16\|i32\|i64\|u8\|u16\|u32\|u64\)\=\>"
syn match neonNumber "\<0b[01_]\+\%(i8\|i16\|i32\|i64\|u8\|u16\|u32\|u64\)\=\>"
syn match neonNumber "\<0o[0-7_]\+\%(i8\|i16\|i32\|i64\|u8\|u16\|u32\|u64\)\=\>"
" floats: `1.5`, `1e3`, `2.5e-2`, `3.0f32`, `4f64` (but not `1..5` or `1.max`)
syn match neonFloat "\<\d[0-9_]*\.\d[0-9_]*\%([eE][+-]\=\d[0-9_]*\)\=\%(f32\|f64\)\=\>"
syn match neonFloat "\<\d[0-9_]*[eE][+-]\=\d[0-9_]*\%(f32\|f64\)\=\>"
syn match neonFloat "\<\d[0-9_]*\%(f32\|f64\)\>"

syn match  neonEscape      contained "\\[nrt0\\'"]"
syn match  neonEscape      contained "\\x\x\{2}"
syn match  neonEscape      contained "\\u{\x\{1,6}}"
syn match  neonEscapeError contained "\\[^nrt0\\'"xu]"
syn region neonString      start=+"+ skip=+\\\\\|\\"+ end=+"+ contains=neonEscape,neonEscapeError,@Spell
" format strings: f"text {expr:spec}" ({{ and }} are literal braces)
syn region neonFString     matchgroup=neonFStringQuote start=+\<f"+ skip=+\\\\\|\\"+ end=+"+ contains=neonEscape,neonEscapeError,neonFStrLitBrace,neonFStrField,@Spell
syn region neonFStrField   contained matchgroup=neonFStrBrace start=+{+ end=+}+ contains=TOP
syn match  neonFStrLitBrace contained "{{\|}}"
syn match  neonCharacter   "'\%([^'\\]\|\\[nrt0\\'"]\|\\x\x\{2}\|\\u{\x\{1,6}}\)'" contains=neonEscape

" ---- names --------------------------------------------------------------------------------
syn match neonFunction  "\<\h\w*\ze\s*("
syn match neonFunction  "\<\h\w*\ze::<"
syn match neonTypeName  "\<[A-Z]\w*\>"
syn match neonConstant  "\<[A-Z][A-Z0-9_]\+\>"
syn match neonPathSep   "::"
syn match neonVariant   "\%(::\)\@2<=[A-Z]\w*"

" declarations: the name after fn / struct / enum / trait
syn match neonFnName    "\<fn\s\+\zs\h\w*"
syn match neonTypeDecl  "\<\%(struct\|enum\|trait\)\s\+\zs\h\w*"

" ---- operators -------------------------------------------------------------------------------
" (a literal bar is written \%x7c: `|` would end the :syntax command)
syn match neonOperator "->\|=>\|\.\.=\=\|&&\|\%x7c\%x7c\|<<=\=\|>>=\=\|[-+*%&^]=\=\|/[/*]\@!=\=\|\%x7c=\=\|[<>!=]=\=\|\~"
syn match neonTry      "?"
syn match neonDelimiter "[,;]"

syn sync fromstart

" ---- highlight links --------------------------------------------------------------------------
hi def link neonTodo          Todo
hi def link neonLineComment   Comment
hi def link neonBlockComment  Comment
hi def link neonDeclaration   Keyword
hi def link neonInclude       Include
hi def link neonConditional   Conditional
hi def link neonRepeat        Repeat
hi def link neonStatement     Statement
hi def link neonBoolean       Boolean
hi def link neonSelf          Identifier
hi def link neonType          Type
hi def link neonSelfType      Type
hi def link neonNumber        Number
hi def link neonFloat         Float
hi def link neonEscape        SpecialChar
hi def link neonEscapeError   Error
hi def link neonString        String
hi def link neonFString       String
hi def link neonFStringQuote  String
hi def link neonFStrBrace     Special
hi def link neonFStrLitBrace  SpecialChar
hi def link neonCharacter     Character
hi def link neonFunction      Function
hi def link neonFnName        Function
hi def link neonTypeName      Type
hi def link neonTypeDecl      Type
hi def link neonConstant      Constant
hi def link neonVariant       Constant
hi def link neonPathSep       Delimiter
hi def link neonOperator      Operator
hi def link neonTry           Special
hi def link neonDelimiter     Delimiter

let b:current_syntax = 'neon'

let &cpo = s:cpo_save
unlet s:cpo_save
