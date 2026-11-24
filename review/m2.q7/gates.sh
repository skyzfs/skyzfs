#!/bin/bash
cd "$1"
export CARGO_TARGET_DIR="$2/target"
export CARGO_INCREMENTAL=0
O="$2"
{
echo "== rustc: $(rustc --version)"
echo "== fmt"; cargo fmt --all --check; echo "exit $?"
echo "== clippy workspace"; cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5; echo "exit ${PIPESTATUS[0]}"
echo "== clippy skyzfsd test-hooks"; cargo clippy -p skyzfsd --features test-hooks --all-targets -- -D warnings 2>&1 | tail -5; echo "exit ${PIPESTATUS[0]}"
echo "== clippy ublk-sys control-test-program"; cargo clippy -p skyzfs-ublk-sys --features control-test-program --all-targets -- -D warnings 2>&1 | tail -5; echo "exit ${PIPESTATUS[0]}"
} > "$O/gates.out" 2>&1
cargo test -p skynbd-refs -p skyzfs-upload -p skyzfsd -p skyzfs-ublk > "$O/t_plain.out" 2>&1; echo "exit $?" >> "$O/t_plain.out"
cargo test -p skynbd-refs -p skyzfs-upload -p skyzfsd -p skyzfs-ublk --features skyzfsd/test-hooks,skyzfs-local/test-hooks > "$O/t_hooks.out" 2>&1; echo "exit $?" >> "$O/t_hooks.out"
cargo test -p skyzfs-local > "$O/t_local.out" 2>&1; echo "exit $?" >> "$O/t_local.out"
cargo test -p skyzfs-ublk-sys --features control-test-program > "$O/t_ublksys.out" 2>&1; echo "exit $?" >> "$O/t_ublksys.out"
echo done > "$O/gates.done"
