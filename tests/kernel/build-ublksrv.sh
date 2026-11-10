#!/usr/bin/env bash
# Build and install the reference ublk server (ublk-org/ublksrv, the
# `ublk` command) from source. No Ubuntu release packages it.
#
#   build-ublksrv.sh [git-ref]     default: the repository's default branch
#
# Needs root (apt-get, make install). Prints the commit that was built
# on the last line of stdout as "ublksrv <describe> <sha>".
set -euo pipefail
REF=${1:-}
SRC=${UBLKSRV_SRC:-/tmp/ublksrv}

export DEBIAN_FRONTEND=noninteractive
apt-get install -y --no-install-recommends \
    git ca-certificates build-essential autoconf automake libtool pkg-config liburing-dev >&2

rm -rf "$SRC"
git clone --quiet https://github.com/ublk-org/ublksrv "$SRC" >&2
cd "$SRC"
[ -z "$REF" ] || git checkout --quiet "$REF" >&2
{
    autoreconf -fi
    ./configure
    make -j"$(nproc)"
    make install
    ldconfig
} >&2
command -v ublk >/dev/null || { echo "ublk is not on PATH after make install" >&2; exit 1; }
echo "ublksrv $(git describe --tags --always 2>/dev/null || echo unknown) $(git rev-parse HEAD)"
