#!/usr/bin/env bash
# Review scratch (Q10, PR #148, next look): the `scale` input's two
# checks at HEAD, taken from the files (not copied by hand), on a set of
# values. The workflow's (Record step) refuses before sudo; the script's
# (main) skips a word with "scale: not a count".
#
#   bash review/q10/scale_probe.sh HEAD
set -uo pipefail
HEAD=${1:?head}
REPO=$(git rev-parse --show-toplevel)
wf_case=$(git -C "$REPO" show "$HEAD:.github/workflows/skyzfs-zfs-cli-record.yml" | grep -F '*[!0-9\ ]*)' | sed 's/^ *//')
sh_case=$(git -C "$REPO" show "$HEAD:tests/kernel/zfs-cli-record.sh" | grep -F 'case $n in *[!0-9]* | 0*)' | sed 's/^ *//')
echo "workflow: $wf_case"
echo "script:   $sh_case"
# The workflow's case, as its step runs it; "refused" for exit 64.
eval "wf() { ZCR_SCALE=\$1; case \$ZCR_SCALE in ${wf_case%%)*}) return 64 ;; esac; return 0; }"
for v in '' '1000 10000' '1000  10000 ' '1e9' '1000;id' '$(id)' '1000,2000' $'1000\n2000' $'1000\t2000' '*' '007' '0' \
    '99999999999999999999' '1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20'; do
    if wf "$v"; then w=passes; else w=refused; fi
    words=""
    if [ "$w" = passes ]; then
        # The script's loop, its case as in main.
        for n in ${v:-}; do
            case $n in *[!0-9]* | 0*) words="$words [$n: skipped]"; continue ;; esac
            words="$words [$n: scale]"
        done
    fi
    printf '%-60s workflow %-8s script%s\n' "$(printf '%q' "$v")" "$w" "${words:- (nothing)}"
done
