# shellcheck shell=bash
# shellcheck disable=SC2034  # the LK_* results are read by the script that sources this
# The looks at a device over a wait, for the rigs. Sourced, not run.
#
# look_word STATE: the word to write down for one look. A look that
# named no state (the tool ran out of time, failed, or printed
# something else) is "none": it is an attempt and is counted, never
# dropped.
look_word() { if [[ ${1:-} =~ ^[A-Z_]+$ ]]; then echo "$1"; else echo none; fi; }

# looks_judge "WORDS" WANT SECONDS: judge the looks of a wait of SECONDS
# during which the device was to read WANT at every look.
#   LK_ALL    attempts        LK_NONE   looks that named no state
#   LK_OTHER  looks that named a state other than WANT
#   LK_FLOOR  the attempts a wait of that length must have: one per
#             12 s, which is a look every 10 s and 2 s for the look
#   LK_SAID   attempts, the named states by count, the looks without
#   LK_VERDICT  ERROR, with LK_WHY, when the rig did not see the device
#             the whole time: a look without a state, too few attempts,
#             or a wait under 100 s. That says nothing of the kernel or
#             the server. Else FAIL if any look named another state,
#             else PASS.
looks_judge() {
    local w want=$2 secs=$3 named
    LK_ALL=0; LK_NONE=0; LK_OTHER=0; LK_WHY=""
    for w in $1; do
        LK_ALL=$((LK_ALL + 1))
        if [ "$w" = none ]; then LK_NONE=$((LK_NONE + 1))
        elif [ "$w" != "$want" ]; then LK_OTHER=$((LK_OTHER + 1)); fi
    done
    LK_FLOOR=$((secs / 12))
    named=$(tr -s ' ' '\n' <<< "$1" | grep -vx none | grep . | sort | uniq -c | awk '{ printf "%s%s x%d", (NR > 1 ? ", " : ""), $2, $1 }')
    LK_SAID="$LK_ALL looks in $secs s: ${named:-no state named}; $LK_NONE looks named no state"
    if [ "$secs" -lt 100 ]; then
        LK_VERDICT=ERROR; LK_WHY="the wait was $secs s; at least 100 s are needed"
    elif [ "$LK_NONE" -gt 0 ]; then
        LK_VERDICT=ERROR; LK_WHY="$LK_NONE of $LK_ALL looks named no state: the rig did not see the device then"
    elif [ "$LK_ALL" -lt "$LK_FLOOR" ]; then
        LK_VERDICT=ERROR; LK_WHY="only $LK_ALL looks in $secs s; at least $LK_FLOOR are needed (one per 12 s)"
    elif [ "$LK_OTHER" -gt 0 ]; then
        LK_VERDICT=FAIL
    else
        LK_VERDICT=PASS
    fi
}
