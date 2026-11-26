F = 'tests/kernel/agent-record.sh'
T = 'bash tests/kernel/test_agent_record.sh'
C = 'tests/kernel/cut-record.sh'
TC = 'bash tests/kernel/test_agent_record.sh && bash tests/kernel/test_cut_record.sh'
# Set C's plants that no longer apply, adapted; then new plants on this
# head's recorder (D5, G6, G1's shell side, G8) and cut-record.sh's hook.
P = [
 ('S13b rare: first three reaches in the shell again', F, '        for p in $pts; do acase "K-$s-${p//:/-}" "$s" "$p"; done', '        for p in $pts; do [ "${p##*:}" -le 3 ] || continue; acase "K-$s-${p//:/-}" "$s" "$p"; done', T),
 ('S20a await_agent: no verdict', F, '''        verdict "$1_ended" "the agent's $1 had not ended $2 s on: $(tail -n 2 "$CDIR/$1.err")"''', '        :', T),
 ('S21a vrun: the run dir itself', F, '''vrun() { printf '%s/v-%x-%x' "$RUN" "$(stat -c %d "$VOL")" "$(stat -c %i "$VOL")"; }''', '''vrun() { printf '%s' "$RUN"; }''', T),
 ('U01 verdict: tabs kept', F, '''    printf '%s\\t%s\\n' "$1" "$(printf '%s' "$2" | tr '\\t\\n' '  ')" >> "$CDIR/verdicts.tsv"''', '''    printf '%s\\t%s\\n' "$1" "$2" >> "$CDIR/verdicts.tsv"''', T),
 ('U02 case_error overwrites', F, '''"$(printf '%s' "$*" | tr '\\t\\n' '  ')" >> "$CDIR/rig-error.txt"''', '''"$(printf '%s' "$*" | tr '\\t\\n' '  ')" > "$CDIR/rig-error.txt"''', T),
 ('U03 await_agent: no timeout status', F, '        echo timeout > "$CDIR/$1.rc"\n', '', T),
 ('U04 await_agent: pid kept', F, '        kill -9 "$AGENT_PID" 2>/dev/null; wait "$AGENT_PID" 2>/dev/null || true; AGENT_PID=""\n        verdict', '        kill -9 "$AGENT_PID" 2>/dev/null; wait "$AGENT_PID" 2>/dev/null || true\n        verdict', T),
 ('U05 recover: no SIGCONT', F, '        kill -CONT "$AGENT_PID" 2>/dev/null || true\n        kill -9 "$AGENT_PID" 2>/dev/null || true', '        kill -9 "$AGENT_PID" 2>/dev/null || true', T),
 ('U06 recover: store not thawed', F, '''    if [ -n "$S3_IS_DOWN" ]; then\n        thaw\n''', '''    if [ -n "$S3_IS_DOWN" ]; then\n''', T),
 ('U07 recover: verdict even after the rig error', F, '        if [ -s "$CDIR/rig-error.txt" ]; then', '        if false; then', T),
 ('U08 recover: daemon not served again', F, '''        echo "the daemon served again" >> "$CDIR/recovered.txt"\n        serve_again\n''', '''        echo "the daemon served again" >> "$CDIR/recovered.txt"\n''', T),
 ('U09 recover: status never read', F, '        if [ -n "$SERVE_PID" ] && serve_end 5; then st=$SERVE_RC; fi', '        :', T),
 ('U10 reserve: non-zero stop not a verdict', F, '    elif [ "$SERVE_RC" != 0 ]; then', '    elif false; then', T),
 ('U11 reserve: hung stop not killed', F, '        kill -9 "$SERVE_PID" 2>/dev/null || true\n        serve_end 10', '        serve_end 10', T),
 ('U12 reserve: hung stop not a verdict', F, '        verdict daemon_stopped "skyzfsd serve $SERVE_K did not end 60 s after SIGTERM"\n', '', T),
 ('U13 acase: recover not called', F, '    "scen_$SCEN" "${4:-}" "${hooks[@]}"\n    recover\n', '    "scen_$SCEN" "${4:-}" "${hooks[@]}"\n', T),
 ('U14 destroy: goes on after its error', F, '            mv "$CDIR/destroy.log" "$CDIR/destroy-failed.log"\n            return 0\n        fi\n        mark "destroyed-$id"', '            mv "$CDIR/destroy.log" "$CDIR/destroy-failed.log"\n        fi\n        mark "destroyed-$id"', T),
 ('U15 destroy: log kept on error', F, '            mv "$CDIR/destroy.log" "$CDIR/destroy-failed.log"\n            return 0', '            return 0', T),
 ('U16 gone: log kept on error', F, '        mv "$CDIR/destroy.log" "$CDIR/destroy-failed.log"\n    fi\n}\nscen_unmarked', '    fi\n}\nscen_unmarked', T),
 ('U17 frozen: no verdict, settle', F, '''        verdict withdrew "the agent ran on and did not withdraw in 120 s"\n        return 0\n''', '', T),
 ('U18 dhook: no asked verdict', F, '        wait_point asked after 60 || verdict asked "the agent did not reach asked:after in 60 s"', '        wait_point asked after 60 || true', T),
 ('U19 late: no asked verdict, goes on', F, '''    wait_point asked after 60 || { verdict asked "the agent did not reach asked:after in 60 s"; return 0; }''', '    wait_point asked after 60 || true', T),
 ('U20 late: no landed verdict', F, '''    snaprow "$t" landed.txt || { verdict landed "snap/$t had not landed 120 s after the thaw"; return 0; }''', '''    snaprow "$t" landed.txt || true''', T),
 ('U21 dhook: lost pid not an error', F, '''        serve_end 5 || { case_error "the daemon's pid went away and serve_end did not see it end"; return 0; }''', '        serve_end 5 || true', T),
 ('U22 agent_begin: no hook', F, '    ON_RIG_ERROR=agent_judged\n', '', T),
 ('U23 agent_judged: status lost', F, '''    $CHECK "$NAME" "$RDIR" "$RDIR/requests.tsv" - $((VDEV_MB << 20)) || rc=$?''', '''    $CHECK "$NAME" "$RDIR" "$RDIR/requests.tsv" - $((VDEV_MB << 20)) || true''', T),
 ('U24 agent_judged: no case in the line', F, '''    printf '%s\\t%s\\n' "$(basename "${CDIR:-none}")" "$1" > "$RDIR/rig-error.tsv"''', '''    printf '%s\\n' "$1" > "$RDIR/rig-error.tsv"''', T),
 ('U25 agent_judged: no checked line', F, '''    printf '%s\\tchecked\\texit %s\\n' "$NAME" "$rc" >> "$OUT/recordings.tsv"\n    return "$rc"''', '''    return "$rc"''', T),
 ('U26 agent_judged: none is 0', F, '    [ "$CHECK" != none ] || return 2', '    [ "$CHECK" != none ] || return 0', T),
 ('U27 aplain: no points-error line', F, '''        || { printf 'trace\\t%s\\n' "$(tr '\\t\\n' '  ' < "$RDIR/points.err")" >> "$RDIR/points-error.tsv"; points=""; }''', '''        || points=""''', T),
 ('U28 arare: no points-error line', F, '''            || { printf '%s\\t%s\\n' "$s" "$(tr '\\t\\n' '  ' < "$RDIR/points.err")" >> "$RDIR/points-error.tsv"; pts=""; }''', '''            || pts=""''', T),
 ('U29 agent_begin: lists of errors kept', F, '    rm -f "$RDIR/points-error.tsv" "$RDIR/rig-error.tsv"\n', '', T),
 ('U30 serve_log: always serve-K', F, '''serve_log() { if [ "$1" = 1 ]; then echo "$RDIR/serve.log"; else echo "$RDIR/serve-$1.log"; fi; }''', '''serve_log() { echo "$RDIR/serve-$1.log"; }''', T),
 # cut-record.sh's hook
 ('V01 rig_error: hook 0 passes', C, '        [ "$rc" = 1 ] || rc=2\n', '', TC),
 ('V02 rig_error: hook not cleared', C, '        ON_RIG_ERROR=""\n        "$h" "$m" || rc=$?', '        "$h" "$m" || rc=$?', TC),
 ('V03 rig_error: hook not called', C, '        "$h" "$m" || rc=$?', '        :', TC),
 ('V04 rig_error: no line', C, '''    echo "$NAME	rig error	$m" >> "$OUT/recordings.tsv"\n    if''', '''    if''', TC),
 ('V05 rig_error: hook any non-zero kept', C, '        [ "$rc" = 1 ] || rc=2\n', '        [ "$rc" != 0 ] || rc=2\n', TC),
]
PLANTS = P
