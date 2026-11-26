# Plants on #169 at 54b666b, one at a time: plant4.py WORKTREE p6.py [IDS]
TA = 'bash tests/kernel/test_agent_record.sh'
TC = 'bash tests/kernel/test_agent_record.sh && bash tests/kernel/test_cut_record.sh'
TZ = 'bash tests/kernel/test_zfs_cli_record.sh'
A = 'tests/kernel/agent-record.sh'
C = 'tests/kernel/cut-record.sh'
Z = 'tests/kernel/zfs-cli-record.sh'
PLANTS = [
 # The settle (R1).
 ('S01 settle: after the margin, not before', Z,
  '    settle_txgs "$log.txg.tsv"\n    sleep "$margin"\n', '    sleep "$margin"\n    settle_txgs "$log.txg.tsv"\n', TZ),
 ('S02 settle: not called', Z, '    settle_txgs "$log.txg.tsv"\n', '', TZ),
 ('S03 settle: groups of the kstat history counted', Z, '$1 == "txg" && s != "" && $3 > s { c++ }', '$1 == "txg" { c++ }', TZ),
 ('S04 settle: ticks counted as groups', Z, '$1 == "txg" && s != "" && $3 > s { c++ }', '($1 == "txg" || $1 == "tick") && s != "" && $2 > s { c++ }', TZ),
 ('S05 settle: default one group', Z, 'n=${ZCR_SETTLE_TXGS:-2}', 'n=${ZCR_SETTLE_TXGS:-1}', TZ),
 ('S06 settle: limit in seconds read as tenths', Z, 'for ((i = 0; i <= 10#$limit * 10; i++)); do', 'for ((i = 0; i <= 10#$limit; i++)); do', TZ),
 ('S07 settle: settled file not written', Z, '    printf \'%s\\n\' "$SETTLED" > "$OUT/$name/settled"\n', '', TZ),
 ('S08 settle: about line not written', Z, '        echo "scale: settle beside $name: $SETTLED"\n', '', TZ),
 ('S09 settle: settings line not said', Z, '    [ "$#" = 0 ] || echo "scale settings:', '    false && echo "scale settings:', TZ),
 # The rounds (R1 to R4).
 ('W01 reading_for: overruns the window', Z, "'BEGIN { exit !(now - t0 + l <= s) }'", "'BEGIN { exit !(now - t0 <= s) }'", TZ),
 ('W02 reading_for: one run only', Z, "'BEGIN { exit !(now - t0 + l <= s) }' || break", "'BEGIN { exit 1 }' || break", TZ),
 ('W03 rounds: idle the step read, odd', Z, '                writing "$c" "$wdir" "$p" idle_for "$w"\n                writing "$r"', '                writing "$c" "$wdir" "$p" idle_for "$secs"\n                writing "$r"', TZ),
 ('W04 rounds: no rounding up', Z, 'if (c < s) c++; ', '', TZ),
 ('W05 rounds: window ignores the step', Z, "print (c > w) ? c : w }')", "print w }')", TZ),
 ('W06 rounds: cut read still run', Z, '        if [ "$(cat "$OUT/scale-$lbl-sweep-$kind/rc" 2> /dev/null)" = 124 ]; then', '        if false; then', TZ),
 ('W07 rounds: budget check removed', Z, '            if [ "$left" -lt "$need" ]; then', '            if false; then', TZ),
 ('W08 rounds: need without the settle', Z, ' + ${ZCR_SETTLE_S:-30} + 2)))', ' + 2)))', TZ),
 ('W09 rounds: limit against one window', Z, '            if [ "$LIMIT" -lt $((2 * w)) ]; then', '            if [ "$LIMIT" -lt "$w" ]; then', TZ),
 ('W10 rounds: octal rounds', Z, '    rounds=$((10#$rounds))\n', '', TZ),
 ('W11 rounds: groups read from the control', Z, 'rt+=("$(field_of "$r" txg txgs)")', 'rt+=("$(field_of "$c" txg txgs)")', TZ),
 ('W12 rounds: longest_sync for sync_longest', Z, 'rs+=("$(field_of "$r" txg sync_longest)")', 'rs+=("$(field_of "$r" txg longest_sync)")', TZ),
 ('W13 rounds: reads stalls from the control', Z, 'rl+=("$(stall_of "$r")")', 'rl+=("$(stall_of "$c")")', TZ),
 ('W14 median: > not stripped', Z, "sed 's/^[<>]//'", "sed 's/^<//'", TZ),
 # #165's records.
 ('K01 counts: first name, not the middle', Z, 'gone=${names[$((${#names[@]} / 2))]}', 'gone=${names[0]}', TZ),
 ('K02 counts: names not made unique', Z, "$1 ~ /@/ && !seen[$1]++ { print $1 }", "$1 ~ /@/ { print $1 }", TZ),
 ('K03 counts: destroyed without its release', Z, '        rec "$x-counts-release-one" zfs release "$TAG" "$gone"\n', '', TZ),
 ('K04 counts: @c not let go before its destroy', Z, '    release_quietly "$p" c\n', '', TZ),
 ('K05 odd tags: a space for the tab', Z, "TAB_TAG=$'other\\tholder'", "TAB_TAG='other holder'", TZ),
 ('K06 odd tags: nl not released', Z, '    rec "$x-release-nl" zfs release "$NL_TAG" "$p@t"\n', '', TZ),
 # G-a, G-b and the nits.
 ('G01 recover: kill.tsv ignored', A, '[ -s "$CDIR/rig-error.txt" ] && [ ! -e "$CDIR/kill.tsv" ]; then', '[ -s "$CDIR/rig-error.txt" ]; then', TA),
 ('G02 quiet: kill.tsv ignored', A, '        if [ -n "$CDIR" ] && [ "$SCEN" = dhook ] && [ ! -e "$CDIR/kill.tsv" ]; then', '        if [ -n "$CDIR" ] && [ "$SCEN" = dhook ]; then', TA),
 ('G03 quiet: kill.tsv inverted', A, '        if [ -n "$CDIR" ] && [ "$SCEN" = dhook ] && [ ! -e "$CDIR/kill.tsv" ]; then', '        if [ -n "$CDIR" ] && [ "$SCEN" = dhook ] && [ -e "$CDIR/kill.tsv" ]; then', TA),
 ('G04 reserve: back to || true', A, '    if ! kill -TERM "$SERVE_PID" 2>/dev/null; then\n        local st=unknown\n        if serve_end 5; then st=$SERVE_RC; fi\n        verdict daemon_stopped "skyzfsd serve $SERVE_K had ended before SIGTERM (status $st): $(tail -n 2 "$(serve_log "$SERVE_K")")"\n    elif ! serve_end 60; then', '    kill -TERM "$SERVE_PID" 2>/dev/null || true\n    if ! serve_end 60; then', TA),
 ('G05 reserve: no serve again after it', A, '        verdict daemon_stopped "skyzfsd serve $SERVE_K had ended before SIGTERM (status $st): $(tail -n 2 "$(serve_log "$SERVE_K")")"\n', '        verdict daemon_stopped "skyzfsd serve $SERVE_K had ended before SIGTERM (status $st): $(tail -n 2 "$(serve_log "$SERVE_K")")"\n        return 0\n', TA),
 ('G06 reserve: status not read', A, '        if serve_end 5; then st=$SERVE_RC; fi\n        verdict daemon_stopped "skyzfsd serve $SERVE_K had ended before', '        verdict daemon_stopped "skyzfsd serve $SERVE_K had ended before', TA),
 ('G07 end_daemon: back to || true', C, '    if ! kill -TERM "$SERVE_PID" 2>/dev/null; then\n        local st=unknown\n        if serve_end 5; then st=$SERVE_RC; fi\n        daemon_wrong daemon_stopped "skyzfsd serve had ended before SIGTERM (status $st): $(tail -n 2 "$SERVE_LOG")"\n    elif ! serve_end 60; then', '    kill -TERM "$SERVE_PID" 2>/dev/null || true\n    if ! serve_end 60; then', TC),
 ('G08 end_daemon: no detach after the verdict', C, '        daemon_wrong daemon_stopped "skyzfsd serve had ended before SIGTERM (status $st): $(tail -n 2 "$SERVE_LOG")"\n', '        daemon_wrong daemon_stopped "skyzfsd serve had ended before SIGTERM (status $st): $(tail -n 2 "$SERVE_LOG")"\n        return 0\n', TC),
 ('N01 cleanup: detach only with a daemon', C, '    if [ "${SERVER:-}" = daemon ] && [ -n "${VOL:-}" ] && [ -d "$VOL" ]; then', '    if [ -n "$SERVE_PID" ] && [ "${SERVER:-}" = daemon ] && [ -n "${VOL:-}" ] && [ -d "$VOL" ]; then', TC),
 ('N02 cleanup: detach under timeout', C, '        lim 60 "$SKYZFSD" detach --dir "$VOL" --run-dir "$RUN" > /dev/null 2>&1\n    fi\n    dmsetup remove', '        timeout 60 "$SKYZFSD" detach --dir "$VOL" --run-dir "$RUN" > /dev/null 2>&1\n    fi\n    dmsetup remove', TC),
 ('N03 cleanup: no volume check', C, '    if [ "${SERVER:-}" = daemon ] && [ -n "${VOL:-}" ] && [ -d "$VOL" ]; then', '    if [ "${SERVER:-}" = daemon ] && [ -n "${VOL:-}" ]; then', TC),
 ('N04 cleanup: not only through the daemon', C, '    if [ "${SERVER:-}" = daemon ] && [ -n "${VOL:-}" ] && [ -d "$VOL" ]; then', '    if [ -n "${VOL:-}" ] && [ -d "$VOL" ]; then', TC),
 ('N05 quiet: agent bound 2000', A, '        for _i in $(seq 1 20); do kill -0 "$AGENT_PID" 2>/dev/null || break; sleep 0.25; done\n        kill -0 "$AGENT_PID" 2>/dev/null || wait "$AGENT_PID"', '        for _i in $(seq 1 2000); do kill -0 "$AGENT_PID" 2>/dev/null || break; sleep 0.25; done\n        kill -0 "$AGENT_PID" 2>/dev/null || wait "$AGENT_PID"', TA),
 ('N06 quiet: writer bound 1200', A, '        for _i in $(seq 1 120); do kill -0 "$WRITER" 2>/dev/null || break; sleep 0.5; done\n        kill -9 "$WRITER" 2>/dev/null || true', '        for _i in $(seq 1 1200); do kill -0 "$WRITER" 2>/dev/null || break; sleep 0.5; done\n        kill -9 "$WRITER" 2>/dev/null || true', TA),
]
