#!/bin/bash
# Tests of looks.sh: the counting of looks at a waiting device. No kernel.
# Run: bash tests/kernel/test_looks.sh
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=tests/kernel/looks.sh
. "$HERE/looks.sh"
fails=0
is() { # name got want
    if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got '$2', want '$3'"; fails=$((fails + 1)); fi
}
rep() { local out=""; while [ "$(wc -w <<< "$out")" -lt "$2" ]; do out+="$1 "; done; echo "$out"; }

is "a state is its own word" "$(look_word QUIESCED)" QUIESCED
is "nothing is none" "$(look_word "")" none
is "something else is none" "$(look_word "dev id 0: ...")" none

# As the wait builds its list: a look that printed nothing must still
# be an attempt.
states=""; for s in QUIESCED "" QUIESCED; do states+="$(look_word "$s") "; done
looks_judge "$states" QUIESCED 300
is "an empty look is counted as an attempt" "$LK_ALL" 3
is "and as a look without a state" "$LK_NONE" 1

looks_judge "$(rep QUIESCED 30)" QUIESCED 300
is "thirty waiting looks in 300 s pass" "$LK_VERDICT" PASS
is "the floor follows the wait" "$LK_FLOOR" 25
is "the evidence line" "$LK_SAID" "30 looks in 300 s: QUIESCED x30; 0 looks named no state"

# The finding: ten looks that said QUIESCED and twenty that said nothing.
looks_judge "$(rep QUIESCED 10)$(rep none 20)" QUIESCED 300
is "ten named and twenty not is a rig error" "$LK_VERDICT" ERROR
is "all thirty are attempts" "$LK_ALL" 30
is "twenty named no state" "$LK_NONE" 20
is "the evidence shows them" "$LK_SAID" "30 looks in 300 s: QUIESCED x10; 20 looks named no state"

looks_judge "$(rep QUIESCED 29)none " QUIESCED 300
is "one look without a state is a rig error" "$LK_VERDICT" ERROR
looks_judge "$(rep QUIESCED 10)" QUIESCED 300
is "ten looks in 300 s are too few: a rig error" "$LK_VERDICT" ERROR
looks_judge "$(rep QUIESCED 24)" QUIESCED 300
is "one under the floor is a rig error" "$LK_VERDICT" ERROR
looks_judge "$(rep QUIESCED 25)" QUIESCED 300
is "the floor itself passes" "$LK_VERDICT" PASS
looks_judge "$(rep QUIESCED 29)LIVE " QUIESCED 300
is "another state is FAIL" "$LK_VERDICT" FAIL
is "the other state is in the evidence" "$LK_SAID" "30 looks in 300 s: LIVE x1, QUIESCED x29; 0 looks named no state"
looks_judge "$(rep QUIESCED 28)LIVE none " QUIESCED 300
is "a look without a state outranks another state" "$LK_VERDICT" ERROR
looks_judge "$(rep QUIESCED 9)" QUIESCED 90
is "a wait under 100 s is a rig error" "$LK_VERDICT" ERROR
looks_judge "" QUIESCED 300
is "no look at all is a rig error" "$LK_VERDICT" ERROR
is "no look: the evidence" "$LK_SAID" "0 looks in 300 s: no state named; 0 looks named no state"

[ "$fails" = 0 ] || { echo "$fails failed"; exit 1; }
echo "all passed"
