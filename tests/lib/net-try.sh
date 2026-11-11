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
