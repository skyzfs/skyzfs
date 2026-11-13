#!/usr/bin/env bash
# Before a workflow's first apt step: if the names of the runner
# image's package mirrors do not resolve, use one mirror outside
# ubuntu.com instead.
#
#   bash tests/lib/apt-fallback.sh
#
# The GitHub runner images take every Ubuntu suite from the mirrors
# listed in /etc/apt/apt-mirrors.txt, all under ubuntu.com. On
# 2026-11-11 those names did not resolve for hours while every other
# name did, and each job that installs a package failed. When any
# mirror of the list does not resolve, this script replaces the list by
# the fallback alone and says so in the job's summary; when they all
# resolve it changes nothing. Every name of the list is looked up and
# not only the first: they are in one zone and failed together, but
# during the outage single names answered now and then, and a list
# with one dead name in it is slow. On a healthy day three lookups
# cost milliseconds. Nothing is put back, because a GitHub-hosted
# runner is thrown away after its job; for that reason the script
# changes nothing on any other runner (RUNNER_ENVIRONMENT set and not
# "github-hosted"), whose apt configuration outlives the job and is
# its owner's. It replaces and does not append: with
# the dead mirrors still in the list, `apt-get update` alone ran for
# more than six minutes (run 43060459690), against 5 s with the
# fallback alone.
#
# Nothing here loosens apt's checks: no key is added and no option is
# set. What the fallback serves is verified against the archive keys
# already on the image, as the image's own mirrors are; a mirror
# cannot change a package, and one whose metadata does not verify
# fails the apt step.
#
# Always exits 0 except on a usage error: when it cannot help (no
# list, the fallback's name does not resolve either), it says so and
# leaves apt to fail as it would have.
#
# Settings, for the test (tests/lib/test_apt_fallback.sh):
#   APT_MIRROR_LIST      the list file
#   APT_SOURCES          the files and directories that hold apt's sources
#   APT_FALLBACK_MIRROR  the fallback
#   APT_FALLBACK_ARCH    the machine's architecture, as dpkg names it,
#                        which chooses the fallback's tree
#   APT_FALLBACK_LOOKUP  a command that is given a host name and
#                        succeeds when it resolves
set -uo pipefail

# The one place that names the fallback. Ubuntu serves amd64 and i386
# from its archive and every other architecture (arm64 on the -arm
# runner images) from a separate tree, ubuntu-ports: a fallback of the
# wrong tree has no packages for the machine. kernel.org's copy of
# ubuntu-ports has no 26.04 (resolute): every Release was 404 (canary
# run 43356802335), so the ports tree comes from another mirror, which
# installed the project's packages on ubuntu-26.04-arm (run 43359493991).
# The ubuntu-24.04-arm image takes its packages from ports.ubuntu.com
# directly, with no mirror list, so there the script changes nothing.
APT_FALLBACK_ARCH=${APT_FALLBACK_ARCH:-$(dpkg --print-architecture 2>/dev/null || echo amd64)}
case $APT_FALLBACK_ARCH in
    amd64|i386) fb_default=https://mirrors.edge.kernel.org/ubuntu/ ;;
    *) fb_default=https://mirrors.ocf.berkeley.edu/ubuntu-ports/ ;;
esac
APT_FALLBACK_MIRROR=${APT_FALLBACK_MIRROR:-$fb_default}
APT_MIRROR_LIST=${APT_MIRROR_LIST:-/etc/apt/apt-mirrors.txt}
APT_SOURCES=${APT_SOURCES:-/etc/apt/sources.list /etc/apt/sources.list.d}
# A name that resolves answers in milliseconds; a dead one takes
# getent 20 s on a runner, so the lookup is cut at 5 s.
LOOKUP_S=5

# mirror_hosts FILE: the host of every mirror in a mirror list, one per
# line, in the file's order. A line is a URL, then optional words such
# as "priority:1"; empty lines and comments are skipped.
mirror_hosts() {
    awk '!/^[[:space:]]*(#|$)/ { print $1 }' "$1" | sed -n 's|^[A-Za-z][A-Za-z0-9+.-]*://\([^/:]*\).*|\1|p'
}

resolves() {
    if [ -n "${APT_FALLBACK_LOOKUP:-}" ]; then "$APT_FALLBACK_LOOKUP" "$1"; return; fi
    timeout "$LOOKUP_S" getent hosts "$1" > /dev/null 2>&1
}

say() { # to the log and to the job's summary
    echo "$*"
    [ -z "${GITHUB_STEP_SUMMARY:-}" ] || printf '%s\n\n' "$*" >> "$GITHUB_STEP_SUMMARY"
}

# An image whose sources do not go through the mirror list (as
# ubuntu-24.04-arm, whose deb822 file names ports.ubuntu.com) gets no
# fallback: rewriting its sources is more risk than the outage it
# covers. The job's summary says so, so that a failure there on a name
# that does not resolve is read as an outage.
no_fallback() {
    say "**Package mirrors: no fallback on this image.** $1 A step that installs packages fails when the names of its sources do not resolve, and that is an outage."
}

main() {
    local hosts h dead="" fb_host tmp as
    if [ -n "${RUNNER_ENVIRONMENT:-}" ] && [ "$RUNNER_ENVIRONMENT" != github-hosted ]; then
        echo "apt-fallback: this runner is $RUNNER_ENVIRONMENT, not github-hosted, and keeps its apt configuration after the job; nothing changed"
        return 0
    fi
    if [ ! -f "$APT_MIRROR_LIST" ]; then
        echo "apt-fallback: no $APT_MIRROR_LIST on this machine; nothing changed"
        no_fallback "There is no $APT_MIRROR_LIST on this machine."
        return 0
    fi
    # shellcheck disable=SC2086  # a list of paths
    if ! grep -rqsF "mirror+file:$APT_MIRROR_LIST" $APT_SOURCES; then
        echo "apt-fallback: no apt source uses $APT_MIRROR_LIST; nothing changed"
        no_fallback "No apt source of this image uses $APT_MIRROR_LIST; they name their mirrors directly."
        return 0
    fi
    hosts=$(mirror_hosts "$APT_MIRROR_LIST")
    if [ -z "$hosts" ]; then
        echo "apt-fallback: no mirror in $APT_MIRROR_LIST; nothing changed"
        return 0
    fi
    fb_host=$(printf '%s\n' "$APT_FALLBACK_MIRROR" | sed -n 's|^https://\([^/:]*\)/.*|\1|p')
    if [ -z "$fb_host" ]; then
        echo "apt-fallback: the fallback is not an https URL with a path: $APT_FALLBACK_MIRROR" >&2
        return 64
    fi
    if [ "$hosts" = "$fb_host" ]; then
        echo "apt-fallback: the list already holds the fallback alone; nothing changed"
        return 0
    fi
    for h in $hosts; do
        resolves "$h" || dead+="${dead:+, }$h"
    done
    if [ -z "$dead" ]; then
        echo "apt-fallback: the image's mirrors resolve ($(echo "$hosts" | tr '\n' ' ')); nothing changed"
        return 0
    fi
    if ! resolves "$fb_host"; then
        say "**Package mirrors:** $dead did not resolve, and neither did the fallback's name $fb_host. Nothing changed; a step that installs packages will fail, and that is a network outage, not the change under test."
        return 0
    fi
    # The new list is made beside the old one and moved over it, so the
    # list is at every moment the image's or the fallback's, whole.
    tmp=$(mktemp) || { say "**Package mirrors:** $dead did not resolve, and no temporary file could be made. Nothing changed."; return 0; }
    if ! { printf '%s\tpriority:1\n' "$APT_FALLBACK_MIRROR" > "$tmp" && chmod 644 "$tmp"; }; then
        rm -f "$tmp"
        say "**Package mirrors:** $dead did not resolve, and the new list could not be written to a temporary file. Nothing changed."
        return 0
    fi
    if [ -w "$APT_MIRROR_LIST" ] && [ -w "$(dirname "$APT_MIRROR_LIST")" ]; then as=""; else as=sudo; fi
    if ! $as cp "$APT_MIRROR_LIST" "$APT_MIRROR_LIST.image"; then
        rm -f "$tmp"
        say "**Package mirrors:** $dead did not resolve, and the image's list could not be copied to $APT_MIRROR_LIST.image. Nothing changed."
        return 0
    fi
    if ! $as cp "$tmp" "$APT_MIRROR_LIST.new" || ! $as mv "$APT_MIRROR_LIST.new" "$APT_MIRROR_LIST"; then
        rm -f "$tmp"
        say "**Package mirrors:** $dead did not resolve, and the fallback could not be put in place. The list $APT_MIRROR_LIST is the image's, unchanged (a copy is in $APT_MIRROR_LIST.image)."
        return 0
    fi
    rm -f "$tmp"
    say "**Package mirrors: the fallback is in use.** The image's mirror $dead did not resolve, so this job installs its packages from $APT_FALLBACK_MIRROR. apt verifies them against the image's archive keys as before."
    [ -z "${GITHUB_ACTIONS:-}" ] || echo "::notice title=Package mirrors::$dead did not resolve; packages come from $APT_FALLBACK_MIRROR"
    return 0
}

# Sourced by the test for mirror_hosts; run by the workflows.
if [ "${BASH_SOURCE[0]}" = "$0" ]; then main "$@"; fi
