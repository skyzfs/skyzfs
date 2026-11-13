#!/usr/bin/env bash
# reason-harness.sh RIG_TREE reason DIR      the rig's check_g_reason on the files of
#                                            an artifact (DIR holds vol-g*.serve.log and
#                                            g*-again.err); prints the pin it writes
# reason-harness.sh RIG_TREE branch reference|log|nofs
#                                            the lines the rig writes for check g without
#                                            running it (the stand-in, the log store, no
#                                            file system), then the judge on them
#
# Nothing of the rig is copied here: the functions are cut out of
# RIG_TREE/tests/kernel/daemon-rig.sh as it stands and run with
# stand-ins for what needs a kernel. Review of #56 (M1.T26).
set -uo pipefail
TREE=$1; MODE=$2; ARG=$3
RIG=$TREE/tests/kernel/daemon-rig.sh
T=$(mktemp -d) || exit 2
[ -n "$T" ] && [ -d "$T" ] || exit 2
trap 'rm -rf "${T:?}"' EXIT
R=$T/results.tsv; : > "$R"
# rec, pin, ctl, obs as the rig has them.
eval "$(sed -n '/^rec() {/,/^}/p' "$RIG")"
eval "$(grep -E '^(pin|ctl|obs)\(\) \{' "$RIG")"
eval "$(sed -n '/^pin_is() {/,/^}/p' "$RIG")"
eval "$(grep -E '^G_PINS=' "$RIG")"
step() { :; }
in_place() { [ "${STORE:-}" = in-place ]; }
fn() { sed -n "/^$1() {/,/^}/p" "$RIG"; }

case $MODE in
reason)
    WORK=$ARG; OUT=$ARG
    eval "$(grep -E '^G_REASON=' "$RIG")"
    eval "$(fn check_g_reason)"
    check_g_reason
    cut -c1-420 "$R"
    ;;
branch)
    case $ARG in
        reference) IMPL=reference; STORE="" ;;
        log) IMPL=daemon; STORE=log ;;
        nofs) IMPL=daemon; STORE=in-place ;;
    esac
    CHECKS=" g "
    if [ "$ARG" = nofs ]; then
        g_fs_up() { echo "no loop device here"; return 1; }
        OUT=$T; G_DM=x; G_LOOP=x; G_SECTORS=0
        eval "$(fn check_g)"
    else
        check_g() { echo "check_g would run here" >&2; }
    fi
    # The last block of the rig that begins with check g's case.
    eval "$(awk '/^case \$CHECKS in \*" g "\*\)/ { on = 1; buf = "" } on { buf = buf $0 "\n" } on && /^esac/ { on = 0; last = buf } END { printf "%s", last }' "$RIG")"
    rec impl "$IMPL: a stand-in for the harness"
    rec store "${STORE:-none: the stand-in has one store, a file it writes in place}"
    rec setup ok
    rec dev_strays none
    echo "--- the lines the rig wrote for check g:"
    grep -E '^(pin|ctl|obs)_' "$R" | cut -f1 | tr '\n' ' '; echo
    python3 -I "$TREE/tests/kernel/daemon_check.py" "$T" --checks g > "$T/summary.md"; rc=$?
    echo "--- the judge's exit: $rc"
    sed -n '/^\*\*/,/^$/p;/^- /p' "$T/summary.md" | grep -E '^\*\*|^- (pin|ctl|obs)' | cut -c1-200
    ;;
esac
