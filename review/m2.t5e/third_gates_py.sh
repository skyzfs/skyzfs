#!/usr/bin/env bash
# Third look at #145: every rig unit test at the worktree given (each
# tests/kernel/test_*.py, tests/bench/test_*.py, tests/kernel/test_*.sh,
# tests/bench/test_*.sh and tests/lib/test_*.sh), one line each: exit
# code, file, its last words; then shellcheck over git ls-files '*.sh'
# with the shellcheck given.
# Usage: bash third_gates_py.sh WORKTREE SHELLCHECK
set -u
WT=$1 SC=$2
cd "$WT" || exit 2
echo "# Rig unit tests at $(git rev-parse --short HEAD): exit code, file, its last words; then $("$SC" --version | sed -n 2p | sed 's/version: /shellcheck /') over git ls-files '*.sh'."
for f in tests/kernel/test_*.py tests/bench/test_*.py; do
    out=$(python3 "$f" 2>&1); rc=$?
    echo "$rc $f $(grep -E '^(Ran |OK|FAILED)' <<< "$out" | tr '\n' ' ')"
done
for f in tests/kernel/test_*.sh tests/bench/test_*.sh tests/lib/test_*.sh; do
    out=$(bash "$f" 2>&1); rc=$?
    echo "$rc $f $(tail -n 2 <<< "$out" | tr '\n' ' ' | cut -c1-160)"
    [ "$rc" = 0 ] || grep -E 'FAIL' <<< "$out" | head -5 | sed 's/^/    /'
done
git ls-files '*.sh' | xargs -r "$SC" > /dev/null 2>&1
echo "shellcheck rc=$?"
