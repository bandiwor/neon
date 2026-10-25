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

let mut counter = 0;
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
neon disasm <file>    print the bytecode
neon ir <file>        print the optimized intermediate representation
neon repl             interactive session

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
* **Bindings**: `let` / `let mut` / `const`; module-level bindings are globals.
  Types are inferred locally; integer and float literals adapt to context.
* **Control flow**: `if`/`else` and `match` are expressions; `while`, `for x in
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

What remains a trap: `panic`, `assert`, `unwrap`/`expect` on `None`/`Err`,
stack overflow and out-of-memory.

More programs live in `tests/corpus/`.

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
| `ir/` | Three-address code in basic blocks over virtual registers; generics are monomorphized during lowering. Folding, copy propagation, DCE and inlining. A reference interpreter acts as the test oracle. |
| `compiler/` | Block layout, linear-scan frame-slot allocation (≤ 256 slots) with a Lua-style sliding call window, stack maps. |
| `bytecode/` | Opcode table, 32-bit words with a 64-bit wide form, relaxing assembler, disassembler, liveness, peephole optimizer, verifier. |
| `gc/` | Handle-based object heap, upvalue cells, exact mark-and-sweep, root scanning from stack maps. |
| `vm/` | The register machine and the `VmIo` trait. |
| `runtime/` | Numeric semantics, formatting and builtins shared by the constant folder, the interpreter and the VM, so `127i8 + 1` means the same thing at compile time and at run time. |
| `cli.rs`, `repl.rs` | The command line and the replay-based REPL, written against traits. |

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
cargo test                                    # unit + integration tests
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
| `fib(25)` recursion | 12.4 ms | 13.7 ms | 24.4 ms | 147 M insn/s |
| tight loop, 1M iterations (`%`, `+`) | 32.9 ms | 48.3 ms | 48.0 ms | 183 M insn/s |
| sieve, 100k | 14.0 ms | 17.3 ms | 16.7 ms | 184 M insn/s |
| float loop, 500k | 26.3 ms | 37.5 ms | 38.9 ms | 152 M insn/s |
| closure calls, 500k | 33.0 ms | 40.3 ms | 65.9 ms | 167 M insn/s |
| structs + enums, 250k | 52.1 ms | 59.5 ms | 110.9 ms | 119 M insn/s |
| map insert + lookup, 100k | 27.8 ms | 38.6 ms | – | 47 M insn/s |
| string concat + split, 50k | 45.3 ms | 36.8 ms | – | 19 M insn/s |
| garbage-heavy, 150k iterations | 53.2 ms (66 GCs) | 55.5 ms | – | 51 M insn/s |
| same, 64 KiB heap limit | 48.9 ms (2133 GCs) | 49.9 ms | – | 55 M insn/s |

`compile/*` measures source → bytecode, including the prelude: about
1.8 MiB/s of source on this machine (400 generated functions in ~67 ms;
roughly 2 ms of fixed prelude cost for a tiny program). Parsing is ~3% of
the total; the IR optimizer more than doubles compile time for a modest
instruction-count reduction, which is visible in the `vm` vs `vm-noopt`
columns.

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
