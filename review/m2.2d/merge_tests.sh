#!/usr/bin/env bash
# #107 at e25bf3a (the merge with zfs-topology at c1bb85c, #106 in): the tests of
# skyzfs-local and skyzfsd, plain and with their test hooks.
#
#     merge_tests.sh CHECKOUT TARGET_DIR
cd "$1" || exit 2
export CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=$2
for features in "" "--features test-hooks"; do
    for pkg in skyzfs-local skyzfsd; do
        # shellcheck disable=SC2086
        out=$(cargo test -p "$pkg" $features --no-fail-fast 2>&1)
        echo "$pkg ${features:-plain}: exit $?; $(grep -c '^test .* ok$' <<< "$out") ok, $(grep -c '^test .* FAILED$' <<< "$out") failed, $(grep -c '^test .* ignored' <<< "$out") ignored"
        grep '^test .* FAILED$' <<< "$out" | head -5
        grep '^error' <<< "$out" | head -3
    done
done
echo finished
