#!/usr/bin/env bash
# Third look at #145: the cargo gates at the worktree given (58fc660), as
# CLAUDE.md asks: fmt; clippy -D warnings plain, skyzfsd with test-hooks,
# skyzfs-ublk-sys with control-test-program; cargo test the same three
# ways. No debuginfo and no incremental cache (the disk is small); a
# watchdog stops the build below 1.5 GB free. Paths are printed as WT.
# Usage: bash third_gates_cargo.sh WORKTREE TARGET_DIR
set -u
WT=$1
export CARGO_TARGET_DIR=$2 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
cd "$WT" || exit 2
done_flag=$(mktemp)
rm -f "$done_flag"
(
    while sleep 5; do
        a=$(df --output=avail -k / | tail -1)
        if [ "$a" -lt 1500000 ]; then
            pkill -f "cargo (test|clippy)"; pkill rustc
            echo "WATCHDOG: stopped at ${a}k free"
            break
        fi
        [ -e "$done_flag" ] && break
    done
) &
w=$!
s() { sed "s#$WT#WT#g"; }
echo "# $(git rev-parse --short HEAD); $(rustc --version)"
cargo fmt --all -- --check 2>&1 | s; echo "fmt rc=${PIPESTATUS[0]}"
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | s | grep -E '^(warning|error)|Finished' ; echo "clippy plain rc=${PIPESTATUS[0]}"
cargo clippy -p skyzfsd --all-targets --features test-hooks -- -D warnings 2>&1 | s | grep -E '^(warning|error)|Finished'; echo "clippy test-hooks rc=${PIPESTATUS[0]}"
cargo clippy -p skyzfs-ublk-sys --all-targets --features control-test-program -- -D warnings 2>&1 | s | grep -E '^(warning|error)|Finished'; echo "clippy control-test-program rc=${PIPESTATUS[0]}"
for v in plain hooks ctp; do
    case $v in
        plain) args=(--workspace) ;;
        hooks) args=(-p skyzfsd --features test-hooks) ;;
        ctp) args=(-p skyzfs-ublk-sys --features control-test-program) ;;
    esac
    out=$(cargo test "${args[@]}" 2>&1); rc=$?
    echo "test $v rc=$rc: $(grep -E '^test result:' <<< "$out" | awk '{p+=$4; f+=$6; i+=$8; n++} END {print "passed " p " failed " f " ignored " i " in " n " test binaries"}')"
    grep -E '^(failures:|---- |error)' <<< "$out" | s | head -20
done
touch "$done_flag"
wait "$w"
rm -f "$done_flag"
df -h / | tail -1
