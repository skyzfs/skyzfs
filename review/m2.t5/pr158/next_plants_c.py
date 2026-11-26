F = 'tests/kernel/agent-record.sh'
T = 'bash tests/kernel/test_agent_record.sh'
# The shell plants that survived the review of 0604977 (S12 and S19 are
# gone with the code they planted), redone at c580c72, then new ones on
# the fixes.
P = [
 ('S03 case_name empty name', 'NF >= 4 && $4 != "" && $2 != "sweep"', 'NF >= 4 && $2 != "sweep"'),
 ('S05 point_reached effect only', "'$2 == e && $3 == s { f = 1 }", "'$2 == e { f = 1 }"),
 ('S13 rare first three', '[ "${p##*:}" -le 3 ] || continue', '[ "${p##*:}" -le 1 ] || continue'),
 ('S14 retry always', '    if [ -n "$t" ] && snaprow "$t" snaprow.txt; then', '    if false; then'),
 ('S15 retry never', '        acmd retry publish --limit-s 600\n', ''),
 ('S16 dhook no freeze', '        freeze\n        sleep 3\n        abg publish publish --limit-s 600 "$@"', '        abg publish publish --limit-s 600 "$@"'),
 ('S17 holds not read before release', '''    zfs list -H -t snapshot -o name -r "$POOL" | xargs -r zfs holds -H > "$CDIR/holds.txt" 2> "$CDIR/holds.err" \\\n        || rig_error "zfs holds failed: $(tail -n 2 "$CDIR/holds.err")"\n''', '    : > "$CDIR/holds.txt"\n'),
 ('S18 destroy keeps hold', '{ zfs release "$HOLD_TAG" "$POOL@$t" && zfs destroy "$POOL@$t"; }', '{ zfs destroy "$POOL@$t"; }'),
 ('S20 await_agent no rig error', '''        rig_error "the agent's $1 in case $(basename "$CDIR") had not ended $2 s on: $(tail -n 2 "$CDIR/$1.err")"''', '        :'),
 ('S21 arun plain run', '''arun() { printf '%s/v-%x-%x' "$RUN" "$(stat -c %d "$VOL")" "$(stat -c %i "$VOL")"; }''', '''arun() { printf '%s' "$RUN"; }'''),
 ('S22 settle without age', '    acmd settle settle --limit-s 600 "${CASE_AGE[@]}"', '    acmd settle settle --limit-s 600'),
 ('S23 cases.tsv kill dash', 'printf \'%s\\t%s\\t%s\\t%s\\t%s\\n\' "$(basename "$CDIR")" "$SCEN" "${KILL:--}"', 'printf \'%s\\t%s\\t%s\\t%s\\t%s\\n\' "$(basename "$CDIR")" "$SCEN" "-"'),
 # D1
 ('T01 published: no gone path (old code)', '''    if ! grep -qxF "$POOL@$n" "$CDIR/snapshots.txt"; then''', '''    if false; then'''),
 ('T02 published: substring match', '''grep -qxF "$POOL@$n" "$CDIR/snapshots.txt"''', '''grep -qF "$n" "$CDIR/snapshots.txt"'''),
 ('T03 published: gone line not written', '''        printf '%s\\t%s\\n' "$(basename "$CDIR")" "$n" >> "$RDIR/published-gone.tsv"\n''', ''),
 ('T04 published: a list that fails goes on', '''        || rig_error "zfs list of $POOL's snapshots failed: $(tail -n 2 "$CDIR/snapshots.err")"''', '''        || true'''),
 ('T05 published: no per-case early return', '''    grep -qxF "$(printf '%s\\t%s' "$(basename "$CDIR")" "$n")" "$RDIR/published-gone.tsv" 2>/dev/null && return 0\n''', ''),
 ('T06 published: snapshot of a descendant counts', '''grep -qxF "$POOL@$n" "$CDIR/snapshots.txt"''', '''grep -qE "@$n\\$" "$CDIR/snapshots.txt"'''),
 ('T07 agent_begin: gone file not reset', '''; : > "$RDIR/published-gone.tsv"\n''', '\n'),
 # D3
 ('T10 adump: status lost', '''    "$AGENT" record --dir "$ADIR" > "$CDIR/$1.txt" 2> "$CDIR/$1.err" || rc=$?''', '''    "$AGENT" record --dir "$ADIR" > "$CDIR/$1.txt" 2> "$CDIR/$1.err" || true'''),
 ('T11 adump: stderr into the dump', '''    "$AGENT" record --dir "$ADIR" > "$CDIR/$1.txt" 2> "$CDIR/$1.err" || rc=$?''', '''    "$AGENT" record --dir "$ADIR" > "$CDIR/$1.txt" 2>&1 || rc=$?'''),
 ('T12 record-after not dumped', '''    adump record-after\n''', ''),
 # D4
 ('T20 sweep: first publish not made', '''    acmd first publish --limit-s 600\n''', ''),
 ('T21 aplain: traced status a rig error again', '''    acase trace plain\n    i=''', '''    acase trace plain\n    [ "$(cat "$RDIR/cases/trace/publish.rc")" = 0 ] || rig_error "the traced publish ended $(cat "$RDIR/cases/trace/publish.rc")"\n    i='''),
 # G2
 ('T30 both: status not kept', '''    wait "$SERVE_PID" 2>/dev/null || rc=$?\n    SERVE_PID=""\n    printf 'killed_rc\\t%s\\n' "$rc"''', '''    wait "$SERVE_PID" 2>/dev/null || true\n    SERVE_PID=""\n    printf 'killed_rc\\t%s\\n' 137'''),
 ('T31 both: daemon not killed, status 137 written', '''    kill -9 "$SERVE_PID" 2>/dev/null || true\n    wait "$SERVE_PID" 2>/dev/null || rc=$?''', '''    rc=137'''),
]
PLANTS = [(pid, F, old, new, T) for pid, old, new in P]
