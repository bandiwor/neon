#!/usr/bin/env bash
# Runs every test binary (unit + integration) under Valgrind and fails unless
# each one exits 0 with: 0 errors from 0 contexts, definitely lost: 0 bytes,
# indirectly lost: 0 bytes (and no possibly/still-reachable leaks either).
#
# Usage: scripts/valgrind_test.sh [extra args passed to every test binary]
set -euo pipefail

cd "$(dirname "$0")/.."

# Backtrace symbolization (triggered by `should_panic` tests) builds process-wide
# caches that are never freed; they are std's, not ours, so keep them out of the run.
export RUST_BACKTRACE=0

if ! command -v valgrind >/dev/null 2>&1; then
    echo "error: valgrind is not installed" >&2
    exit 2
fi

VG_FLAGS=(
    --suppressions="$PWD/scripts/valgrind.supp"
    --leak-check=full
    --show-leak-kinds=all
    --errors-for-leak-kinds=all
    --error-exitcode=1
)

# Build the test binaries without running them and collect their paths.
mapfile -t BINS < <(
    cargo test --no-run --message-format=json 2>/dev/null |
        sed -n 's/.*"executable":"\([^"]*\)".*/\1/p' |
        sort -u |
        grep -v '/neon$' || true # the plain CLI binary is exercised separately below
)

if [ "${#BINS[@]}" -eq 0 ]; then
    echo "error: no test binaries found" >&2
    exit 2
fi

LOG="$(mktemp)"
trap 'rm -f "$LOG"' EXIT
failed=0

for bin in "${BINS[@]}"; do
    name="$(basename "$bin")"
    echo "==> valgrind $name"
    # --test-threads=1 keeps Valgrind's thread serialization predictable.
    if ! valgrind "${VG_FLAGS[@]}" "$bin" --test-threads=1 "$@" >"$LOG" 2>&1; then
        echo "FAIL: $name exited non-zero"
        tail -n 40 "$LOG"
        failed=1
        continue
    fi
    for pattern in \
        "0 errors from 0 contexts" \
        "definitely lost: 0 bytes" \
        "indirectly lost: 0 bytes"; do
        if ! grep -q "$pattern" "$LOG"; then
            echo "FAIL: $name: missing '$pattern'"
            tail -n 40 "$LOG"
            failed=1
            continue 2
        fi
    done
    echo "ok: $name"
done

# The command-line binary itself, on every corpus program, with the collector
# running at every safe-point (this exercises stack maps, upvalues and the
# heap under Valgrind's memory checking).
cargo build --quiet --bin neon
CLI="$PWD/target/debug/neon"
OUT="$(mktemp)"
trap 'rm -f "$LOG" "$OUT"' EXIT
programs=0
for prog in tests/corpus/*.neon; do
    expected="${prog%.neon}.out"
    programs=$((programs + 1))
    if ! valgrind "${VG_FLAGS[@]}" --log-file="$LOG" "$CLI" run --gc-stress "$prog" >"$OUT" 2>/dev/null; then
        echo "FAIL: neon run $prog exited non-zero under valgrind"
        tail -n 40 "$LOG"
        failed=1
        continue
    fi
    if ! cmp -s "$OUT" "$expected"; then
        echo "FAIL: neon run $prog printed the wrong output under valgrind"
        failed=1
        continue
    fi
    for pattern in \
        "0 errors from 0 contexts" \
        "definitely lost: 0 bytes" \
        "indirectly lost: 0 bytes"; do
        if ! grep -q "$pattern" "$LOG"; then
            echo "FAIL: neon run $prog: missing '$pattern'"
            tail -n 40 "$LOG"
            failed=1
            continue 2
        fi
    done
done
echo "ran $programs corpus programs through the CLI under valgrind"

if [ "$failed" -ne 0 ]; then
    echo "valgrind validation FAILED" >&2
    exit 1
fi
echo "valgrind validation passed for ${#BINS[@]} test binaries and $programs CLI runs"
