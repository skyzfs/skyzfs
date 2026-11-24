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
# TB_SKZ_RELEASE (M2.T4): a releaser with no held cuts to release, or a
# build with no commands about them, ends the row with 64 before
# anything runs; with both, the row gets past the check (and ends on the
# unknown topology after it).
bench_release() { # REF ARGS -> "rc|log"
    local rc=0
    PATH="$T/bin:$PATH" TB_TOPOLOGIES="zfs-skyzfsd no-such-topology" TB_SKYZFSD=$T/current TB_SKYZFSD_REF=$1 TB_SKYZFSD_ARGS=$2 \
        TB_SKZ_RELEASE=1 TB_OUT=$T/bench-out TB_WORK=$T/bench-work bash "$ROOT/tests/bench/topology-bench.sh" > "$T/bench.log" 2>&1 || rc=$?
    echo "$rc|$(cat "$T/bench.log")"
}
r=$(bench_release abc+hooks "--queues,1")
is "TB_SKZ_RELEASE without --hold-cuts: the row ends with 64" "${r%%|*}" 64
has "  and says why" "$r" "TB_SKZ_RELEASE needs --hold-cuts in TB_SKYZFSD_ARGS ('--queues,1')"
r=$(bench_release abc "--hold-cuts")
is "TB_SKZ_RELEASE with a build without the test hooks: 64" "${r%%|*}" 64
has "  and says why" "$r" "TB_SKZ_RELEASE needs a build with the test hooks, TB_SKYZFSD_REF=<ref>+hooks, not 'abc'"
r=$(bench_release "" "--hold-cuts")
has "  a build that says nothing of its ref is refused too" "$r" "not '': a release build"
r=$(bench_release abc+hooks "--queues,1,--hold-cuts")
has "TB_SKZ_RELEASE with both: past the check" "$r" "unknown topology: no-such-topology"
has "  --hold-cuts may come anywhere, and with spaces" "$(bench_release abc+hooks "--hold-cuts --queues 1")" "unknown topology: no-such-topology"
is "  --hold-cutsx is not --hold-cuts" "$(bench_release abc+hooks "--hold-cutsx" | head -c 2)" 64
has "    and it is refused for it" "$(bench_release abc+hooks "--hold-cutsx")" "needs --hold-cuts"
# TB_SKZ_UPLOAD's refusals (the review of #123, F3): each before
# anything runs, with exit 64 and the reason. The full settings with a
# topology the script does not know get past them all.
bench_upload() { # TOPOLOGIES then NAME=VALUE... -> "rc|log"
    local rc=0 topo=$1; shift
    env PATH="$T/bin:$PATH" TB_TOPOLOGIES="$topo" TB_SKYZFSD="$T/current" TB_SKYZFSD_ARGS=--hold-cuts \
        TB_OUT="$T/bench-out" TB_WORK="$T/bench-work" TB_SKZ_UPLOAD=1 TB_S3_ENDPOINT=http://127.0.0.1:9000 \
        TB_S3_BUCKET=b TB_S3_KEY_ID=k TB_S3_SECRET=s "$@" bash "$ROOT/tests/bench/topology-bench.sh" > "$T/bench.log" 2>&1 || rc=$?
    echo "$rc|$(cat "$T/bench.log")"
}
r=$(bench_upload "zfs-skyzfsd no-such-topology")
has "uploads with every setting: past the checks" "$r" "unknown topology: no-such-topology"
r=$(bench_upload zfs-slog)
is "uploads in a row with no daemon: 64" "${r%%|*}" 64
has "  and says why" "$r" "TB_SKZ_UPLOAD=1 is for zfs-skyzfsd only, not: zfs-slog"
r=$(bench_upload "zfs-slog zfs-skyzfsd")
is "uploads in a row with the daemon and another: 64" "${r%%|*}" 64
has "  and says why" "$r" "TB_SKZ_UPLOAD=1 is for zfs-skyzfsd only, not: zfs-slog zfs-skyzfsd"
for v in TB_S3_ENDPOINT TB_S3_BUCKET TB_S3_KEY_ID TB_S3_SECRET; do
    r=$(bench_upload zfs-skyzfsd "$v=")
    is "uploads without $v: 64" "${r%%|*}" 64
    has "  and says which" "$r" "TB_SKZ_UPLOAD=1 needs $v"
done
r=$(bench_upload zfs-skyzfsd TB_SKZ_RELEASE=1 TB_SKYZFSD_REF=abc+hooks)
is "uploads with the releaser: 64" "${r%%|*}" 64
has "  and says why" "$r" "TB_SKZ_UPLOAD=1 and TB_SKZ_RELEASE"
r=$(bench_upload zfs-skyzfsd TB_SKYZFSD_COPY_REPEAT=2)
is "uploads with copies repeated: 64" "${r%%|*}" 64
has "  and says why" "$r" "TB_SKZ_UPLOAD=1 and TB_SKYZFSD_COPY_REPEAT"
r=$(bench_upload zfs-skyzfsd TB_SKZ_UPLOAD=yes)
is "TB_SKZ_UPLOAD=yes: 64" "${r%%|*}" 64
has "  and says why" "$r" "TB_SKZ_UPLOAD is 1 or unset, not: yes"
r=$(bench_upload zfs-slog TB_SKZ_UPLOAD=0)
is "TB_SKZ_UPLOAD=0 is not an upload, and is refused as not 1" "${r%%|*}" 64
has "  and says why" "$r" "TB_SKZ_UPLOAD is 1 or unset, not: 0"
r=$(bench_upload zfs-skyzfsd TB_SKZ_UPLOAD_WAIT_S=abc)
is "TB_SKZ_UPLOAD_WAIT_S=abc: 64" "${r%%|*}" 64
has "  and says why" "$r" "TB_SKZ_UPLOAD_WAIT_S is a number of seconds, not: abc"
# TB_SKZ_UPLOAD_FROM_LOAD: MinIO frozen until the workload begins.
r=$(bench_upload "zfs-skyzfsd no-such-topology" TB_SKZ_UPLOAD_WAIT_S=0 TB_SKZ_UPLOAD_FROM_LOAD=1)
has "the seed from the load, with uploads and no wait: past the checks" "$r" "unknown topology: no-such-topology"
r=$(bench_upload zfs-skyzfsd TB_SKZ_UPLOAD_WAIT_S=0 TB_SKZ_UPLOAD_FROM_LOAD=yes)
is "TB_SKZ_UPLOAD_FROM_LOAD=yes: 64" "${r%%|*}" 64
has "  and says why" "$r" "TB_SKZ_UPLOAD_FROM_LOAD is 1, window or unset, not: yes"
# =window: MinIO frozen until the first window begins (the seed's cost).
r=$(bench_upload "zfs-skyzfsd no-such-topology" TB_SKZ_UPLOAD_WAIT_S=0 TB_SKZ_UPLOAD_FROM_LOAD=window)
has "the seed from the first window, with uploads and no wait: past the checks" "$r" "unknown topology: no-such-topology"
r=$(bench_upload zfs-skyzfsd TB_SKZ_UPLOAD= TB_SKZ_UPLOAD_WAIT_S=0 TB_SKZ_UPLOAD_FROM_LOAD=window)
is "the seed from the first window without uploads: 64" "${r%%|*}" 64
has "  and says why" "$r" "TB_SKZ_UPLOAD_FROM_LOAD=window needs TB_SKZ_UPLOAD=1"
r=$(bench_upload zfs-skyzfsd TB_SKZ_UPLOAD_FROM_LOAD=window)
is "the seed from the first window with the wait's default (900): 64" "${r%%|*}" 64
has "  and says why: the wait could not end" "$r" "TB_SKZ_UPLOAD_FROM_LOAD=window needs TB_SKZ_UPLOAD_WAIT_S=0, not 900"
r=$(bench_upload zfs-skyzfsd TB_SKZ_UPLOAD= TB_SKZ_UPLOAD_WAIT_S=0 TB_SKZ_UPLOAD_FROM_LOAD=1)
is "the seed from the load without uploads: 64" "${r%%|*}" 64
has "  and says why" "$r" "TB_SKZ_UPLOAD_FROM_LOAD=1 needs TB_SKZ_UPLOAD=1"
r=$(bench_upload zfs-skyzfsd TB_SKZ_UPLOAD_FROM_LOAD=1)
is "the seed from the load with the wait's default (900): 64" "${r%%|*}" 64
has "  and says why: the wait could not end" "$r" "TB_SKZ_UPLOAD_FROM_LOAD=1 needs TB_SKZ_UPLOAD_WAIT_S=0, not 900"
# The check after the loop, alone: the runs above end in the loop before
# it for a row the loop lets through (review of #123, R9).
eval "$(sed -n '/^skz_upload_topologies() {/,/^}$/p' "$ROOT/tests/bench/topology-bench.sh")"
up_topos() { local rc=0; skz_upload_topologies "$@" 2> /dev/null || rc=$?; echo "$rc"; }
is "uploads with the daemon alone: let through" "$(up_topos 1 zfs-skyzfsd)" 0
is "  with blanks around it too" "$(up_topos 1 "  zfs-skyzfsd ")" 0
is "  the daemon twice: refused" "$(up_topos 1 "zfs-skyzfsd zfs-skyzfsd")" 1
is "  beside the model: refused" "$(up_topos 1 "zfs-skyzfsd zfs-slog")" 1
is "  no topology: refused" "$(up_topos 1 "")" 1
is "no uploads, any topologies: let through" "$(up_topos "" "zfs-slog zfs-skyzfsd")" 0
# shellcheck disable=SC2016  # the script's own text
check_call=$(grep -c '^skz_upload_topologies "\$SKZ_UPLOAD" "\$TOPOLOGIES" || exit 64$' "$ROOT/tests/bench/topology-bench.sh")
is "  and the script calls it, with exit 64" "$check_call" 1
# minio.sh deletes its directory as root: only one named minio-data
# (G8), refused before docker or sudo is called.
mkdir -p "$T/keep"
rc=0; out=$(bash "$ROOT/tests/bench/minio.sh" down "$T/keep" 2>&1) || rc=$?
is "minio.sh down on a directory not named minio-data: 64" "$rc" 64
has "  and says why" "$out" "deletes only a directory named minio-data, not: $T/keep"
is "  and the directory is still there" "$(test -d "$T/keep" && echo there)" there
rc=0; bash "$ROOT/tests/bench/minio.sh" down "$T/keep/" > /dev/null 2>&1 || rc=$?
is "  a trailing slash is not a way round it" "$rc" 64
# The releaser's line in each snapshot (the review of #108): the count
# of its releases, whether it runs, its last line.
eval "$(sed -n '/^skz_release_line() {/,/^}$/p' "$ROOT/tests/bench/topology-bench.sh")"
is "skzrel with no file and no releaser" "$(skz_release_line "$T/none.tsv" "")" "skzrel 0 0 "
printf '# released every 4\n123\t1\t7\n124\t2\t8\n# 125 cut 3 gone after its release was refused (busy)\n' > "$T/rel.tsv"
sleep 30 & live=$!
is "skzrel counts the releases and says it runs" "$(skz_release_line "$T/rel.tsv" "$live")" \
    "skzrel 2 1 # 125 cut 3 gone after its release was refused (busy)"
kill "$live"; wait "$live" 2>/dev/null || true
is "  and that it ended" "$(skz_release_line "$T/rel.tsv" "$live" | cut -d' ' -f1-3)" "skzrel 2 0"
printf '1\t%s\n' "$(head -c 400 /dev/zero | tr '\0' x)" > "$T/long.tsv"
# "skzrel 1 0 ", 300 characters and the newline
is "  the last line is cut at 300 characters" "$(skz_release_line "$T/long.tsv" "" | wc -c)" 312
# its blanks squeezed, none left at its ends (the review of #111, B4)
printf '1\t2\n\t# a  line\t with blanks \t\n' > "$T/blanks.tsv"
is "  the last line's blanks squeezed and its ends bare" "$(skz_release_line "$T/blanks.tsv" "")" \
    "skzrel 1 0 # a line with blanks"
# and the snapshot of a row with the releaser writes it (B5)
snap=$(sed -n '/^snapshot() {/,/^}$/p' "$ROOT/tests/bench/topology-bench.sh")
# shellcheck disable=SC2016  # the script's own text
has "the snapshot writes the releaser's line" "$snap" 'skz_release_line "$OUT/skz-releases.tsv" "$SKZ_RELEASER"'
# shellcheck disable=SC2016  # the script's own text
has "  where the daemon serves and the releaser was asked for" "$snap" 'if [ -n "$SKZ_UP" ] && [ -n "$SKZ_RELEASE" ]; then'
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
