#!/usr/bin/env bash
# refusals.sh ROOT: TB_SKZ_UPLOAD's refusals at ROOT's topology-bench.sh, as
# tests/lib/test_skz_store.sh runs the releaser's: made-up tools so that
# nothing past the checks can run, and "no-such-topology" after the daemon
# so that a row that gets past every check ends there (64, "unknown
# topology"). One line per case: name, exit status, last line said.
# Needs root, as the script does.
set -uo pipefail
ROOT=$1
T=$(mktemp -d); trap 'rm -rf -- "${T:?}"' EXIT
cat > "$T/current" <<E2
#!/bin/bash
if [ "\$1" = help ]; then echo "usage: skyzfsd serve --dir DIR [--run-dir DIR] [--queues 1]"; exit 0; fi
echo "\$*" >> "$T/served"; exit 2
E2
chmod +x "$T/current"
mkdir -p "$T/bin"
for t in zpool zfs tar ublk dmsetup losetup blockdev mkfs.ext4 pgbench modprobe; do
    printf '#!/bin/sh\necho "%s $*" >> "%s/served"\nexit 1\n' "$t" "$T" > "$T/bin/$t"; chmod +x "$T/bin/$t"
done
run() { # name, then VAR=value...
    local name=$1 rc=0; shift
    env -u TB_S3_ENDPOINT -u TB_S3_BUCKET -u TB_S3_KEY_ID -u TB_S3_SECRET -u TB_SKZ_UPLOAD -u TB_SKZ_RELEASE \
        -u TB_SKZ_UPLOAD_WAIT_S -u TB_SKYZFSD_ARGS -u TB_SKYZFSD_REF \
        PATH="$T/bin:$PATH" TB_SKYZFSD="$T/current" TB_OUT="$T/out" TB_WORK="$T/work" "$@" \
        timeout 60 bash "$ROOT/tests/bench/topology-bench.sh" > "$T/log" 2>&1 || rc=$?
    printf '%-52s rc=%-3s %s\n' "$name" "$rc" "$(tail -n 1 "$T/log" | cut -c1-120)"
}
S3=(TB_S3_ENDPOINT=http://127.0.0.1:9000 TB_S3_BUCKET=b TB_S3_KEY_ID=k TB_S3_SECRET=s)
D="zfs-skyzfsd no-such-topology"
run "A1 upload, no daemon (zfs-slog)"                TB_TOPOLOGIES="zfs-slog" TB_SKZ_UPLOAD=1 "${S3[@]}"
run "A2 upload, no daemon, no settings"              TB_TOPOLOGIES="zfs-slog" TB_SKZ_UPLOAD=1
run "A3 upload, thin topology (nocache)"             TB_TOPOLOGIES="nocache" TB_SKZ_UPLOAD=1 "${S3[@]}"
run "A4 upload, daemon named as zfs-skyzfsdx"        TB_TOPOLOGIES="zfs-slog zfs-skyzfsdx" TB_SKZ_UPLOAD=1 "${S3[@]}"
run "B0 upload + daemon, the four: past the checks"  TB_TOPOLOGIES="$D" TB_SKZ_UPLOAD=1 "${S3[@]}"
for v in TB_S3_ENDPOINT TB_S3_BUCKET TB_S3_KEY_ID TB_S3_SECRET; do
    a=(); for x in "${S3[@]}"; do [ "${x%%=*}" = "$v" ] || a+=("$x"); done
    run "B1 upload + daemon, without $v"   TB_TOPOLOGIES="$D" TB_SKZ_UPLOAD=1 "${a[@]}"
    run "B2 upload + daemon, $v empty"     TB_TOPOLOGIES="$D" TB_SKZ_UPLOAD=1 "${a[@]}" "$v="
done
run "C1 upload + releaser (hold-cuts, hooks)"        TB_TOPOLOGIES="$D" TB_SKZ_UPLOAD=1 "${S3[@]}" TB_SKZ_RELEASE=1 TB_SKYZFSD_REF=abc+hooks TB_SKYZFSD_ARGS=--hold-cuts
run "C2 upload + releaser, no hold-cuts"             TB_TOPOLOGIES="$D" TB_SKZ_UPLOAD=1 "${S3[@]}" TB_SKZ_RELEASE=1 TB_SKYZFSD_REF=abc+hooks
run "D1 TB_SKZ_UPLOAD=0"                             TB_TOPOLOGIES="$D" TB_SKZ_UPLOAD=0
run "D2 TB_SKZ_UPLOAD=yes"                           TB_TOPOLOGIES="$D" TB_SKZ_UPLOAD=yes "${S3[@]}"
run "D3 TB_SKZ_UPLOAD=0, no daemon"                  TB_TOPOLOGIES="zfs-slog no-such-topology" TB_SKZ_UPLOAD=0
run "E1 wait abc"                                    TB_TOPOLOGIES="$D" TB_SKZ_UPLOAD=1 "${S3[@]}" TB_SKZ_UPLOAD_WAIT_S=abc
run "E2 wait -5"                                     TB_TOPOLOGIES="$D" TB_SKZ_UPLOAD=1 "${S3[@]}" TB_SKZ_UPLOAD_WAIT_S=-5
run "E3 wait 0 (no wait): past the checks"           TB_TOPOLOGIES="$D" TB_SKZ_UPLOAD=1 "${S3[@]}" TB_SKZ_UPLOAD_WAIT_S=0
run "E4 wait 30 without upload: past the checks"     TB_TOPOLOGIES="$D" TB_SKZ_UPLOAD_WAIT_S=30
run "F1 upload, zfs-slog and zfs-skyzfsd in one row" TB_TOPOLOGIES="zfs-slog $D" TB_SKZ_UPLOAD=1 "${S3[@]}"
run "F2 no upload, the settings given"               TB_TOPOLOGIES="$D" "${S3[@]}"
run "F3 releaser alone (as before)"                  TB_TOPOLOGIES="$D" TB_SKZ_RELEASE=1 TB_SKYZFSD_REF=abc+hooks TB_SKYZFSD_ARGS=--hold-cuts
run "F4 upload + header sync args"                   TB_TOPOLOGIES="$D" TB_SKZ_UPLOAD=1 "${S3[@]}" TB_SKYZFSD_ARGS=--hold-cuts,--intent-header-sync
run "F5 upload + copy repeat"                        TB_TOPOLOGIES="$D" TB_SKZ_UPLOAD=1 "${S3[@]}" TB_SKYZFSD_COPY_REPEAT=2
run "F6 upload + hooks build, no releaser"           TB_TOPOLOGIES="$D" TB_SKZ_UPLOAD=1 "${S3[@]}" TB_SKYZFSD_REF=abc+hooks
echo "served: $([ -e "$T/served" ] && tr '\n' ';' < "$T/served" || echo nothing)"
