#!/bin/bash
# #113 at 3fe4133: tear_at_kill.py (one writing thread, 300 kills) on each
# file system that could be had here, to see whether what a kill -9 leaves
# of a write(2) ends at a multiple of 4096 of the file on each.
#
#   next2_tear_fs.sh BASE
#
# BASE holds one directory per file system, mounted beforehand:
#   root         the container's own (ext4)
#   shm          a link to /dev/shm (tmpfs)
#   tmpfs-huge   mount -t tmpfs -o huge=always
#   ovl          mount -t overlay, its lower, upper and work directories on ext4
#   ext4-1k      mkfs.ext4 -b 1024 on a file, mount -o loop
#   ext4-4k      mkfs.ext4 -b 4096 on a file, mount -o loop,data=journal
here=$(cd "$(dirname "$0")" && pwd)
for d in root shm tmpfs-huge ovl ext4-1k ext4-4k; do
    dir=$(realpath "$1/$d")
    echo "== $d ($(findmnt -no FSTYPE,OPTIONS -T "$dir" | head -n 1 | tr -s ' ' | sed 's/lowerdir=[^ ]*/lowerdir=.../' | cut -c1-110))"
    python3 "$here/tear_at_kill.py" "$dir" 300 1
done
