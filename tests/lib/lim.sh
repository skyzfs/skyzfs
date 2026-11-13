# shellcheck shell=bash
# lim SECONDS COMMAND...: run a command under a time limit that holds
# even when the command cannot be killed.
#
# timeout(1) waits for its child after killing it, and a process that
# sleeps uninterruptibly in the kernel does not die: a write to a
# suspended ZFS pool, or I/O on a ublk device with no server. A rig that
# provokes exactly those states would then hang with it. This gives up
# on such a child and returns 124, leaving it behind; the callers run on
# throwaway machines.
#
# Returns the command's status, or 124 when the limit passed. The
# command runs in the background of the calling shell, with its stdin.
# What it writes to stdout and stderr is passed on when lim returns,
# stdout first; a command abandoned at the limit passes on what it had
# written by then.
#
# It holds under `set -e` and `set -o pipefail` as well: the look at a
# process that has just ended fails (its /proc entry is gone), and in a
# script with errexit that failure used to end the caller with status 2
# and no message, before the command's output was passed on. Every
# step that can fail here is therefore allowed to (`|| true`, and the
# command's status taken with `|| rc=$?`).
lim() {
    local secs=$1 pid state n=0 out err rc
    shift
    # The command writes to files, not to the caller's stdout and
    # stderr: a child that is left behind would otherwise keep a pipe
    # open, and a caller reading that pipe would wait for it for ever.
    out=$(mktemp) && err=$(mktemp) || return 125
    "$@" <&0 > "$out" 2> "$err" &      # an asynchronous command would otherwise read /dev/null
    pid=$!
    rc=""
    while [ -z "$rc" ]; do
        # Field 3 of /proc/PID/stat, after the command name in brackets.
        state=$(sed 's/^.*) //' "/proc/$pid/stat" 2>/dev/null | cut -c1) || true
        case $state in
            ''|Z|X) rc=0; wait "$pid" || rc=$?; break ;;
        esac
        if [ "$n" -ge $((secs * 10)) ]; then
            kill -9 "$pid" 2>/dev/null || true
            sleep 0.5
            state=$(sed 's/^.*) //' "/proc/$pid/stat" 2>/dev/null | cut -c1) || true
            case $state in
                ''|Z|X) wait "$pid" 2>/dev/null || true ;;
            esac
            rc=124
            break
        fi
        sleep 0.1
        n=$((n + 1))
    done
    cat "$out" || true
    cat "$err" >&2 || true
    unlink "$out" || true
    unlink "$err" || true
    return "$rc"
}
