# Neon

A statically typed language with a register bytecode VM, written in pure Rust
(edition 2021, **standard library only** — no third-party crates).

Status: **Phase 4 of 10** — front end complete: VFS, diagnostics, lexer, parser, semantic analysis.

## Design constraints

* **Leak-free, UB-free** VM and runtime; every test binary must pass Valgrind
  (see below).
* **LSP-ready**: all positions are `Span`s resolved by one `SourceMap`; the
  parser (Phase 3) recovers from errors so partial ASTs survive.
* **WASM-ready**: the engine builds for `wasm32-unknown-unknown`. It never
  touches `std::fs` or stdin/stdout directly — files go through the `Vfs`
  trait (`OsVfs` for the CLI, `MemVfs` in memory), VM I/O through `VmIo`.

## Layout

```
src/
  vfs/    Vfs trait, OsVfs (non-wasm only), MemVfs, lexical path helpers
  diag/   FileId/Span, SourceMap (offset -> line/col), Diagnostic, DiagBag, ASCII Emitter
  syntax/ Interner/Symbol, LitPool, tokens, streaming Lexer, flat Ast, resilient Parser, S-expr printer
  sema/   module DAG loader, collection, type inference/checking, traits, exhaustiveness, def-use table, prelude.neon
  builtin.rs  intrinsic operations shared by sema, IR and VM
  main.rs CLI entry point
tests/    integration tests
scripts/  valgrind_test.sh, valgrind.supp
```

Later phases add `ir/`, `bytecode/`, `compiler/`, `gc/`, `vm/`.

## Checks

```sh
cargo test
cargo build --lib --target wasm32-unknown-unknown   # rustup target add wasm32-unknown-unknown
scripts/valgrind_test.sh
```

### Valgrind gate

`scripts/valgrind_test.sh` runs every test binary under
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
