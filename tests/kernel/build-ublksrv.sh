#!/usr/bin/env bash
# Build and install the reference ublk server (ublk-org/ublksrv, the
# `ublk` command) from source. No Ubuntu release packages it.
#
#   build-ublksrv.sh [git-ref]     default: the repository's default branch.
#                                  Callers pass a pinned commit: this is
#                                  third-party code that then runs as root.
#
# Needs root (apt-get, make install). Prints the commit that was built
# on the last line of stdout as "ublksrv <describe> <sha>".
set -euo pipefail
REF=${1:-}
# A ref that starts with "-" would be read by git as an option.
case $REF in
    -*) echo "bad git ref: $REF" >&2; exit 64 ;;
    *[!A-Za-z0-9._/-]*) echo "bad git ref: $REF" >&2; exit 64 ;;
esac
SRC=${UBLKSRV_SRC:-/tmp/ublksrv}

export DEBIAN_FRONTEND=noninteractive
apt-get install -y --no-install-recommends \
    git ca-certificates build-essential autoconf automake libtool pkg-config liburing-dev >&2

rm -rf "$SRC"
git clone --quiet https://github.com/ublk-org/ublksrv "$SRC" >&2
cd "$SRC"
[ -z "$REF" ] || git checkout --quiet --detach "$REF" >&2
# UBLKSRV_PATCH: a patch to apply before building. The daemon rig builds
# the loop target with test switches (tests/kernel/ublksrv-skz.patch);
# the probe, the replay rig and the benchmark build it as it is.
if [ -n "${UBLKSRV_PATCH:-}" ]; then
    git apply --verbose "$UBLKSRV_PATCH" >&2
    echo "patched with $(basename "$UBLKSRV_PATCH")" >&2
fi
{
    autoreconf -fi
    ./configure
    make -j"$(nproc)"
    make install
    ldconfig
} >&2
command -v ublk >/dev/null || { echo "ublk is not on PATH after make install" >&2; exit 1; }
echo "ublksrv $(git describe --tags --always 2>/dev/null || echo unknown) $(git rev-parse HEAD)"
