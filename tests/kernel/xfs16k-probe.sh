#!/usr/bin/env bash
# A probe, record only: can this machine make and mount an XFS whose
# block (16 KiB) is larger than the page, and what does skyzfsd do with
# a volume directory there (work package M1.3g: a volume's write unit is
# at least the block size of the file system it is on)?
#
#   xfs16k-probe.sh SKYZFSD OUT_DIR [WORK_DIR]
#
# It judges nothing and always exits 0 once it has started: every step
# writes what it saw to OUT_DIR/probe.txt, a refusal included. Needs
# root, losetup, mkfs.ext4, mkfs.xfs; for part 3 the ublk driver and ZFS.
# shellcheck source=tests/lib/lim.sh
. "$(dirname "${BASH_SOURCE[0]}")/../lib/lim.sh"
# shellcheck source=tests/lib/gnu-dd.sh
. "$(dirname "${BASH_SOURCE[0]}")/../lib/gnu-dd.sh"

SKYZFSD=$1; OUT=$2; W=${3:-/mnt/xfs16k-probe}
VOL_BYTES=$((8 << 30))
mkdir -p "$OUT" "$W"
P=$OUT/probe.txt
: > "$P"
say() { printf '%s\n' "$*" | tee -a "$P"; }
# run LABEL SECONDS COMMAND...: the command under a limit, its output
# and its exit status written down. The status is also in RC.
run() {
    local label=$1 secs=$2; shift 2
    lim "$secs" "$@" > "$W/run.out" 2> "$W/run.err"; RC=$?
    say "-- $label: exit $RC"
    sed 's/^/   out: /' "$W/run.out" | cut -c1-400 | tee -a "$P"
    sed 's/^/   err: /' "$W/run.err" | cut -c1-400 | tee -a "$P"
    return "$RC"
}
sums() { (cd "$1" && sha256sum -- * 2>&1 | sort -k2); }    # dir -> the checksums of its files

say "== the machine"
say "kernel $(uname -r), page size $(getconf PAGESIZE)"
say "mkfs.xfs: $(mkfs.xfs -V 2>&1 | head -n 1)"
say "mkfs.ext4: $(mkfs.ext4 -V 2>&1 | head -n 1)"
say "skyzfsd: $SKYZFSD, $(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse HEAD 2>/dev/null)"
modprobe xfs 2>/dev/null
say "xfs in /proc/filesystems: $(grep -c -w xfs /proc/filesystems)"

# ---- 1: the file systems
declare -A MNT=() LOOP=()
mkfs_of() {
    case $1 in
        ext4) echo "mkfs.ext4 -q -F" ;;
        xfs) echo "mkfs.xfs -q -f" ;;
        xfs16k) echo "mkfs.xfs -q -f -b size=16k" ;;
    esac
}
for fs in ext4 xfs xfs16k; do
    say ""
    say "== 1: $fs ($(mkfs_of "$fs"))"
    truncate -s 40G "$W/$fs.img"
    loop=$(losetup --find --show --direct-io=on "$W/$fs.img" 2>> "$P") || { say "losetup failed"; continue; }
    LOOP[$fs]=$loop
    # shellcheck disable=SC2046  # the command and its options, words on purpose
    if ! run "mkfs" 300 $(mkfs_of "$fs") "$loop"; then say "$fs: mkfs REFUSED"; continue; fi
    mkdir -p "$W/mnt-$fs"
    if ! run "mount" 60 mount "$loop" "$W/mnt-$fs"; then
        say "$fs: mount REFUSED; the kernel's last lines: $(dmesg 2>/dev/null | tail -n 4 | tr '\n' ';' | cut -c1-600)"
        continue
    fi
    MNT[$fs]=$W/mnt-$fs
    say "$fs: mounted. stat -f: $(stat -f -c 'type %T, block size %s, fundamental block size %S' "$W/mnt-$fs")"
    say "$fs: stat of the directory: $(stat -c 'st_blksize %o' "$W/mnt-$fs")"
    case $fs in xfs*) run "xfs_info" 30 xfs_info "$W/mnt-$fs" ;; esac
    say "$fs: mount line: $(grep " $W/mnt-$fs " /proc/mounts)"
done

# ---- 2: create on each; then a volume made on ext4, copied to the 16 KiB XFS, served there
vol_look() { # dir: what a volume directory holds, and what status says of it
    say "   files: $(ls -ln "$1" 2>&1 | awk 'NR > 1 { printf "%s %s bytes; ", $NF, $5 }')"
    run "status" 30 "$SKYZFSD" status --dir "$1" --run-dir "$W/run"
}
mkdir -p "$W/run"
for fs in ext4 xfs xfs16k; do
    say ""
    say "== 2: skyzfsd create on $fs"
    [ -n "${MNT[$fs]:-}" ] || { say "not mounted: nothing to try"; continue; }
    if run "create --dir ${MNT[$fs]}/vol --size $VOL_BYTES" 120 "$SKYZFSD" create --dir "${MNT[$fs]}/vol" --size "$VOL_BYTES"; then
        vol_look "${MNT[$fs]}/vol"
    else
        say "$fs: create exit $RC; the directory: $(ls -la "${MNT[$fs]}/vol" 2>&1 | tr '\n' ';' | cut -c1-300)"
    fi
done

# serve_try DIR SECONDS: serve in the background; -> SERVED (device or ""), SERVE_RC ("" while it runs)
SERVE_PID=""
serve_try() {
    local dir=$1
    SERVED=""; SERVE_RC=""
    "$SKYZFSD" serve --dir "$dir" --run-dir "$W/run" > "$W/serve.out" 2> "$W/serve.err" &
    SERVE_PID=$!
    for _ in $(seq 1 $(($2 * 4))); do
        SERVED=$(sed -n 's/^ready \(\/dev\/ublkb[0-9]*\)$/\1/p' "$W/serve.out" | head -n 1)
        [ -n "$SERVED" ] && [ -b "$SERVED" ] && break
        if ! kill -0 "$SERVE_PID" 2>/dev/null; then wait "$SERVE_PID"; SERVE_RC=$?; SERVE_PID=""; break; fi
        sleep 0.25
    done
    say "-- serve --dir $dir: $([ -n "$SERVE_RC" ] && echo "exit $SERVE_RC" || echo "running")$([ -n "$SERVED" ] && echo ", ready $SERVED")"
    sed 's/^/   out: /' "$W/serve.out" | cut -c1-500 | tee -a "$P"
    sed 's/^/   err: /' "$W/serve.err" | cut -c1-500 | tee -a "$P"
}
serve_end() { # dir: SIGTERM, wait, detach

    if [ -n "$SERVE_PID" ]; then
        kill -TERM "$SERVE_PID" 2>/dev/null
        for _ in $(seq 1 240); do kill -0 "$SERVE_PID" 2>/dev/null || break; sleep 0.25; done
        if kill -0 "$SERVE_PID" 2>/dev/null; then say "   serve did not end within 60 s of SIGTERM; killed"; kill -9 "$SERVE_PID"
        else wait "$SERVE_PID"; say "   serve ended with $? on SIGTERM"; fi
        SERVE_PID=""
    fi
    run "detach" 90 "$SKYZFSD" detach --dir "$1" --run-dir "$W/run"
}

say ""
say "== 2: a volume made on ext4, copied to the 16 KiB XFS, served there"
if [ -n "${MNT[ext4]:-}" ] && [ -n "${MNT[xfs16k]:-}" ] && [ -d "${MNT[ext4]}/vol" ]; then
    run "cp -a --sparse=always" 300 cp -a --sparse=always "${MNT[ext4]}/vol" "${MNT[xfs16k]}/copied"
    sums "${MNT[ext4]}/vol" > "$W/sums.src"; sums "${MNT[xfs16k]}/copied" > "$W/sums.before"
    say "the copy's files equal the source's: $(cmp -s "$W/sums.src" "$W/sums.before" && echo yes || echo NO)"
    serve_try "${MNT[xfs16k]}/copied" 30
    if [ -n "$SERVED" ]; then say "IT SERVED: the copy was not refused"; fi
    serve_end "${MNT[xfs16k]}/copied"
    sums "${MNT[xfs16k]}/copied" > "$W/sums.after"
    say "the copy's files after the serve are as before it: $(cmp -s "$W/sums.before" "$W/sums.after" && echo yes || { echo NO; diff "$W/sums.before" "$W/sums.after" | tr '\n' ';'; })"
    sed 's/^/   /' "$W/sums.after" | tee -a "$P"
else
    say "not tried: ext4 mounted '${MNT[ext4]:-no}', xfs16k mounted '${MNT[xfs16k]:-no}'"
fi

# ---- 3: a volume created on the 16 KiB XFS, served, under a small pool
say ""
say "== 3: a volume created on the 16 KiB XFS: serve, a pool, a write, a scrub"
if [ -n "${MNT[xfs16k]:-}" ] && [ -f "${MNT[xfs16k]}/vol/home.img" ]; then
    modprobe ublk_drv 2>/dev/null; modprobe zfs 2>/dev/null
    serve_try "${MNT[xfs16k]}/vol" 60
    if [ -n "$SERVED" ]; then
        q=/sys/block/$(basename "$SERVED")/queue
        say "the device: logical block $(cat "$q/logical_block_size"), physical block $(cat "$q/physical_block_size"), minimum_io_size $(cat "$q/minimum_io_size"), optimal_io_size $(cat "$q/optimal_io_size"), max_sectors_kb $(cat "$q/max_sectors_kb"), size $(blockdev --getsize64 "$SERVED")"
        mkdir -p "$W/pool"
        if run "zpool create (ashift left to ZFS)" 180 zpool create -f -o cachefile=none -O mountpoint="$W/pool" -O compression=off skzprobe "$SERVED"; then
            say "ashift ZFS chose: $(lim 30 zpool get -H -o value ashift skzprobe 2>&1), of the vdev: $(lim 30 zdb -C skzprobe 2>/dev/null | grep -m1 ashift | tr -s ' ')"
            run "write 16 MiB and fsync" 120 "$DD" if=/dev/urandom of="$W/pool/file" bs=1M count=16 conv=fsync status=none
            want=$(sha256sum < "$W/pool/file" | cut -d' ' -f1)
            run "zpool sync" 120 zpool sync skzprobe
            run "zpool scrub -w" 600 zpool scrub -w skzprobe
            run "zpool status" 60 zpool status -v skzprobe
            vol_look "${MNT[xfs16k]}/vol"
            run "zpool export" 120 zpool export skzprobe
            # Once more through a stop and a new serve: what was written reads back.
            serve_end "${MNT[xfs16k]}/vol"
            serve_try "${MNT[xfs16k]}/vol" 60
            if [ -n "$SERVED" ] && run "zpool import" 120 zpool import -d "$SERVED" -o cachefile=none skzprobe; then
                say "the file after a stop, a serve and an import: $([ "$(sha256sum < "$W/pool/file" | cut -d' ' -f1)" = "$want" ] && echo 'as written' || echo 'NOT as written')"
                run "zpool export" 120 zpool export skzprobe
            fi
        fi
    fi
    serve_end "${MNT[xfs16k]}/vol"
else
    say "not tried: no volume was created on the 16 KiB XFS"
fi

say ""
say "== the kernel log's lines on xfs, ublk and zfs"
dmesg 2>/dev/null | grep -iE 'xfs|ublk|zio|blocksize|block size' | tail -n 30 | cut -c1-300 | tee -a "$P"

for fs in ext4 xfs xfs16k; do
    [ -z "${MNT[$fs]:-}" ] || lim 60 umount "${MNT[$fs]}"
    [ -z "${LOOP[$fs]:-}" ] || losetup -d "${LOOP[$fs]}" 2>/dev/null
done
exit 0
