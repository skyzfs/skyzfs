#!/usr/bin/env bash
set -euo pipefail
OUT=$1; RECS="$2"; WORST=0
: > "$OUT/recordings.tsv"
log() { printf '%s\n' "$*" >&2; }
cleanup() { local rc=$?; set +e; exit "$rc"; }
rig_error() { echo "$NAME	rig error	$*" >> "$OUT/recordings.tsv"; exit 2; }
begin() { NAME=$1; }
end() { echo "$NAME	checked	exit $VERDICT" >> "$OUT/recordings.tsv"; WORST=$(( VERDICT > WORST ? VERDICT : WORST )); }
rec_fails() { false; echo "went on after a failed command" >&2; VERDICT=0; }
rec_rigerr() { rig_error "said so"; }
rec_verdict() { VERDICT=1; }
rec_ok() { VERDICT=0; }
rec_pipe() { false | cat; echo "went on after a failed pipeline" >&2; VERDICT=0; }
for r in $RECS; do
    set +e
    (
        set -e
        trap cleanup EXIT
        if [ "$r" = stray ]; then begin "$r" delay-end; else begin "$r"; fi
        "rec_$r"
        end
        exit "$WORST"
    )
    rc=$?
    set -e
    # A command that failed outside z and rig_error stopped the recording
    # with its own status and no line: that is the rig's error, not a
    # verdict.
    if [ "$rc" -ne 0 ] && ! awk -F '\t' -v r="$r" '$1 == r && ($2 == "rig error" || $2 == "checked") { f = 1 } END { exit !f }' "$OUT/recordings.tsv"; then
        log "the rig's own error in $r: a command failed (status $rc); the log above names it"
        printf '%s\trig error\ta command failed (status %s); the job log names it\n' "$r" "$rc" >> "$OUT/recordings.tsv"
        rc=2
    fi
    WORST=$(( rc > WORST ? rc : WORST ))
done
echo "worst $WORST"
