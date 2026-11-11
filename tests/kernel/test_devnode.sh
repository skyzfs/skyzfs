#!/bin/bash
# Tests of devnode.sh and of the dd rule it states: no kernel feature.
# Run: bash tests/kernel/test_devnode.sh
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=tests/kernel/devnode.sh
. "$HERE/devnode.sh"
# shellcheck source=tests/lib/gnu-dd.sh
. "$HERE/../lib/gnu-dd.sh"
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
fails=0
is() { # name got want
    if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got '$2', want '$3'"; fails=$((fails + 1)); fi
}

# The fault: a plain dd to a path that is not there makes a file.
"$DD" if=/dev/zero of="$T/ublkb3" bs=4k count=1 status=none 2>/dev/null
is "dd without nocreat creates the missing path" "$([ -f "$T/ublkb3" ] && echo created)" created
rm -f "$T/ublkb3"
# The rule: with conv=nocreat it fails and creates nothing.
"$DD" if=/dev/zero of="$T/ublkb3" bs=4k count=1 conv=nocreat status=none 2>/dev/null; rc=$?
is "dd with nocreat fails on the missing path" "$([ $rc != 0 ] && echo failed)" failed
is "and creates nothing" "$([ -e "$T/ublkb3" ] && echo created || echo nothing)" nothing
# As the rigs call it, with direct I/O as well.
"$DD" if=/dev/zero of="$T/ublkb3" bs=4k count=1 seek=7 oflag=direct conv=nocreat status=none 2>/dev/null; rc=$?
is "the same with oflag=direct and seek" "$([ $rc != 0 ] && [ ! -e "$T/ublkb3" ] && echo failed-nothing)" failed-nothing
# On a path that is there it still writes.
: > "$T/there"
"$DD" if=/dev/zero of="$T/there" bs=4k count=1 conv=nocreat status=none 2>/dev/null
is "nocreat writes to what exists" "$(wc -c < "$T/there" | tr -d ' ')" 4096

# The strays.
mkdir "$T/dev"
is "an empty directory has none" "$(dev_strays "$T/dev")" ""
: > "$T/dev/ublkb3"; mkdir "$T/dev/ublkb12"; : > "$T/dev/ublkc3"; : > "$T/dev/sda"
out=$(dev_strays "$T/dev")
is "a regular file and a directory under a device's name are strays" "$(printf '%s\n' "$out" | wc -l | tr -d ' ')" 2
is "the listing names the file" "$(printf '%s\n' "$out" | grep -c "^-.*$T/dev/ublkb3\$")" 1
is "other names are not looked at" "$(printf '%s\n' "$out" | grep -c 'ublkc3\|sda')" 0
# A real block device under such a name is not a stray. Making one
# needs a privilege the test may not have; then this case is skipped.
if mknod "$T/dev/ublkb7" b 7 250 2>/dev/null; then
    is "a block device is not a stray" "$(dev_strays "$T/dev" | grep -c 'ublkb7')" 0
else
    echo "skip a block device is not a stray (mknod not permitted here)"
fi
is "a directory that is not there has none" "$(dev_strays "$T/none")" ""

[ "$fails" = 0 ] || { echo "$fails failed"; exit 1; }
echo "all passed"
