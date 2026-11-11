# shellcheck shell=bash
# net_try COMMAND...: run a command that needs the network up to three
# times, a minute apart, and return its last status. A runner's name
# resolution can fail for minutes at a time (2026-11-11: apt could not
# resolve the Ubuntu mirrors for over an hour, on and off), and a rig
# that gives up at the first failure loses a half-hour run to it.
# NET_TRY_PAUSE (seconds, default 60) is the pause; NET_TRIES says how
# many tries the last call took.
net_try() {
    local rc=0
    for NET_TRIES in 1 2 3; do
        "$@"; rc=$?
        [ $rc = 0 ] && return 0
        [ "$NET_TRIES" = 3 ] || sleep "${NET_TRY_PAUSE:-60}"
    done
    return $rc
}

# net_why LOG...: why a network step failed, in one line, from its
# log. A name that does not resolve is said in plain words first: that
# is an outage of the mirrors' names (2026-11-11: Canonical's DNS, for
# hours), not something a rerun in five minutes or the change under
# test will mend.
net_why() {
    local line
    line=$(grep -h -m 1 -E 'Temporary failure resolving|Temporary failure in name resolution|Could not resolve' "$@" 2>/dev/null | head -n 1)
    if [ -n "$line" ]; then
        echo "could not resolve the package mirrors or the source's host (${line#*: }): a network outage, not the rig or the change"
        return
    fi
    line=$(grep -h -m 1 -E '^E: |^fatal: |^error: ' "$@" 2>/dev/null | head -n 1)
    echo "${line:-no reason found in $(basename "$1")}"
}
