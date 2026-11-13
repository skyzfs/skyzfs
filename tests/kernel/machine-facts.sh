#!/usr/bin/env bash
# What a runner is and has, as Markdown, before anything is installed:
# the architecture and page size, the kernel, whether the modules the
# rigs load are in the kernel's tree, whether the distribution offers
# the packages the rigs install, and the work disk with its write
# cache. Changes nothing on the machine but apt's package lists.
# Exit 0 whatever it finds: it reports.
set -u

row() { printf '| %s | %s |\n' "$1" "$2"; }
mod() { # name: where the module is, or "not in this kernel's tree"
    local f
    if f=$(modinfo -n "$1" 2>/dev/null); then echo "${f#/lib/modules/*/}"
    elif grep -qw "^${1//-/_}" /proc/modules 2>/dev/null; then echo "loaded, built in or from elsewhere"
    else echo "not in this kernel's tree"; fi
}
pkg() { # name: the candidate version apt offers, or "not offered"
    local v
    v=$(apt-cache policy "$1" 2>/dev/null | awk '$1 == "Candidate:" { print $2; exit }')
    if [ -n "$v" ] && [ "$v" != "(none)" ]; then echo "$v"; else echo "not offered"; fi
}

sudo apt-get update -qq >/dev/null 2>&1 || echo "apt-get update failed" >&2
kver=$(uname -r)
# The disk under /mnt, found by its device number: the name the mount
# table gives can be an alias such as /dev/root.
work=$(findmnt -n -o SOURCE -T /mnt 2>/dev/null | head -n 1)
majmin=$(findmnt -n -o MAJ:MIN -T /mnt 2>/dev/null | head -n 1 | tr -d " ")
# A file system with no block device of its own (major 0, as tmpfs or
# overlay) has no disk to name.
if [ -n "$majmin" ] && [ -e "/sys/dev/block/$majmin" ]; then
    dev=$(basename "$(readlink -f "/sys/dev/block/$majmin")")
else
    dev="no block device"
fi
parent=$dev
[ -e "/sys/class/block/$dev/partition" ] && parent=$(basename "$(readlink -f "/sys/class/block/$dev/..")")

model=""
[ ! -r "/sys/block/$parent/device/model" ] || model=$(xargs < "/sys/block/$parent/device/model")

echo "### The machine"
echo
echo "| | |"
echo "|---|---|"
row "architecture" "$(uname -m) ($(dpkg --print-architecture 2>/dev/null))"
row "page size" "$(getconf PAGESIZE) bytes"
row "kernel" "$kver"
# shellcheck source=/dev/null
row "os" "$(. /etc/os-release && echo "$PRETTY_NAME")"
row "CPUs, memory" "$(nproc), $(awk '/MemTotal/ { printf "%.1f GiB", $2 / 1048576 }' /proc/meminfo)"
row "ublk_drv" "$(mod ublk_drv)"
row "zfs module" "$(mod zfs)"
row "dm-log-writes" "$(mod dm-log-writes)"
row "dm-snapshot" "$(mod dm-snapshot)"
row "dm-delay" "$(mod dm-delay)"
row "loop" "$(mod loop)"
# The system call's own entry (__x64_sys_ or __arm64_sys_io_uring_setup)
# is there whether or not the compiler kept the static function apart.
row "io_uring" "$(grep -q 'sys_io_uring_setup$' /proc/kallsyms 2>/dev/null && echo "in the kernel" || echo "not found in kallsyms")"
for p in zfsutils-linux zfs-dkms "linux-modules-extra-$kver" libdevmapper-dev libudev-dev libclang-dev xfsprogs postgresql fio perf-tools-unstable "linux-tools-$kver" strace; do
    row "package $p" "$(pkg "$p")"
done
row "/mnt" "$(df -h --output=source,size,avail,fstype /mnt 2>/dev/null | tail -n 1)"
row "work disk" "$work ($majmin, $dev), on $parent: model ${model:-not said}; write_cache $(cat "/sys/block/$parent/queue/write_cache" 2>/dev/null || echo 'not said'); rotational $(cat "/sys/block/$parent/queue/rotational" 2>/dev/null || echo '?')"
row "its mount" "$(findmnt -n -o FSTYPE,OPTIONS -T /mnt 2>/dev/null)"
echo
exit 0
