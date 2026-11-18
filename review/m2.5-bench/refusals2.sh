#!/usr/bin/env bash
# refusals2.sh ROOT: the check after the loop of TB_SKZ_UPLOAD's refusals (an
# upload row with zfs-skyzfsd as its only topology), reached by rows whose
# topologies are all known. Made-up tools as in refusals.sh, so that a row that
# gets past every check stops at its machine. One line per case: name, exit
# status, last line said. Needs root, as the script does.
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
# The check after the loop (one topology, the daemon, in an upload row):
# rows whose topologies are all known, so that the loop's "unknown
# topology" does not end them first. A row that gets past every check
# goes on to its machine, where the made-up tools stop it.
run "G1 upload, zfs-skyzfsd alone: past every check" TB_TOPOLOGIES="zfs-skyzfsd" TB_SKZ_UPLOAD=1 "${S3[@]}"
run "G2 upload, spaces around zfs-skyzfsd"           TB_TOPOLOGIES="  zfs-skyzfsd  " TB_SKZ_UPLOAD=1 "${S3[@]}"
run "G3 upload, zfs-slog and zfs-skyzfsd"            TB_TOPOLOGIES="zfs-slog zfs-skyzfsd" TB_SKZ_UPLOAD=1 "${S3[@]}"
run "G4 upload, zfs-skyzfsd twice"                   TB_TOPOLOGIES="zfs-skyzfsd zfs-skyzfsd" TB_SKZ_UPLOAD=1 "${S3[@]}"
run "G5 no upload, zfs-slog and zfs-skyzfsd"         TB_TOPOLOGIES="zfs-slog zfs-skyzfsd"
echo "served: $([ -e "$T/served" ] && tr '\n' ';' < "$T/served" | cut -c1-300 || echo nothing)"
