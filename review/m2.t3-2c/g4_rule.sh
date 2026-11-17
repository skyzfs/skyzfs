#!/bin/bash
# #113 at 98024d5: daemon-rig.sh g.4's rule for the serve after the
# refused start, taken from the script as it is (the `if` that follows
# `gj_serves "on data=ordered again"`) and run on the cases by hand:
# what the refused start changed in the volume's directory, and the
# cut_state the serve after reports. No kernel: only the rule's text.
#
#   g4_rule.sh CHECKOUT/tests/kernel/daemon-rig.sh
src=$1
rule=$(awk '/gj_serves "on data=ordered again"/ { on = 1; next }
            on && /^    if \[ \$\{#hold\[@\]\} != 0 \] && \[ -n "\$st" \]; then$/ { take = 1 }
            take { print }
            take && /^    fi$/ { exit }' "$src")
[ -n "$rule" ] || { echo "the rule was not found in $src"; exit 2; }
eval "$(grep -m 1 '^GJ_START_TOKEN=' "$src")"
try() { # what-for changed gone st
    local changed=$2 gone=$3 st=$4 bad="" hold=(--hold-cuts)
    eval "$rule"
    printf '%-58s cut_state %-9s -> %s\n' "$1" "${st:-(not served)}" "${bad:-accepted}"
}
try "the token rewritten (as today's daemon)" "status volume.start " "status volume.start " begun
try "the token rewritten" "status volume.start " "status volume.start " taken_up
try "the token rewritten, nothing else" "volume.start " "volume.start " taken_up
try "the token left as it was" "status " "status " taken_up
try "the token left as it was" "status " "status " begun
try "the token left as it was" "" "" off
try "the token left as it was, no cut_state in the status" "" "" none
try "the token removed" "" "volume.start " taken_up
try "the token rewritten, the serve after did not serve" "volume.start " "volume.start " ""
