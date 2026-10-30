#!/usr/bin/env bash
# Compares compile time (and the produced bytecode) of the working tree with another revision.
#
# Usage: scripts/compare_compile.sh <git-revision> [threads]
#
# Builds `examples/compile_bench.rs` in a temporary worktree of <revision> and in this tree, runs
# both, and prints the timings side by side. Equal `bytecode` fingerprints mean the two compilers
# produced exactly the same code. `threads` (default: all cores) is used for this tree; revisions
# before the parallel compiler have no such setting.
set -euo pipefail
cd "$(dirname "$0")/.."
rev="${1:?usage: scripts/compare_compile.sh <git-revision> [threads]}"
threads="${2:-0}"
work="$(mktemp -d)"
trap 'git worktree remove --force "$work/old" >/dev/null 2>&1 || true; rm -rf "$work"' EXIT

git worktree add -f "$work/old" "$rev" >/dev/null
mkdir -p "$work/old/examples"
# the benchmark minus the thread setting, which older revisions do not have
sed 's/^    if let Some(t) = std::env::var("NEON_THREADS").*$//' examples/compile_bench.rs > "$work/old/examples/compile_bench.rs"

CARGO_TARGET_DIR="$work/target-old" cargo build --release --quiet --manifest-path "$work/old/Cargo.toml" --example compile_bench
cargo build --release --quiet --example compile_bench

new_bin="${CARGO_TARGET_DIR:-target}/release/examples/compile_bench"
echo "== $rev"
"$work/target-old/release/examples/compile_bench" | tee "$work/old.txt"
echo "== working tree ($([ "$threads" = 1 ] && echo 1 thread || echo "${threads/#0/all} threads"))"
NEON_THREADS="$threads" "$new_bin" | tee "$work/new.txt"
echo "== speed-up and output"
paste -d' ' <(awk '{print $(NF-7)}' "$work/old.txt") <(awk '{print $(NF-7)}' "$work/new.txt") \
           <(awk '{print $(NF-4)}' "$work/old.txt") <(awk '{print $(NF-4)}' "$work/new.txt") \
  | awk '{ printf "%8.1fx   %s\n", $1 / $2, ($3 == $4 ? "same bytecode" : "BYTECODE DIFFERS") }'
