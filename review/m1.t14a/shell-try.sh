#!/usr/bin/env bash
# pool_step and the fill step of replay-rig.sh, run by themselves with the
# real lim and with zpool, zfs and dd replaced by stand-ins.
#
#   bash review/m1.t14a/shell-try.sh <checkout of the head>
#
# Each case runs the rig's own lines (taken out of the script with sed) in
# a shell of its own under `set -euo pipefail`, and prints its exit status
# and the first lines of the summary it wrote. 1 is a verdict against the
# daemon, 2 the rig's error. Nothing outside a temporary directory is
# touched; /dev/full stands in for a pool that is full.
set -u
repo=${1:?checkout}
rig=$repo/tests/kernel/replay-rig.sh
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
mkdir -p "$t/bin"
head() {  # the common start of every snippet
    echo 'set -euo pipefail'
    echo "HERE=$repo/tests/kernel; OUT=$t/out; WORK=$t/work; mkdir -p \$OUT \$WORK; : > \$OUT/x; PHASE=record; VERDICT=\"\""
    echo 'POOL=p; VOLUME_MB=1024; MOUNTPOINT=$WORK/mnt; mkdir -p "$MOUNTPOINT"; log() { :; }'
    echo ". $repo/tests/lib/lim.sh"
    sed -n '/^rig_error() {/,/^}/p; /^daemon_verdict() {/,/^}/p; /^pool_step() {/,/^}/p' "$rig"
}
try() {  # name, then the snippet's own lines on stdin
    local name=$1 rc=0 t0=$SECONDS
    rm -rf "$t/out" "$t/work"
    { head; cat; echo 'echo went-on'; } > "$t/snippet.sh"
    PATH="$t/bin:$PATH" bash "$t/snippet.sh" > "$t/stdout" 2> "$t/stderr" || rc=$?
    printf '%-46s exit %-3s %2ss  %s\n' "$name" "$rc" "$((SECONDS - t0))" \
        "$( { sed -n '1p;3p' "$t/out/replay-summary.md" 2>/dev/null | tr '\n' ' '; grep -h went-on "$t/stdout"; } | cut -c1-175)"
}
fill() { sed -n '/^if \[ -n "\$SKYZFSD" \] && \[ "\$FILL_MB" -gt 0 \]; then/,/^fi/p' "$rig" | sed 's/lim 600 /lim 2 /'; }

echo "== pool_step, through the daemon"
try "a command that succeeds"            <<< 'SKYZFSD=d; pool_step 5 "zpool sync 1" true'
try "a command that fails with 3"        <<< 'SKYZFSD=d; pool_step 5 "zpool sync 1" sh -c "exit 3"'
try "a command that does not end"        <<< 'SKYZFSD=d; pool_step 1 "zpool sync 1" sleep 30'
try "a command that cannot be started"   <<< 'SKYZFSD=d; pool_step 5 "zpool create" /nonexistent/zpool'
try "lim itself fails (no temporary file)" <<< 'SKYZFSD=d; export TMPDIR=/nonexistent; pool_step 5 "zpool sync 1" true'
echo "== pool_step, on the reference server"
try "a command that fails with 3"        <<< 'SKYZFSD=""; pool_step 5 "zpool sync 1" sh -c "exit 3"'
try "a command that takes 2 s, limit 1"  <<< 'SKYZFSD=""; pool_step 1 "zpool sync 1" sleep 2'

echo "== the fill step (its limit shortened from 600 s to 2 s)"
zfs_says() { printf '#!/bin/sh\n%s\n' "$1" > "$t/bin/zfs"; chmod +x "$t/bin/zfs"; }
dd_does() { printf '#!/bin/sh\n%s\n' "$1" > "$t/bin/fakedd"; chmod +x "$t/bin/fakedd"; }
zfs_says 'echo 800000000'; dd_does 'exit 0'
try "it fits and is written"             < <(echo 'SKYZFSD=d; FILL_MB=160; DD=fakedd'; fill)
zfs_says 'echo 100000000'
try "it does not fit (95 MiB available)" < <(echo 'SKYZFSD=d; FILL_MB=160; DD=fakedd'; fill)
zfs_says 'echo -'
try "zfs get gives no number"            < <(echo 'SKYZFSD=d; FILL_MB=160; DD=fakedd'; fill)
zfs_says 'echo "cannot open p" >&2; exit 1'
try "zfs get fails"                      < <(echo 'SKYZFSD=d; FILL_MB=160; DD=fakedd'; fill)
zfs_says 'echo 800000000'
try "dd runs out of space"               < <(echo 'SKYZFSD=d; FILL_MB=1; DD=dd; ln -s /dev/full "$MOUNTPOINT/fill"'; fill)
dd_does 'echo "dd: error writing: Input/output error" >&2; exit 1'
try "dd fails with an I/O error"         < <(echo 'SKYZFSD=d; FILL_MB=160; DD=fakedd'; fill)
dd_does 'sleep 30'
try "dd does not end"                    < <(echo 'SKYZFSD=d; FILL_MB=160; DD=fakedd'; fill)
dd_does 'exit 0'
try "no daemon: the step is skipped"     < <(echo 'SKYZFSD=""; FILL_MB=160; DD=fakedd'; fill)
