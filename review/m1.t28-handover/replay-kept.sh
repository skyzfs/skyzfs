#!/bin/bash
# Replay every volume of the kept M1.T28 traces (run 43476467345) with the tool
# at 8e8cb23 (merged with #60) and at 0a78798 (pull request 66), and compare
# the two outputs byte for byte.
#   replay-kept.sh ART_DIR OLD_TOOL NEW_TOOL
set -u
ART=$1; OLD=$2; NEW=$3
for gz in "$ART"/*/c-trace.tsv.gz; do
    pass=$(basename "$(dirname "$gz")")
    d=$(mktemp -d)
    gzip -dc "$gz" > "$d/c-trace.tsv"
    words=$(cut -f3 "$d/c-trace.tsv" | grep -c '^before_kill$')
    new_words=$(cut -f3 "$d/c-trace.tsv" | grep -c '^answered_before_kill$')
    vols=0; differ=0; wrong_old=0; wrong_new=0
    for v in $(cut -f2 "$d/c-trace.tsv" | sort -un); do
        vols=$((vols + 1))
        "$OLD" --replay "$d/c-trace.tsv" "$v" > "$d/old.txt" 2>&1; ro=$?
        "$NEW" --replay "$d/c-trace.tsv" "$v" > "$d/new.txt" 2>&1; rn=$?
        [ "$ro" = 0 ] || wrong_old=$((wrong_old + 1))
        [ "$rn" = 0 ] || wrong_new=$((wrong_new + 1))
        { [ "$ro" = "$rn" ] && cmp -s "$d/old.txt" "$d/new.txt"; } || differ=$((differ + 1))
    done
    echo "$pass: $vols volumes, $words before_kill lines, $new_words answered_before_kill lines; not exit 0: old tool $wrong_old, new tool $wrong_new; volumes whose output differs: $differ"
    rm -rf "${d:?}"
done
