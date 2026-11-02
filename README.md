# Neon

A small, statically typed language with a register bytecode VM and a precise
mark-and-sweep garbage collector, written in pure Rust — edition 2021,
**standard library only**, no third-party crates.

```neon
enum Shape { Circle(f64), Rect(f64, f64), Empty }

fn area(s: Shape) -> f64 {
    match s {
        Shape::Circle(r) => 3.14159 * r * r,
        Shape::Rect(w, h) => w * h,
        Shape::Empty => 0.0,
    }
}

var counter = 0;
let bump = || => { counter += 1; counter };
bump(); bump();
println(counter);                        // 2
println(area(Shape::Rect(3.0, 4.0)));    // 12.0
```

## Quick start

```sh
cargo build --release
target/release/neon run tests/corpus/sample_bank.neon
target/release/neon repl
```

```
neon run <file> [-- args...]   compile and run (also: neon <file>); what follows `--` is `sys::args()`
neon check <file>     report errors without running
neon disasm <file>    print the bytecode (-a / --annotate: a `# r1 = r1 + 4` note on every line)
neon ir <file>        print the optimized intermediate representation (--effects: and the
                      const / pure / impure verdict for every function)
neon c <file>         translate to one self-contained C file (stdout, or -o out.c)
neon repl             interactive session
neon fmt <file>...    formatter (-w rewrites files, --check verifies, no file = stdin)
neon lsp              language server on stdin/stdout (see "Editor support")

--no-opt  --no-peephole      turn optimizers off
-j N, --threads N            compiler threads (0 = every core, 1 = none; NEON_THREADS does the same)
--gc-limit 64k               cap the heap; exceeding it is a run-time error
--gc-stress                  collect at every safe-point (finds missing roots)
--max-steps N  --max-frames N
--stats                      collector statistics on stderr
```

Exit status: `0` success, `1` compile error, `2` run-time error, `64` usage.
Diagnostics (with source snippets) go to stderr, program output to stdout.

## The language in one page

* **Types**: `i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 bool char string`, arrays
  `[T]`, maps `{K: V}`, sets `set { .. }`, structs, enums with payloads,
  function types `fn(A) -> B`, generics with trait bounds.
* **Bindings**: `let` is an immutable variable, `var` a mutable one, `const` a
  compile-time constant. Module-level `let`/`var` are globals initialized when
  the program starts. Types are inferred locally.
* **Number literals**: an unsuffixed integer literal is `i64` and a float literal
  is `f64` unless the context says otherwise (`let a: u8 = 5`, `f(1.5f32)`).
  An integer literal also stands for a float where a float is required, so
  `let x: f64 = 15`, `x / 2`, `2 * x`, `x < 3` and `P { y: 1 }` need no `.0`.
  Only literals do this: a variable `n: i64` is never converted implicitly
  (`x * n` is an error, write `x * (n as f64)`).
* **`let` vs `const`**: both cannot be reassigned, but a `let` is a *run-time*
  value (its initializer may call functions, build arrays, read input), while a
  `const` is computed by the compiler from literals, operators, casts and other
  constants, and substituted at every use. Constants have no storage, can be
  `export`ed and imported like any item, and fail at compile time if they
  overflow or divide by zero (`const X: i8 = 100 + 100;` is an error, even if
  `X` is never used). `const` is module-level only; its value must be a number,
  `bool`, `char` or `string`.
* **Control flow**: `if`/`elif`/`else` and `match` are expressions (`else if`
  is an error with a hint; so is `let mut`, write `var`); `while`, `for x in
  xs`, ranges `a..b` / `a..=b`, `break`, `continue`, `return`.
* **Patterns**: literals, ranges, `|` alternatives, bindings, enum variants with
  nested payload patterns, `_`, and `if` guards. Matches must be exhaustive.
* **Closures**: `|x| => x + 1`, `|| => { n += 1; n }`. Captured variables are
  shared and mutable; each loop iteration captures its own variables.
* **Traits**: `trait`/`impl`, default methods, operator traits (`Add`, `Mul`,
  `Neg`, `Eq`, `Ord`, ...), `Display` used by `println`. Resolution is static
  (monomorphization), there are no trait objects. Bounds go in `<T: A + B>` or in a `where` clause
  (`fn f<T>(x: T) where T: A + B`); both work on functions, impls, traits, structs and enums.
* **Choosing an impl**: of several impls that apply, the *narrower* one wins whatever the order they are
  written in — `impl<T> Show for T` next to `impl Show for bool`, or `impl<T> W<T> { fn get }` next to
  `impl W<i64> { fn get }`. Two impls that cannot be told apart are an error where the second is written
  (E0272), and so are two that overlap with neither inside the other (E0273).
* **Your own iterables**: implement `Iterator<T>` (`fn next(var self) -> Option<T>`) and `for x in value`
  calls `next` until it answers `None`; `value` must be a `var` when it is a variable, since the loop
  advances it. It also works through a bound: `fn total<I: Iterator<i64>>(var it: I)`.
* **Limits of generics**: a generic function that calls itself with a bigger type each time
  (`f<T>` calling `f<[T]>`) is stopped after 64 levels of nesting with an error instead of
  instantiating for ever; the solver follows impl bounds 64 levels deep.
* **Modules**: `import { f } from "dir/file"`, `import "dir/file" as m` (then `m::f()`),
  `export`. A module DAG is built through the virtual file system. A bare name that belongs to the
  standard library (`"sys"`, `"fs"`, `"term"`) is a [standard module](#the-standard-modules).
* **Arithmetic is checked**: integer overflow, division by zero, bad shifts,
  out-of-range indexing and missing map keys are run-time errors, never
  undefined behavior. Float → int casts saturate; NaN casts to 0.

### Handling run-time errors: `Option` and `Result`

Every operation that can stop the program has a variant that returns a value
instead. `Option<T>` (`Some` / `None`) and `Result<T, E>` (`Ok` / `Err`) are
defined in the prelude, with methods written in Neon itself: `is_some`,
`is_none`, `unwrap`, `expect`, `unwrap_or`, `unwrap_or_else`, `map`,
`and_then`, `or`, `ok_or` (and `is_ok`, `is_err`, `map_err`, `ok`, `err` for
`Result`). A program can still declare its own `Option`; it shadows the
built-in one.

| Plain (traps) | Handled |
|---|---|
| `a + b`, `-`, `*`, `/`, `%`, `<<`, `>>`, `pow`, `-x` | `a.checked_add(b)`, `checked_sub`, `checked_mul`, `checked_div`, `checked_rem`, `checked_shl`, `checked_shr`, `checked_pow(n)`, `x.checked_neg()` → `Option<T>` |
| `xs[i]` | `xs.get(i)` → `Option<T>` |
| `xs.pop()`, `xs.remove(i)` | `xs.try_pop()`, `xs.try_remove(i)` → `Option<T>` |
| `m[k]` | `m.get(k)` → `Option<V>` |
| `s.parse_i64()`, `s.parse_u64()`, `s.parse_f64()` | `s.try_parse_i64()`, `s.try_parse_u64()`, `s.try_parse_f64()` → `Result<_, string>` |
| `s.char_at(i)`, `s.substring(a, b)` | `s.get_char(i)` → `Option<char>`, `s.try_substring(a, b)` → `Result<string, string>` |
| `s.find(sub)` (−1 if absent) | `s.index_of(sub)` → `Option<i64>` |
| `input()` (empty string at EOF) | `read_line()` → `Option<string>` (`None` at EOF) |

```neon
match "12x".try_parse_i64() {
    Result::Ok(n) => println(n + 1),
    Result::Err(e) => println("bad input: " + e),   // bad input: cannot parse `12x`
}
let safe = 250u8.checked_add(10u8).unwrap_or(255u8);  // 255
```

### Tuples

`(a, b)` builds a tuple, `(A, B)` is its type, `t.0` reads an element, and tuples are taken apart in `let`,
`for` and `match` patterns:

```
fn minmax(a: i64, b: i64) -> (i64, i64) { if a < b { (a, b) } else { (b, a) } }
let (lo, hi) = minmax(9, 4);
for (n, name) in [(1, "a"), (2, "b")] { println(f"{n}{name}"); }
println((1, "x"));                      // (1, "x")
```

Tuples have 2 to 8 elements and are immutable. A tuple is an ordinary one-variant enum of the prelude
(`Tuple2::T(a, b)`), so exhaustiveness, printing, the garbage collector and the C translation need nothing of
their own; the optimizer replaces tuples that do not escape (also ones built in two branches and merged
afterwards) by plain registers. Top-level `let` still binds a single name, so destructure inside a function.

### Compact enums

An enum whose payloads are all `bool`, `char` or integers of at most 32 bits, and fit into the 7 bytes after
the tag, is **not an object at all**: it is one 64-bit word, the variant number in the lowest byte and the
fields in the other seven, each at an offset that is a multiple of its own size (so `Rgb(u8, u8, u8)`,
`Option<char>`, `(u8, i16)` and `Result<u8, bool>` all fit; `Option<i64>`, `Option<string>` or `(u32, u32)`
do not and stay heap objects). Nothing in the language changes: such enums are built, matched, compared in
patterns, passed, stored in structs, arrays and maps, printed and used with `?` exactly like the others.
What changes is the cost. Building one is a couple of shifts and ors, a `match` is a mask and a compare, a
field is a shift and a cast, the allocator and the garbage collector never see them, and in the generated C
they are plain `uint64_t` locals the C compiler keeps in registers.

The layout is a pure function of the type descriptor (`ir::packed_enum`), which the final `legalize` pass,
the printers, the collector's object layouts and the C translation all ask, so the two representations cannot
drift apart. In the IR a compact enum is built and taken apart by the same `NewEnum` / `EnumTag` /
`EnumField` / `BranchTag` as any enum, in a register of type `packed`: until the very last pass the
optimizer treats it as the object it is, so private ones are scalarized and a `match` on a known variant is
decided at compile time, exactly as for heap enums. Only the words that really are passed, returned,
stored or merged are turned into arithmetic. Builtins that return an `Option` (`arr.get`, `checked_add`,
...) still build their result on the heap; lowering reads a packed result out of it immediately.

### Ranges and lazy iterators

`a..b`, `a..=b` and `a..` are values (`Range<T>`, `RangeInclusive<T>`: fields `start` / `end`, methods `contains`,
`is_empty`). `xs.iter()` or a range starts a **lazy iterator**, and the chain is compiled into **one loop**:
no array is built between the steps, no closure object exists, the closures are inlined.

```
let evens = (1..=100).filter(|n| => n % 2 == 0).map(|n| => n * n).take(5).collect();
let total = xs.iter().zip(weights.iter()).map(|(x, w)| => x * w).sum();
for (i, line) in lines.iter().enumerate().skip(1) { println(f"{i}: {line}"); }
```

| steps that give another iterator | steps that end the chain |
|---|---|
| `map` `filter` `filter_map` `flat_map` `take` `skip` `take_while` `skip_while` `step_by` `enumerate` `zip` `chain` `rev` `inspect` | `for_each` `fold` `reduce` `collect` `count` `sum` `product` `any` `all` `find` `position` `first` `last` `nth` `max` `min` `max_by_key` `min_by_key` `join` |

`|(a, b)| => ..` takes a tuple argument apart. Laziness is Rust's: `take(2)` pulls two elements, `any` stops at the
first hit. The price of compiling the chain away: an iterator exists **only inside one chain** (it may be bound
with `let it = ..` and used several times inside a function, each use starts over), so it cannot be passed to a
function, returned or stored: end the chain with `collect()` first. `rev()` must come right after the source (or
`map` / `filter`), the right side of `zip` must be an array or a range (possibly with `map`s), and
`flat_map` closures return arrays. On a 3-million-element array a `map -> filter -> sum` chain runs as fast as the
hand-written loop; the array methods of the previous section (`xs.map(..)`) still build a new array per step.

### Array combinators

Arrays have the usual functional methods, written in Neon itself (in the prelude) and taking closures,
so chains read left to right and the closures' parameter types are inferred:

```
let words = ["pear", "fig", "apple"];
words.filter(|w| => w.len() > 3).map(|w| => w.to_upper()).for_each(|w| => println(w));
println([5, 3, 8].sorted());            // [3, 5, 8]  (a copy; `sort()` sorts in place)
println([5, 3, 8].fold(0, |a, x| => a + x));
```

| method | |
|---|---|
| `map(f)` `filter(p)` `filter_map(f)` `flat_map(f)` `for_each(f)` | new array / side effects |
| `fold(init, f)` `reduce(f)` | accumulate (`reduce` → `Option`) |
| `any(p)` `all(p)` `count(p)` `find(p)` `position(p)` | search (`find`, `position` → `Option`) |
| `first()` `last()` `take(n)` `skip(n)` `reversed()` | slicing (copies) |
| `sum()` `product()` | numeric arrays (every integer and float type) |
| `max()` `min()` `sorted()` | element types with an order |

`Option` also has `map`, `and_then`, `filter`, `is_some_and`, `or`, `or_else`, `unwrap_or`,
`unwrap_or_else`, `expect`, `ok_or`; `Result` has the matching set. Each combinator builds a new array, so
a long chain is a few times slower than one hand-written loop; ranges are not values yet, so
`(0..n).map(..)` is not available (use `for`).

### Nested options and arrays of options

`flatten()` peels every layer of a nested option: `Option<Option<Option<T>>>` becomes `Option<T>` (the same
for `Result<Result<T, E>, E>` and `[[T]]`). An array of options turns into an option of an array when
none is missing — `[Some(1), Some(2)].sequence()` is `Some([1, 2])`, a single `None` makes it `None`
(`[Result<T, E>]` gives the first error) — and `flatten()` on `[Option<T>]` keeps the present values.

### The standard modules

`import "sys" as sys;` and the members are reached with `::`: `sys::args()`. The modules are Neon source
carried by the compiler (`src/stdlib/*.neon`); a function declared without a body is native, bound by the
name `module.function` to a builtin that the VM and the C translation implement, so the same program
runs both ways. The bare names are reserved; your own files are imported with a path.

| Module | What it has |
| --- | --- |
| `sys` | `args()` (what follows `--`), `exit(code)`, `env(name)` → `Option<string>`, `platform()`, `now_ms()`, `ticks_ms()`, `sleep(ms)` |
| `fs` | `read_file`, `write_file`, `append_file`, `exists`, `is_file`, `is_dir`, `list_dir`, `create_dir`, `remove`, `rename`; and in Neon `read_lines`, `write_lines`, `create_dir_all`, `copy`. Failures are `Result::Err("path: reason")` with the same reasons everywhere (`not found`, `permission denied`, `already exists`, ...) |
| `term` | keys without Enter: `raw(on)`, `read_key()`, `poll_key(ms)`, `width()`, `height()`, `is_tty()`; and drawing helpers `clear`, `move_to`, `hide_cursor`, `enter_screen`, `paint`, ... |

A key is a name: the character itself (`"a"`, `" "`, `"я"`) or `"enter"`, `"esc"`, `"up"`, `"pagedown"`,
`"f5"`, with a modifier in front (`"ctrl+c"`, `"alt+x"`, `"shift+tab"`). In raw mode a key arrives the moment
it is pressed: `neon run` switches the terminal with `stty`, the C translation uses termios (POSIX) or
`conio` (Windows, not tested). When the input is not a terminal the keys are read from it as the bytes a
terminal would send, so a program driven by keys can be driven by a file, and `poll_key` says `"eof"` once
it has ended. In the browser `fs` sees the project's files (writes last for one run).

### The REPL

`neon repl` at a terminal is a small editor, not a bare line reader:

* **Colors.** Keywords, types and variants, functions and methods, parameters, locals, globals, numbers,
  strings, operators (`->`, `=>` and the rest in red) and punctuation each have their own color, also inside
  `f"..."` interpolations. They come from the compiler, not from patterns: the lexer cuts the input into
  tokens and the language server's analysis of the session plus the input says what every name refers to
  (a parameter, a local, a global, a function, a method, a type, a variant), so `n` is a parameter in one
  function and a global in the next. An unfinished input is completed (a missing operand, `;`, closing
  brackets) before it is analyzed. 24-bit colors with `COLORTERM=truecolor`, the 256-color palette
  otherwise, none with `NO_COLOR`.
* **Live preview.** Under a complete expression a gray `↳ value` shows what Enter would print: `foo()` after
  `fn foo() -> i64 { 42 }` shows `↳ 42` as you type. The preview runs the cell on the side: the session replays
  its cells from scratch for every evaluation, so nothing is kept; expressions that print, ask for input,
  trap or run too long show nothing.
* **Completion.** Tab completes names visible at the cursor (the session's variables and functions, the
  prelude, keywords), the members of a value by its type (`xs.ma` → `map`, `max`) and `Enum::` variants,
  using the same analysis as the language server; Tab again cycles through the choices, Shift-Tab goes back.
* **Editing.** Left/Right (Ctrl-Left/Right by word), Home/End, Ctrl-A/E/K/U/W/Y, undo (Ctrl-_), bracketed
  paste, Alt-Enter for a line break; unfinished input (open brackets) continues on an indented line, and
  Enter between `{` and `}` opens a block. Up/Down move between the lines of the input and then through the
  history, Ctrl-R searches it. The history is kept in `~/.neon_history` (`NEON_HISTORY` picks another file;
  empty disables it).
* **Ctrl-C** stops the cell that is running (an endless loop, an `input()` waiting for a line) and
  keeps the session; at the prompt it drops the input. While a cell runs, the terminal is read by a
  separate thread, so the interruption is noticed within a moment; what you type for `input()` is echoed
  and can be corrected with Backspace.
* **Errors** talk about what you typed: positions are relative to the cell (`<repl>:1:5`), the code the
  REPL wraps around an expression is never shown, follow-up errors are dropped, and the severity, the
  arrow and the carets are colored. A forgotten final `;` (`let x = 5`) is not an error.
* **vi keys.** Esc switches to normal mode (the prompt becomes `[n]`, the cursor a block): `h j k l w b e 0 ^ $`,
  `i a I A o`, `x X D C dd dw d$ cw`, `p P u`; `k` and `j` walk the history. `i` goes back to inserting.

Without a terminal (a pipe, `TERM=dumb`, `NEON_REPL=plain`) the plain `>>>` loop is used. `:help` lists
the commands.

### In the browser

`web/` is a complete online compiler: the Rust engine built to WebAssembly, an editor with
completion, diagnostics, hover, rename and formatting (the language server runs inside the module), a console
with interactive input, IR / bytecode / C views and a REPL. Everything runs on the client and the whole
program can be shared as a link. Built with strict TypeScript, Vite (Terser + Lightning CSS) and SCSS
modules. See [web/README.md](web/README.md) (in Russian); the workflow
`.github/workflows/pages.yml` publishes it to GitHub Pages.

### Examples

* `examples/scene.neon` — a small software renderer (compact enums, tuples, ranges, fused iterator chains).
* `examples/paint.neon` — an interactive terminal paint program that reacts to every key press: arrows or
  `w a s d` move the brush, space stamps, `p` holds the pen down, `t` changes the tool, `+ -` the size,
  `1`-`5` the color, `u` undoes, `o` saves the picture to `paint.txt`, `q` quits; the brush blinks while you
  think (`poll_key`). `neon run examples/paint.neon`, or `neon c examples/paint.neon -o paint.c` for a native
  build; `-- --plain` prints every frame instead of redrawing in place.

### Format strings

`f"..."` strings embed expressions in `{}` with Python's format mini-language
(`{expr:spec}`):

```neon
let name = "world"; let n = 42; let pi = 3.14159;
println(f"hello {name}! n={n} pi={pi:.2f}");        // hello world! n=42 pi=3.14
println(f"[{n:>6}] [{n:<6}] [{n:^6}] [{n:06}]");    // [    42] [42    ] [  42  ] [000042]
println(f"{n:x} {n:#b} {1234567:,} {0.25:.1%}");    // 2a 0b101010 1,234,567 25.0%
println(f"{name:*^11}|{name:.3}|{{braces}}");       // ***world***|wor|{braces}
let w = 8;
println(f"[{pi:{w}.3f}]");                           // [   3.142]  (a specifier may hold fields)
println(5.format("04"));                             // 0005  (the same, as a method)
```

A specifier is `[[fill]align][sign][#][0][width][,|_][.precision][type]` with
align `< > ^ =`, sign `+ - space`, and type `d b o x X c` for integers,
`e E f F g G %` for floats, `s` for strings (booleans and characters print as
strings, or as numbers with `d b o x X`). Without a type a float prints like
`println` does. Fields hold any expression; a `:` inside brackets belongs to the
expression, and a closure's `|x: i32|` needs parentheses. A specifier written out
in the source is checked by the compiler (`E0263`/`E0264`); one built from fields
is checked when the code runs. `neon fmt` leaves format strings as they are.
Constant fields fold at compile time (`println(f"{1 + 2}:{"x":>3}")` is one
string), and `print(f"{n}")` prints the number without building a string.

### The `?` operator

`expr?` unwraps a `Some` / `Ok` and otherwise returns the failure from the
enclosing function (`None`, or `Err(e)` with the same error type):

```neon
fn total(items: [string]) -> Result<i64, string> {
    var sum: i64 = 0;
    for s in items { sum += s.trim().try_parse_i64()?; }
    Result::Ok(sum)
}
fn first_even(xs: [i64]) -> Option<i64> {
    let first = xs.get(0)?;
    if first % 2 == 0 { Option::Some(first) } else { Option::None }
}
```

It works in functions and closures, chains (`a.b()?.c()?`), and the compiler
checks that the function returns the same family: `?` on an `Option` needs an
`Option<_>` return type (use `.ok_or(error)` to turn it into a `Result`), `?` on
a `Result<_, E>` needs `Result<_, E>`.

What remains a trap: `panic`, `assert`, `unwrap`/`expect` on `None`/`Err`,
stack overflow and out-of-memory.

More programs live in `tests/corpus/`.

## Editor support

`neon lsp` is a language server (stdio, no dependencies). It reports the
compiler's own diagnostics (with `help:` notes) and offers hover, go to
definition, references, rename, document and workspace symbols, completion,
signature help, document highlights, formatting, semantic tokens and code actions
(missing `impl` methods, missing `match` arms and struct fields, `let` → `var`, typo fixes, `;`,
plain string → `f"..."`). Unsaved buffers are
analyzed through an overlay file system, and the importers of a changed module
are re-analyzed. Rename and references work across the import graph of the open
document; files that are not reachable from it are not searched.

`editors/nvim/` is a Neovim plugin: filetype detection, syntax highlighting,
indentation, `:NeonRun`/`:NeonCheck`/`:NeonDisasm`/`:NeonIr`/`:NeonRepl`, and
the client configuration for the server. Setup with lazy.nvim (Mason cannot
install `neon`, it is built with cargo) is in
[`editors/nvim/README.md`](editors/nvim/README.md).

### Formatter

`neon fmt file.neon` prints the formatted source, `neon fmt -w a.neon b.neon`
rewrites files, `neon fmt --check ...` exits with 1 if any file would change (for
CI), and `neon fmt` with no file filters stdin to stdout. The formatter is
deliberately conservative: it does not wrap or join lines, it only fixes
indentation (4 spaces; one more level for continuation lines such as `.method()`
chains), spacing around operators, commas and brackets, runs of blank lines,
trailing whitespace and the final newline. Comments are kept as they are. Before
anything is written, the result is lexed again and must contain exactly the
same tokens and comments; source with syntax errors is refused, not guessed at.

## Architecture

```
source ─▶ lexer ─▶ parser ─▶ sema ─▶ IR ─▶ IR optimizer ─▶ compiler ─▶ bytecode ─▶ VM
          syntax/            sema/    ir/    ir/opt/       compiler/    bytecode/   vm/ + gc/
```

| Module | What it does |
|---|---|
| `vfs/` | `Vfs` trait with `OsVfs` (native only) and `MemVfs`. All file access goes through it. |
| `diag/` | `FileId`/`Span`, one central `SourceMap`, `Diagnostic`s and an ASCII renderer. Positions are stable, which is what an LSP needs. |
| `syntax/` | Interner, token stream, a flat-arena AST and an error-resilient parser: panic-mode recovery leaves `Error` nodes so later items are still analyzed. |
| `sema/` | Module loading, name resolution (def-use table), Hindley–Milner-style local inference, trait solving, exhaustiveness checking. |
| `ir/` | Three-address code in basic blocks over virtual registers; generics are monomorphized during lowering. Passes: folding, value forwarding / CSE, loop-invariant motion, DCE and inlining, driven by an effect analysis (below). A reference interpreter acts as the test oracle. |
| `compiler/` | Block layout, linear-scan frame-slot allocation (≤ 256 slots) with a Lua-style sliding call window, stack maps. |
| `bytecode/` | Opcode table, 32-bit words with a 64-bit wide form, relaxing assembler, disassembler, liveness, peephole optimizer, verifier. |
| `gc/` | Handle-based object heap, upvalue cells, exact mark-and-sweep, root scanning from stack maps. |
| `vm/` | The register machine and the `VmIo` trait. |
| `runtime/` | Numeric semantics, formatting and builtins shared by the constant folder, the interpreter and the VM, so `127i8 + 1` means the same thing at compile time and at run time. |
| `fmt/` | The formatter (`neon fmt`, LSP formatting): re-indents by brackets, normalizes spacing (operators are classified with the parser's tree) and blank lines, keeps the author's line breaks and every comment, and verifies that the token stream is unchanged. |
| `lsp/` | Language server (`neon lsp`): JSON-RPC over stdio, a std-only JSON codec, an overlay VFS for unsaved buffers, and features built on the same analysis the compiler uses. |
| `cli.rs`, `repl/` | The command line and the replay-based REPL (`repl/`: the session, plus the terminal prompt: `highlight`, `editor`, `terminal`, `complete`, `history`, `interactive`). |

### Effect analysis

The optimizer asks one question of every piece of code: *what can it do?* The answer is a set
of effects (`src/effects.rs`): `reads` memory, `writes` memory, `io`, `traps`, `allocates`,
`may-diverge` (a loop or recursion) and `fragile` (valid only under a condition checked
elsewhere, like reading the payload of a variant). From it follows the usual verdict:

* **const**: depends only on the arguments;
* **pure**: may also read memory, but writes nothing and does no I/O;
* **impure**: writes or does I/O.

Every builtin has a fixed entry in an exhaustive table (`Builtin::effects`: `len` reads, `push`
writes, `println` is I/O, `Abs` may trap, `to_upper` allocates, ...). For user functions
`ir::effects::analyze` computes the least fixed point of
`effects(f) = local(f) ∪ ⋃ effects(callee)` over the call graph: strongly connected components
are solved callee-first, and any cycle or loop adds `may-diverge`. A call through a closure has
every effect. `neon ir --effects` prints the result for each function.

What the optimizer does with it:

| transformation | needs |
|---|---|
| drop an unused call or instruction (DCE) | no writes, I/O, traps or divergence (allocation does not count) |
| reuse the result of an identical earlier call (CSE) | no writes, I/O or allocation; if it reads, no write in between |
| move work out of a loop (LICM) | the above plus no allocation and no hidden precondition; if it reads, nothing in the loop writes |
| keep known field and element values across a call | the callee does not write |

A repeated call that traps is still reusable (the first execution already trapped), but an
unused one that traps is not removable.

### Escape analysis

Registers connected by moves may hold one object, so they form an *alias class*. A class is
*fresh* if everything it can hold was allocated by the function itself, and it *escapes* if the
object may be kept past the function: stored in the heap, a global or an upvalue, put into another
object, captured by a closure, handed to a closure, or passed to a callee that retains that
argument (builtins say which arguments they retain, user functions get a per-parameter summary
computed as a fixed point over the call graph; returning a parameter counts as keeping it). A fresh,
non-escaping class is *private*: nothing outside the function can tell what happens to its objects.

* Reads and writes of private objects no longer count in a function's effect summary, so a
  function that builds and fills a local array is `const` and merely allocates.
* A private struct or enum used only through field accesses is replaced by registers
  (scalar replacement). For enums the variant is known statically: tag checks become constants and
  `match` becomes a jump.
* A private array, map, set or struct that is only ever written to disappears with its writes.

### IR pass pipeline

`optimize` runs these until nothing changes, then inlines and runs them again:

| pass | what it does |
|---|---|
| `fold` | constant folding and propagation (trapping operations are kept), copy propagation |
| `sccp` | constant propagation over the whole CFG: values meet at joins, branches decided by a constant only feed the edge they take (arithmetic follows the run-time semantics, trapping operations stay) |
| `copies` | global copy propagation (available copies across blocks and loops) |
| `sroa`, `dead objects` | scalar replacement and removal of write-only private objects; also of structs that are rebuilt or copied in loops (`p = V { x: p.x + 1, .. }`) as long as no field is assigned in place |
| `simplify` | value numbering over extended basic blocks (also calls to const/pure functions and global reads), store-to-load forwarding for fields, elements and globals, known arrays and enums, integer identities, power-of-two division, `!c` branches, `print(a + b)`, dead element loads |
| `strings` | constant concatenation, merging of adjacent constant prints |
| `stores` | dead store elimination: a store overwritten later in the block with nothing able to read it in between (fields, elements, globals; constant indices are compared by value) |
| `dce` | unreachable blocks, jump threading (also on branches decided by a constant set in the jumping block), block merging, boolean diamonds (`if c { true } else { false }`), duplicating tiny return blocks, move coalescing, dead instructions and calls |
| `rotate` | `while` loops test at the bottom (the header is copied behind the body), and the part of the test that does not change in the loop, even trapping work such as `n - i`, runs once in the guard |
| `licm` | loop-invariant code motion (last, so the other passes see the loop condition); reads of private objects (`xs.len()` of an array only this function sees) move out of loops that write elsewhere; a loop-bottom copy of an instruction that already ran before the loop is dropped |
| `consts` | registers holding the same constant are merged (small constants that only feed additions stay separate: they become immediates) |
| `unroll` | loops of a few instructions that run at most 8 times become straight-line code: the loop is copied, `sccp` decides the exit tests, and the copy is kept only if the original loop becomes unreachable |
| `tail` | self tail calls become loops (after the first round) |
| `globals` | stores to globals nobody reads, unused global slots |
| `devirt` | calls of closures whose code is known become direct calls: a closure built just before the call, a global assigned once from a capture-less closure, the result of a function that always returns `closure f [its parameters]` (curried `\|x\| => \|y\| => ...`). Closures with captures are called through a lifted copy that takes the captured values as extra parameters; this is skipped when anything assigns the captured variables |
| `inline`, `prune` | inlining of small functions, removal of unreachable functions |
| `legalize` | last, always: compact enums (see [Compact enums](#compact-enums)), which every pass above treats as the objects they semantically are, become shifts, masks and compares on `u64`; the registers' type `packed` becomes `u64`, so no backend ever sees them. A cleanup round follows when anything was rewritten |

### Execution model

* Registers are untagged 64-bit words. Integers are kept in canonical form
  (signed values sign-extended, unsigned zero-extended).
* A call places its arguments in consecutive registers *above every live
  value*; the callee's frame **is** that window, and the result returns in the
  window's first register — no copying on call or return.
* Upvalues follow Lua: a closure first points at the live stack slot (open),
  and the variable moves to the heap when its scope ends (closed).
* The compiler records, for every instruction at which the collector may run,
  which registers hold live references (**stack maps**: bytecode liveness ∧ a
  reference-type dataflow). The collector scans exactly those, so integers that
  look like handles are never mistaken for references.
* Heap handles are 1-based; 0 is null. With `--gc-limit` the heap has a hard
  cap and traps with *out of memory* only if live data exceeds it.

### Translation to C

`neon c program.neon -o program.c` writes the optimized IR as C99 that builds with any C compiler
and cross-compiles like any C program. The program is one small file; the **runtime is the pair
`runtime/neon.h` + `runtime/neon.c`** (hand-written C, kept in the project, `neon runtime <dir>` writes
a copy out of the binary):

```sh
neon c tests/corpus/sample_sieve.neon -o sieve.c
cc -O2 -I runtime sieve.c runtime/neon.c -o sieve -lm
```

The generated file is `#include "neon.h"` and the program: its declarations (globals, string
constants, prototypes), its functions and `main` - no macros, no runtime, no system headers.
`neon.h` holds every system header the runtime needs, the type names (`i8`..`u64`, `f32`, `f64`,
`bool`, `chr`, `unit`), the hint and `FRAME` / `GC_POINT` macros, the prototypes of the runtime and
the checked arithmetic of every integer type as `static inline` functions (`add_i32`, `shr_u8`,
`chk_mul_i64`, `f2i_u8`, ...), so the C compiler inlines them. `neon.c` holds the allocator and the
collector, strings, arrays, maps, formatting, input and the `sys` / `fs` / `term` modules (with the
platform headers); build it once and link it with every program.

* Registers become typed locals (`int32_t v3`, `Obj *v7`); containers hold 64-bit words.
  Control flow is `goto` in the layout order of the bytecode compiler, so most jumps are
  fall-throughs. Single-assignment constants are folded into their uses.
* Integer arithmetic traps like the VM does (`add_i32`, `div_i64`, ... use
  `__builtin_*_overflow`); casts, float formatting (`1.0`, `1e16`, `NaN`), `{x:>8.2f}`
  format specifiers, UTF-8 strings and insertion-ordered maps behave like the VM — a
  differential test builds every corpus program with the system C compiler and compares.
* Runtime errors print `runtime error: ...` to stderr and exit with status 2, like `neon run`.
* **Debug and release are one macro.** By default (and with `-DNEON_CHECKS=1`) the file is a checked
  build: integer overflow, division by zero, bad shifts and recursion deeper than `NEON_MAX_DEPTH`
  (20000, `-DNEON_MAX_DEPTH=n`) end the program with an error. `cc -O3 -DNDEBUG` (or
  `-DNEON_CHECKS=0`) is the release build: integers wrap around, division by zero and `MIN / -1` are
  left to C (undefined), shift counts are masked, and the recursion is not counted. The checks
  sit inside the inline functions of `neon.h` and compile away. Array bounds, map
  keys, parsing, `assert` and `panic` stay checked in both, and `checked_add` & co. keep returning
  `Option`.
* Memory is a precise mark-and-sweep collector over a size-class allocator (objects up to 256 bytes are
  cut from 64 KB blocks and recycled through per-class free lists, an 8-byte header, no per-object
  `malloc`; bigger ones are plain `malloc` blocks): functions that hold references register them in a
  `Frame` (only those that can reach an allocation), collections run at `GC_POINT()`s
  before allocating instructions, variables captured by closures live in shared heap cells.
  `-DNEON_GC_STRESS` collects at every safe-point.
* Compiler hints, guarded by `__GNUC__`/`__clang__` so other compilers still work: `LIKELY` /
  `UNLIKELY` on every trap check, `NORETURN COLD` on the trap functions, `CONST_FN` / `PURE_FN` on
  user functions the effect analysis proves free of side effects (not recursive, no allocation,
  no traps; reading memory only for `PURE_FN`).
* IR inlining is left on, `--no-inline` turns it off. Measured with gcc -O3 on small programs, turning
  it off made structure- and closure-heavy code about 4.5x slower (the IR optimizer then no longer
  removes the temporary objects and the closure calls, which the C compiler cannot see through the
  collector's heap), `fib` 1.2-1.6x slower, and changed nothing for plain loops.
* Limits: structs and enum variants with more than 64 fields are rejected; case mapping and
  `is_alphabetic` cover ASCII, Latin, Greek, Cyrillic and the common CJK / kana / Hangul ranges
  (no full Unicode tables); the checked arithmetic needs GCC or Clang.
  Not tried here for lack of a sysroot: a real cross build (the code assumes only C99, 8/16/32/64-bit
  integers, IEEE floats and 32- or 64-bit pointers).

### WebAssembly

The compiler, analyzer and VM never touch `std::fs` or the process streams.
Files come from a `Vfs`, program I/O goes through `VmIo` (`BufferIo`,
`StreamIo<R, W>` or your own). `cargo build --lib --target wasm32-unknown-unknown`
is part of the checks below.

## Testing

```sh
cargo test                                    # unit + integration tests (the editor tests skip when vim/nvim is absent)
cargo build --lib --target wasm32-unknown-unknown
scripts/valgrind_test.sh                      # every test binary and the CLI under Valgrind
```

Highlights:

* `tests/corpus/*.neon` + `.out` — programs with known output. They are
  compiled under every optimizer setting, executed by the reference
  interpreter and by the VM, with and without a collection at *every*
  safe-point, and run through the real binary.
* `tests/phase13_c.rs` — the C backend: every corpus program is translated, built with `-Wall -Wextra
  -Werror -pedantic` (also with `-DNEON_GC_STRESS`), run and compared with the VM; run-time errors,
  `input()`/`read_line()` and the layout of the generated file (one include, no runtime copied into
  it) and of `runtime/` are checked as well. The test skips itself when there is no C compiler.
* `tests/phase9_math.rs` — boundary-value arithmetic for every numeric type,
  operator and cast, VM against interpreter.
* `tests/phase8_gc.rs`, `src/gc/tests.rs` — collector behavior under restricted
  limits, cycles, closures and randomized object graphs.
* Peephole optimizer: a randomized semantic-equivalence test against a tiny
  instruction interpreter.

### Valgrind gate

`scripts/valgrind_test.sh` runs every test binary (and the CLI on every corpus
program with `--gc-stress`) under
`valgrind --leak-check=full --show-leak-kinds=all --errors-for-leak-kinds=all --error-exitcode=1`
and requires `0 errors from 0 contexts`, `definitely lost: 0 bytes` and
`indirectly lost: 0 bytes`.

Rust's own runtime keeps a few process-lifetime allocations alive until exit
(the main thread's stack-guard bookkeeping, libtest's main-thread handle). With
`--errors-for-leak-kinds=all` valgrind reports them for *any* Rust program, even
an empty `main`. `scripts/valgrind.supp` suppresses exactly those, each entry
restricted to a std-internal frame and a leak kind; leaks in Neon code are
still reported (the suppressions were checked against a deliberately leaking
program).

The script also sets `RUST_BACKTRACE=0`: symbolizing a backtrace (printed by
`should_panic` tests) builds process-wide caches in std that are never freed.

## Benchmarks

```sh
cargo bench                        # full run (~3 min); results as a table
cargo bench -- fib compile         # only benchmarks whose name contains a filter word
NEON_BENCH_QUICK=1 cargo bench     # quick smoke run
```

`benches/vm.rs` is a std-only harness (`harness = false`, no external crates).
Every workload runs on the optimized VM (`vm`), on the VM with the IR
optimizer and peephole off (`vm-noopt`) and, for the smaller ones, on the
reference IR interpreter (`ir-interp`). Output is checked to be identical
across settings. Times are the median of repeated runs; "M insn/s" is VM
instructions executed per second.

Sample run (release build, one shared cloud core — your numbers will differ,
compare the *ratios*):

| workload | vm | vm-noopt | VM speed |
|---|---|---|---|
| `fib(25)` recursion | 10.5 ms | 15.3 ms | 127 M insn/s |
| tight loop, 1M iterations (`%`, `+`) | 35.8 ms | 54.4 ms | 112 M insn/s |
| sieve, 100k | 15.2 ms | 26.5 ms | 98 M insn/s |
| float loop, 500k | 25.3 ms | 44.8 ms | 119 M insn/s |
| closure calls, 500k | 42.8 ms | 45.8 ms | 117 M insn/s |
| structs + enums, 250k | 33.3 ms | 78.6 ms | 120 M insn/s |
| map insert + lookup, 100k | 9.8 ms | 14.4 ms | 81 M insn/s |
| string concat + split, 50k | 38.7 ms | 26.9 ms | 18 M insn/s |

(The machine was busier than for the earlier tables, so read the `vm` / `vm-noopt` ratio, not the
absolute times: fib 1.5x, loop 1.5x, sieve 1.7x, float loop 1.8x, structs + enums 2.4x.)

Before the optimizer passes, the loop-invariant motion, the loop rotation, the
open-addressing map table and the inline struct fields, the same machine gave
fib 12.4 ms, loop 33.5 ms, sieve 14.3 ms, structs + enums 52.0 ms, map 26.8 ms,
strings 41.6 ms and garbage 54.1 ms (66 GCs). The biggest wins are in the heap
(`map`, `strings`, `structs`): maps no longer rehash with SipHash twice or
rebuild their index on removal, and structs and enums with up to four fields keep
their words inside the object slot instead of a separate allocation (which also
halves the number of collections).

`compile/*` measures source → bytecode, including the prelude. The optimizer and the code
generator work on one function at a time, so they run on all cores (`std::thread::scope`, no
dependencies; on `wasm32` everything runs inline): optimizing functions in parallel, merging their string
constants in function order afterwards, and building the constant pool in function order after the
per-function code generation, make the output byte-for-byte independent of the thread count (a test
compares 1, 2, 4 and all cores). Phase times for 400 generated functions, from
`cargo run --release --example phases`:

| | 1 thread | 4 threads |
|---|---|---|
| parse + analysis + lowering | 9 ms | 9 ms |
| IR optimizer | 70 ms | 24 ms |
| register allocation, code generation, peephole | 10 ms | 4 ms |
| **total** | **90 ms** (was 142 ms) | **37 ms** |

Source → bytecode for several program shapes, before this work (revision `a005fb2`) and after, on a
4-core machine; the bytecode is identical in every row (`scripts/compare_compile.sh <revision>` rebuilds
the old revision and prints this table, with a fingerprint of the output):

| program | before | after, 1 thread | after, 4 threads | speed-up |
|---|---|---|---|---|
| hello (prelude only) | 0.35 ms | 0.34 ms | 0.35 ms | 1.0x |
| 10 functions | 4.7 ms | 3.5 ms | 3.6 ms | 1.3x |
| 100 functions | 35 ms | 23 ms | 12.5 ms | 2.8x |
| 400 functions | 145 ms | 96 ms | 38.6 ms | 3.7x |
| one `main` of 250 statements | 336 ms | 24 ms | 24 ms | 14x |
| one `main` of 1000 statements | 4933 ms | 219 ms | 209 ms | 24x |
| 400 small loops in one `main` | 414 ms | 38 ms | 38 ms | 11x |

On one thread the work went down by a third without changing a byte of the output: a worklist
liveness analysis, dense per-register tables instead of hash maps, no per-block allocations in the hot
passes, an unroller that rules out loops before copying them, and constant propagation that only runs
where something can change. Functions with thousands of statements used to grow super-linearly
(a 1000-statement `main` took 3.2 s; type-variable chains, whole-function liveness after every
peephole rewrite and an all-intervals scan per call were the culprits), now 0.27 s. The optimizer
passes still cost compile time (12 ms without them, ~70 ms with them on 400 functions) in exchange for
the 20–45% instruction-count reduction visible in the `vm` vs `vm-noopt` columns.

What the numbers say, honestly: the dispatch loop is a straightforward
`match` over decoded words, so the VM is ~1.5–2× faster than the reference
interpreter on call- and allocation-heavy code and about even on simple
arithmetic loops. Integer add/sub/mul/div/rem on `i64`/`i32`/`u64`/`u32` use
native checked operations (verified against an `i128` reference path); other
widths take the general path. Programs dominated by builtins (maps, strings)
are bound by those builtins rather than by dispatch.

## Limits

At most 256 registers per function (a function that needs more is reported as
a compile error), 255 captured variables per closure, 250 call arguments,
255 enum variants. Long array/set/map literals are built incrementally so
they do not hit the register limit.
