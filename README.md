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
neon run <file>       compile and run (also: neon <file>)
neon check <file>     report errors without running
neon disasm <file>    print the bytecode (-a / --annotate: a `# r1 = r1 + 4` note on every line)
neon ir <file>        print the optimized intermediate representation (--effects: and the
                      const / pure / impure verdict for every function)
neon repl             interactive session
neon fmt <file>...    formatter (-w rewrites files, --check verifies, no file = stdin)
neon lsp              language server on stdin/stdout (see "Editor support")

--no-opt  --no-peephole      turn optimizers off
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
  (monomorphization), there are no trait objects.
* **Modules**: `import { f } from "dir/file"`, `import "dir/file" as m`,
  `export`. A module DAG is built through the virtual file system.
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
| `s.parse_i64()`, `s.parse_f64()` | `s.try_parse_i64()`, `s.try_parse_f64()` → `Result<_, string>` |
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
signature help, document highlights, formatting and semantic tokens. Unsaved buffers are
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
| `cli.rs`, `repl.rs` | The command line and the replay-based REPL, written against traits. |

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

| workload | vm | vm-noopt | ir-interp | VM speed |
|---|---|---|---|---|
| `fib(25)` recursion | 11.8 ms | 14.5 ms | 23.7 ms | 155 M insn/s |
| tight loop, 1M iterations (`%`, `+`) | 30.3 ms | 55.2 ms | 51.4 ms | 132 M insn/s |
| sieve, 100k | 12.2 ms | 19.2 ms | 17.9 ms | 139 M insn/s |
| float loop, 500k | 30.5 ms | 40.5 ms | 38.5 ms | 98 M insn/s |
| closure calls, 500k | 31.6 ms | 38.9 ms | 61.9 ms | 158 M insn/s |
| structs + enums, 250k | 31.8 ms | 54.5 ms | 106.0 ms | 147 M insn/s |
| map insert + lookup, 100k | 11.6 ms | 14.4 ms | – | 86 M insn/s |
| string concat + split, 50k | 25.7 ms | 31.7 ms | – | 29 M insn/s |
| garbage-heavy, 150k iterations | 43.9 ms (37 GCs) | 58.6 ms | – | 51 M insn/s |
| same, 64 KiB heap limit | 40.1 ms (1210 GCs) | 50.6 ms | – | 56 M insn/s |

Before the optimizer passes, the loop-invariant motion, the loop rotation, the
open-addressing map table and the inline struct fields, the same machine gave
fib 12.4 ms, loop 33.5 ms, sieve 14.3 ms, structs + enums 52.0 ms, map 26.8 ms,
strings 41.6 ms and garbage 54.1 ms (66 GCs). The biggest wins are in the heap
(`map`, `strings`, `structs`): maps no longer rehash with SipHash twice or
rebuild their index on removal, and structs and enums with up to four fields keep
their words inside the object slot instead of a separate allocation (which also
halves the number of collections).

`compile/*` measures source → bytecode, including the prelude: about
1.3 MiB/s of source on this machine (400 generated functions in ~89 ms;
roughly 2 ms of fixed prelude cost for a tiny program). Parsing is ~3% of
the total; the optimizer passes roughly triple compile time (12 ms without them, 42 ms with
them on the 200-function benchmark) in exchange for the 20–45% instruction-count
reduction visible in the `vm` vs `vm-noopt` columns.

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
