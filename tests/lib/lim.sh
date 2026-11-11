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
# command runs in the background of the calling shell, with its stdin,
# stdout and stderr.
lim() {
    local secs=$1 pid state n=0
    shift
    "$@" <&0 &      # an asynchronous command would otherwise read /dev/null
    pid=$!
    while :; do
        # Field 3 of /proc/PID/stat, after the command name in brackets.
        state=$(sed 's/^.*) //' "/proc/$pid/stat" 2>/dev/null | cut -c1)
        case $state in
            ''|Z|X) wait "$pid"; return $? ;;
        esac
        if [ "$n" -ge $((secs * 10)) ]; then
            kill -9 "$pid" 2>/dev/null
            sleep 0.5
            state=$(sed 's/^.*) //' "/proc/$pid/stat" 2>/dev/null | cut -c1)
            case $state in
                ''|Z|X) wait "$pid" 2>/dev/null ;;
            esac
            return 124
        fi
        sleep 0.1
        n=$((n + 1))
    done
}
