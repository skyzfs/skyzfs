# shellcheck shell=bash
# The serve options a rig or the benchmark hands to a build of skyzfsd,
# as far as the store goes (M1.T34). Sourced by
# tests/kernel/replay-rig.sh, tests/kernel/hole-rig.sh and
# tests/bench/topology-bench.sh.
#
# The daemon has one store since M1.3t, and since M1.3t-d no option
# `--store`: a current build ends its first serve with a usage error
# (exit 2) when given one, and a rig would read that as a verdict
# against the daemon. A build from between the choice of store and
# M1.3t-d (a negative control, a TB_SKYZFSD_REF of an older commit)
# offers `[--store` in its help and defaults to the log store, so it is
# served with `--store in-place`. The log store's own options are of
# no build that is still compared.
#
# skz_store_args SKYZFSD [OPTION...]: 0 when the options may go to a
# serve of that build. SKZ_STORE_ADD is then 1 when the build has a
# choice of store and the options name none: the caller adds
# `--store in-place`. 1 when they may not, with the reason in
# SKZ_STORE_WHY, before anything is served.

# The words of a log store option, with its value as the next word or
# after `=`.
SKZ_LOG_STORE_OPTIONS=" --log-high --log-low --writeback-kib --home-writeback-kib --readers --step-pause-ms --step-pause-until --step-pause-at --refuse-holes "

# shellcheck disable=SC2034  # SKZ_STORE_ADD and SKZ_STORE_WHY are read by the callers
skz_store_args() {
    local build=$1 help choice=0 named=0 i o v
    shift
    local args=("$@")
    SKZ_STORE_ADD=0; SKZ_STORE_WHY=""
    # The help is read whole, not piped into grep -q: a reader that ends
    # early can fail the writer with SIGPIPE under pipefail.
    help=$("$build" help 2>&1) || true
    [[ $help != *'[--store'* ]] || choice=1
    for ((i = 0; i < ${#args[@]}; i++)); do
        o=${args[i]}
        case $o in
            --store|--store=*)
                if [ "$o" = --store ]; then i=$((i + 1)); v=${args[i]:-}; else v=${o#--store=}; fi
                if [ "$v" != in-place ]; then
                    SKZ_STORE_WHY="--store ${v:-(no value)} is of the log store, which M1.3t removed"
                    return 1
                fi
                if [ "$choice" = 0 ]; then
                    SKZ_STORE_WHY="--store in-place: this build of skyzfsd ($build) has no option --store (its help offers no [--store; M1.3t-d removed it with the choice of store), so its first serve would end with a usage error that reads as a verdict"
                    return 1
                fi
                named=1 ;;
            *)
                case $SKZ_LOG_STORE_OPTIONS in
                    *" ${o%%=*} "*) SKZ_STORE_WHY="$o is of the log store, which M1.3t removed"; return 1 ;;
                esac ;;
        esac
    done
    [ "$choice" = 0 ] || [ "$named" = 1 ] || SKZ_STORE_ADD=1
    return 0
}
