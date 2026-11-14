#!/bin/bash
# Tests of tests/lib/skz-store.sh (M1.T34), and of its three callers:
# a --store that the build does not take ends the replay rig and the
# hole rig with their own error (exit 2) and a row of the benchmark
# with 64, before anything is served. No kernel is needed.
# Run: sudo bash tests/lib/test_skz_store.sh (the rigs need root to get
# as far as the check; the function's own cases do not)
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
# shellcheck source=tests/lib/skz-store.sh
. "$HERE/skz-store.sh"
if ! T=$(mktemp -d) || [ -z "$T" ] || [ ! -d "$T" ]; then echo "no temporary directory"; exit 2; fi
trap 'rm -rf "${T:?}"' EXIT
fails=0
is() { # name got want
    if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got '$2', want '$3'"; fails=$((fails + 1)); fi
}
has() { # name got part
    case $2 in *"$3"*) echo "ok   $1" ;; *) echo "FAIL $1: '$3' not in '$2'"; fails=$((fails + 1)) ;; esac
}

# Made-up builds: a current one (no choice of store), one from before
# M1.3t-d (its usage offers [--store), and one whose help fails. Any
# other command leaves a mark: nothing may be served.
fake() { # path, help text, help's exit status
    cat > "$1" <<EOF
#!/bin/bash
if [ "\$1" = help ]; then printf '%s\n' "$2"; exit $3; fi
echo "\$*" >> "$T/served"
exit 2
EOF
    chmod +x "$1"
}
fake "$T/current" "usage: skyzfsd serve --dir DIR [--run-dir DIR] [--queues 1]" 0
fake "$T/older" "usage: skyzfsd serve --dir DIR [--run-dir DIR] [--store in-place|log] [--log-high N]" 0
fake "$T/broken" "" 1

check() { # build options... -> "rc ADD"
    local rc=0
    skz_store_args "$@" || rc=$?
    echo "$rc $SKZ_STORE_ADD"
}

is "a current build, no options" "$(check "$T/current")" "0 0"
is "a current build, other options" "$(check "$T/current" --queues 1 --request-log=x)" "0 0"
check "$T/current" --store in-place > /dev/null || true
is "a current build, --store in-place" "$(check "$T/current" --store in-place)" "1 0"
skz_store_args "$T/current" --store in-place || true
has "  says why" "$SKZ_STORE_WHY" "this build of skyzfsd ($T/current) has no option --store"
has "  and what it would have done" "$SKZ_STORE_WHY" "its first serve would end with a usage error"
is "a current build, --store=in-place" "$(check "$T/current" --queues 1 --store=in-place)" "1 0"
is "a build whose help fails, --store in-place" "$(check "$T/broken" --store in-place)" "1 0"
is "a build from before M1.3t-d, no options: --store in-place is added" "$(check "$T/older")" "0 1"
is "  and with other options" "$(check "$T/older" --queues 1)" "0 1"
is "a build from before M1.3t-d, --store in-place" "$(check "$T/older" --store in-place)" "0 0"
is "  --store=in-place" "$(check "$T/older" --store=in-place)" "0 0"
is "  --store log" "$(check "$T/older" --store log)" "1 0"
skz_store_args "$T/older" --store log || true
is "  says why" "$SKZ_STORE_WHY" "--store log is of the log store, which M1.3t removed"
is "  --store with no value" "$(check "$T/older" --store)" "1 0"
is "  --store=log" "$(check "$T/older" --store=log)" "1 0"
for o in --log-high --log-low --writeback-kib --home-writeback-kib --readers --step-pause-ms --step-pause-until --step-pause-at --refuse-holes; do
    is "a log store option, $o N" "$(check "$T/older" "$o" 3)" "1 0"
    is "  $o=N, on a current build" "$(check "$T/current" "$o=3")" "1 0"
done
skz_store_args "$T/current" --log-high=3 || true
is "  says why" "$SKZ_STORE_WHY" "--log-high=3 is of the log store, which M1.3t removed"
is "an option that begins like one" "$(check "$T/current" --storex 1 --readers-x 2)" "0 0"
is "a --store with no value, after the other options, is refused" "$(check "$T/older" --store in-place --queues --store)" "1 0"
is "nothing was served by the checks" "$([ -e "$T/served" ] && cat "$T/served" || echo nothing)" nothing

# The callers. Each is run as far as the check, with a current build and
# --store in-place, and must stop there with its own exit status. The
# tools each needs are made up, and none of them may run: past the
# check, a missing kernel target (made-up dmsetup and modprobe) or the
# unknown topology after zfs-skyzfsd ends a caller before it sets up a
# machine, should the check ever be gone.
mkdir -p "$T/bin"
for t in zpool zfs tar ublk dmsetup losetup blockdev mkfs.ext4 pgbench modprobe; do printf '#!/bin/sh\necho "%s $*" >> "%s/served"\nexit 1\n' "$t" "$T" > "$T/bin/$t"; chmod +x "$T/bin/$t"; done
OUTD=$T/replay-out; mkdir -p "$OUTD"
rc=0
PATH="$T/bin:$PATH" RIG_SKYZFSD=$T/current RIG_SERVE_ARGS="--store in-place" bash "$ROOT/tests/kernel/replay-rig.sh" "$OUTD" "$T/replay-work" > "$T/replay.log" 2>&1 || rc=$?
is "the replay rig: the rig's own error" "$rc" 2
has "  its summary says why" "$(cat "$OUTD/replay-summary.md" 2>/dev/null)" "the rig's own error, not a verdict"
has "  and names the option" "$(cat "$OUTD/replay-summary.md" 2>/dev/null)" "RIG_SERVE_ARGS (--store in-place): --store in-place: this build of skyzfsd"
OUTD=$T/hole-out; mkdir -p "$OUTD"
rc=0
PATH="$T/bin:$PATH" RIG_SKYZFSD=$T/current RIG_SERVE_ARGS="--store=in-place" bash "$ROOT/tests/kernel/hole-rig.sh" "$OUTD" "$T/hole-work" > "$T/hole.log" 2>&1 || rc=$?
is "the hole rig: the rig's own error" "$rc" 2
has "  its summary says why" "$(cat "$OUTD/hole-summary.md" 2>/dev/null)" "RIG_SERVE_ARGS (--store=in-place): --store in-place: this build of skyzfsd"
rc=0
PATH="$T/bin:$PATH" TB_TOPOLOGIES="zfs-skyzfsd no-such-topology" TB_SKYZFSD=$T/current TB_SKYZFSD_ARGS="--store,in-place" TB_OUT=$T/bench-out TB_WORK=$T/bench-work \
    bash "$ROOT/tests/bench/topology-bench.sh" > "$T/bench.log" 2>&1 || rc=$?
is "the benchmark: the row ends with 64" "$rc" 64
has "  and says why" "$(cat "$T/bench.log")" "TB_SKYZFSD_ARGS (--store,in-place): --store in-place: this build of skyzfsd"
# skz_start, which serves each row, uses the same check and adds
# --store in-place for a build from before M1.3t-d.
start=$(sed -n '/^skz_start() {/,/^}$/p' "$ROOT/tests/bench/topology-bench.sh")
# shellcheck disable=SC2016  # the script's own text
has "skz_start checks the options" "$start" 'if ! skz_store_args "$SKYZFSD" "${args[@]}"; then'
# shellcheck disable=SC2016  # the script's own text
has "  and adds --store in-place when the build has the choice" "$start" '[ "$SKZ_STORE_ADD" = 0 ] || args=(--store in-place "${args[@]}")'
# A build from before M1.3t-d is not run through the callers: past the
# check they go on to set up pools and devices as root. The function's
# own cases above hold what they are given.
is "no caller ran anything" "$(cat "$T/served" 2>/dev/null || echo nothing)" nothing

if [ "$fails" = 0 ]; then echo "all passed"; else echo "$fails failed"; exit 1; fi
