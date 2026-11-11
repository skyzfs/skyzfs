# shellcheck shell=bash
# Device paths, for the rigs. Sourced, not run.
#
# A tool that opens a device's path for writing and may create it (dd
# without conv=nocreat, a shell redirection) makes a REGULAR FILE of
# that name in /dev when the device has just been deleted, and writes
# to it with success. The name is then taken: the kernel cannot make
# the node of the next device with that number, and whoever is told
# "/dev/ublkbN" finds the file. So:
#   - every dd that writes to a device has conv=nocreat;
#   - a path is used as a device only when it is a block device;
#   - dev_strays lists what stands in the way, and finding any is the
#     rig's error.

# dev_strays [DIR]: every entry of DIR (default /dev) named like a ublk
# block device that is not a block device, as `ls -ld` shows it, one per
# line. Nothing printed when there is none.
dev_strays() {
    local f
    for f in "${1:-/dev}"/ublkb*; do
        { [ -e "$f" ] || [ -L "$f" ]; } || continue
        [ -b "$f" ] || ls -ld "$f"
    done
}
