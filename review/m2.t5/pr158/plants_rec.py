F = 'tests/kernel/agent-record.sh'
T = 'bash tests/kernel/test_agent_record.sh'
P = [
 ('S01 shard formula', '[ $(((k - 1) % n)) = $((i - 1)) ] || continue', '[ $((k % n)) = $((i - 1)) ] || continue'),
 ('S02 case_name sweep', '$4 != "" && $2 != "sweep" { t = $4 }', '$4 != "" { t = $4 }'),
 ('S03 case_name empty name', 'NF >= 4 && $4 != "" && $2 != "sweep"', 'NF >= 4 && $2 != "sweep"'),
 ('S04 wait_point no early end', '        kill -0 "$AGENT_PID" 2>/dev/null || { point_reached "$1" "$2"; return; }\n', ''),
 ('S05 point_reached effect only', "'$2 == e && $3 == s { f = 1 }", "'$2 == e { f = 1 }"),
 ('S06 shard regex 0', '^([1-9][0-9]*)/([1-9][0-9]*)$', '^([0-9]+)/([1-9][0-9]*)$'),
 ('S07 K<=N removed', ' && [ "${BASH_REMATCH[1]}" -le "${BASH_REMATCH[2]}" ]', ''),
 ('S08 points validation', '[[ $p =~ ^[a-z]+:(before|after):[1-9][0-9]*$ ]] ||', '[[ $p =~ ^[a-z]+:[a-z]+:[0-9]+$ ]] ||'),
 ('S09 S3 check removed', '    [ -n "$S3_DOWN" ] && [ -n "$S3_UP" ] || { echo "$r needs CUT_S3_DOWN and CUT_S3_UP" >&2; exit 64; }\n', ''),
 ('S10 agent check removed', '''    [ -x "$AGENT" ] || { echo "$r needs CUT_AGENT, the agent's binary built with its test hooks" >&2; exit 64; }\n''', ''),
 ('S11 AGENT_POINTS ignored', 'points=${AGENT_POINTS:-$(acheck points "$RDIR/cases/trace/trace.tsv")}', 'points=$(acheck points "$RDIR/cases/trace/trace.tsv")'),
 ('S12 trace rc not checked', '''    [ "$(cat "$RDIR/cases/trace/publish.rc")" = 0 ] || rig_error "the traced publish ended $(cat "$RDIR/cases/trace/publish.rc")"\n''', ''),
 ('S13 rare first three', '[ "${p##*:}" -le 3 ] || continue', '[ "${p##*:}" -le 1 ] || continue'),
 ('S14 retry always', '    if [ -n "$t" ] && snaprow "$t" snaprow.txt; then', '    if false; then'),
 ('S15 retry never', '        acmd retry publish --limit-s 600\n', ''),
 ('S16 dhook no freeze', '        freeze\n        sleep 3\n        abg publish publish --limit-s 600 "$@"', '        abg publish publish --limit-s 600 "$@"'),
 ('S17 holds not read before release', '''    zfs list -H -t snapshot -o name -r "$POOL" | xargs -r zfs holds -H > "$CDIR/holds.txt" 2> "$CDIR/holds.err" \\\n        || rig_error "zfs holds failed: $(tail -n 2 "$CDIR/holds.err")"\n''', '    : > "$CDIR/holds.txt"\n'),
 ('S18 destroy keeps hold', '{ zfs release "$HOLD_TAG" "$POOL@$t" && zfs destroy "$POOL@$t"; }', '{ zfs destroy "$POOL@$t"; }'),
 ('S19 both no daemon kill', '    await_agent publish 300\n    kill_serve\n    serve_again\n}', '    await_agent publish 300\n}'),
 ('S20 await_agent no rig error', '''        rig_error "the agent's $1 in case $(basename "$CDIR") had not ended $2 s on: $(tail -n 2 "$CDIR/$1.err")"''', '        :'),
 ('S21 arun plain run', '''arun() { printf '%s/v-%x-%x' "$RUN" "$(stat -c %d "$VOL")" "$(stat -c %i "$VOL")"; }''', '''arun() { printf '%s' "$RUN"; }'''),
 ('S22 settle without age', '    acmd settle settle --limit-s 600 "${CASE_AGE[@]}"', '    acmd settle settle --limit-s 600'),
 ('S23 cases.tsv kill dash', 'printf \'%s\\t%s\\t%s\\t%s\\t%s\\n\' "$(basename "$CDIR")" "$SCEN" "${KILL:--}"', 'printf \'%s\\t%s\\t%s\\t%s\\t%s\\n\' "$(basename "$CDIR")" "$SCEN" "-"'),
]
PLANTS = [(pid, F, old, new, T) for pid, old, new in P]
