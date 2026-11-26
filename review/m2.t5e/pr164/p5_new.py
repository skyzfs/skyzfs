# Set p5 at 0f25686: plants on this round's new code (agent_quiet's
# verdict, z under lim, the G1 tests' reach, the sweep's rounds).
# plant3.py WORKTREE p5_new.py [IDS]
TR = 'bash tests/kernel/test_agent_record.sh'
TRC = 'bash tests/kernel/test_agent_record.sh && bash tests/kernel/test_cut_record.sh'
TZ = 'bash tests/kernel/test_zfs_cli_record.sh'
A = 'tests/kernel/agent-record.sh'
C = 'tests/kernel/cut-record.sh'
Z = 'tests/kernel/zfs-cli-record.sh'
Q = '''    if [ -n "$SERVE_PID" ] && ! kill -0 "$SERVE_PID" 2>/dev/null; then
        local k=$SERVE_K st=unknown
        if serve_end 5; then st=$SERVE_RC; fi
        SERVE_PID=""
        if [ -n "$CDIR" ] && [ "$SCEN" = dhook ]; then
            log "$NAME: skyzfsd serve $k had ended (status $st) in a dhook case, after the rig's own error"
        else
            agent_daemon_wrong daemon_serving "skyzfsd serve $k had ended (status $st) with no kill of the rig's, seen after the rig's error: $(tail -n 2 "$(serve_log "$k")" 2>/dev/null)"
        fi
    fi
'''
PLANTS = [
 # G5: agent_quiet's verdict.
 ('N01 quiet: no verdict block', A, Q, '', TR),
 ('N02 quiet: exempt in every case', A, '        if [ -n "$CDIR" ] && [ "$SCEN" = dhook ]; then', '        if [ -n "$CDIR" ]; then', TR),
 ('N03 quiet: dhook exempt outside a case', A, '        if [ -n "$CDIR" ] && [ "$SCEN" = dhook ]; then', '        if [ "$SCEN" = dhook ]; then', TR),
 ('N04 quiet: status not read', A, '        if serve_end 5; then st=$SERVE_RC; fi\n        SERVE_PID=""\n        if [ -n "$CDIR" ]', '        SERVE_PID=""\n        if [ -n "$CDIR" ]', TR),
 ('N06 quiet: a live daemon judged too', A, '    if [ -n "$SERVE_PID" ] && ! kill -0 "$SERVE_PID" 2>/dev/null; then\n        local k=$SERVE_K st=unknown', '    if [ -n "$SERVE_PID" ]; then\n        local k=$SERVE_K st=unknown', TR),
 ('N07 quiet: verdict, not the course dir', A, '            agent_daemon_wrong daemon_serving "skyzfsd serve $k had ended (status $st) with no kill of the rig\'s, seen', '            verdict daemon_serving "skyzfsd serve $k had ended (status $st) with no kill of the rig\'s, seen', TR),
 ('N08 quiet: another pin', A, '            agent_daemon_wrong daemon_serving "skyzfsd serve $k had ended (status $st) with no kill of the rig\'s, seen', '            agent_daemon_wrong daemon_stopped "skyzfsd serve $k had ended (status $st) with no kill of the rig\'s, seen', TR),
 ('N09 quiet: recover\'s rule (rig-error.txt)', A, '        if [ -n "$CDIR" ] && [ "$SCEN" = dhook ]; then', '        if [ "$SCEN" = dhook ] && [ -s "$CDIR/rig-error.txt" ]; then', TR),
 ('N10 quiet: no log tail', A, 'seen after the rig\'s error: $(tail -n 2 "$(serve_log "$k")" 2>/dev/null)"', 'seen after the rig\'s error"', TR),
 ('N13 quiet: verdict before session_end', A, '    session_end\n    # A daemon that had ended with no kill of the rig\'s is a verdict, as\n    # in recover (G5 of #163\'s review); in a dhook case the rig\'s own\n    # error about its end comes first.\n' + Q, Q + '    session_end\n', TR),
 ('N15 quiet: serve_end not waited', A, '        if serve_end 5; then st=$SERVE_RC; fi\n        SERVE_PID=""\n        if [ -n "$CDIR" ]', '        if serve_end 0; then st=$SERVE_RC; fi\n        SERVE_PID=""\n        if [ -n "$CDIR" ]', TR),
 # X25/X26 reach: the stub cuts seq, so a huge bound passes.
 ('N17 quiet: the writer\'s bound a million tries', A, '        for _i in $(seq 1 120); do kill -0 "$WRITER" 2>/dev/null || break; sleep 0.5; done\n        kill -9 "$WRITER"', '        for _i in $(seq 1 1200000); do kill -0 "$WRITER" 2>/dev/null || break; sleep 0.5; done\n        kill -9 "$WRITER"', TR),
 ('N17b quiet: the agent\'s bound a million tries', A, '        for _i in $(seq 1 20); do kill -0 "$AGENT_PID"', '        for _i in $(seq 1 2000000); do kill -0 "$AGENT_PID"', TR),
 ('N18 quiet: the writer waited for in a loop', A, '        for _i in $(seq 1 120); do kill -0 "$WRITER" 2>/dev/null || break; sleep 0.5; done\n        kill -9 "$WRITER"', '        while kill -0 "$WRITER" 2>/dev/null; do sleep 0.5; done\n        kill -9 "$WRITER"', TR),
 ('N18b quiet: the agent waited for in a loop', A, '        for _i in $(seq 1 20); do kill -0 "$AGENT_PID" 2>/dev/null || break; sleep 0.25; done', '        while kill -0 "$AGENT_PID" 2>/dev/null; do sleep 0.25; done', TR),
 # G2: z.
 ('N20 z: timeout again', C, '    lim "$secs" "$@" || rig_error "$* failed (status $?)"', '    timeout "$secs" "$@" || rig_error "$* failed (status $?)"', TRC),
 ('N21 z: its failure not an error', C, '    lim "$secs" "$@" || rig_error "$* failed (status $?)"', '    lim "$secs" "$@" || true', TRC),
 ('N21b z: no limit', C, '    lim "$secs" "$@" || rig_error "$* failed (status $?)"', '    "$@" || rig_error "$* failed (status $?)"', TRC),
 # The G1 tests' reach.
 ('N23 recover: an empty rig-error.txt exempt', A, '        if [ "$SCEN" = dhook ] && [ -s "$CDIR/rig-error.txt" ]; then', '        if [ "$SCEN" = dhook ] && [ -e "$CDIR/rig-error.txt" ]; then', TR),
 ('N24 late: not served again', A, '    akill_serve\n    serve_again\n    mark "continued-', '    akill_serve\n    mark "continued-', TR),
 ('N25 late: the agent left stopped', A, '    mark "continued-$(basename "$CDIR")"\n    kill -CONT "$AGENT_PID"\n', '    mark "continued-$(basename "$CDIR")"\n', TR),
 ('N26 end_daemon: SIGTERM twice', C, '        kill -9 "$SERVE_PID" 2>/dev/null || true\n        serve_end 10 || rig_error "skyzfsd serve did not end 10 s after SIGKILL"', '        kill -TERM "$SERVE_PID" 2>/dev/null || true\n        serve_end 10 || rig_error "skyzfsd serve did not end 10 s after SIGKILL"', TRC),
 # The sweep's rounds.
 ('R01 rounds: the idle of no length', Z, '                writing "$c" "$wdir" "$p" idle_for "$secs"\n                writing "$r"', '                writing "$c" "$wdir" "$p" idle_for 0\n                writing "$r"', TZ),
 ('R04 rounds: the reads\' stalls from the controls', Z, '            reads+=("$(stall_of "$r")")', '            reads+=("$(stall_of "$c")")', TZ),
 ('R05 rounds: the medians swapped', Z, 'read longest ${reads[*]:-none} (median $(median "${reads[@]}")); idle longest ${idles[*]:-none} (median $(median "${idles[@]}"))', 'read longest ${reads[*]:-none} (median $(median "${idles[@]}")); idle longest ${idles[*]:-none} (median $(median "${reads[@]}"))', TZ),
 # #163's X24, whose anchor this round's block moved: no session_end.
 ('X24h quiet: no session_end', A, "    session_end\n    # A daemon that had ended with no kill of the rig's is a verdict, as\n", "    # A daemon that had ended with no kill of the rig's is a verdict, as\n", TR),
]
