# Set new at 4f0d632: plants on the three commits after fafc771 (1eaa7ca,
# 7399124, 4f0d632), my own, in the spirit of the description's W, X and
# Y (not their text). plant2.py WORKTREE p4_new.py
TR = 'bash tests/kernel/test_agent_record.sh'
TRC = 'bash tests/kernel/test_agent_record.sh && bash tests/kernel/test_cut_record.sh'
TC = 'bash tests/kernel/test_zfs_cli_record.sh'
A = 'tests/kernel/agent-record.sh'
C = 'tests/kernel/cut-record.sh'
Z = 'tests/kernel/zfs-cli-record.sh'
PLANTS = [
 # 7399124: serve_start's files before the fork.
 ('V1 the ready file not made', C, '    : > "$VOL.ready"\n    : > "$slog"\n', '    : > "$slog"\n', TRC),
 ('V2 the log not made', C, '    : > "$VOL.ready"\n    : > "$slog"\n', '    : > "$VOL.ready"\n', TRC),
 ('V3 both after the fork', C, '    : > "$VOL.ready"\n    : > "$slog"\n    "$SKYZFSD" serve --dir "$VOL" --run-dir "$RUN" "${hold[@]}" --request-log "$RDIR/requests.tsv" > "$VOL.ready" 2> "$slog" &\n    SERVE_PID=$!; DEV=""\n',
  '    "$SKYZFSD" serve --dir "$VOL" --run-dir "$RUN" "${hold[@]}" --request-log "$RDIR/requests.tsv" > "$VOL.ready" 2> "$slog" &\n    SERVE_PID=$!; DEV=""\n    : > "$VOL.ready"\n    : > "$slog"\n', TRC),
 # 4f0d632: agent_quiet's skip, and cleanup's lim.
 ('V4 quiet: export with no daemon', A, '    if [ -z "$SERVE_PID" ] || ! kill -0 "$SERVE_PID" 2>/dev/null; then\n        log "$NAME: $POOL not exported', '    if false; then\n        log "$NAME: $POOL not exported', TR),
 ('V5 quiet: skip only on an empty pid', A, '    if [ -z "$SERVE_PID" ] || ! kill -0 "$SERVE_PID" 2>/dev/null; then\n        log "$NAME: $POOL not exported', '    if [ -z "$SERVE_PID" ]; then\n        log "$NAME: $POOL not exported', TR),
 ('V6 quiet: skip always', A, '    if [ -z "$SERVE_PID" ] || ! kill -0 "$SERVE_PID" 2>/dev/null; then\n        log "$NAME: $POOL not exported', '    if true; then\n        log "$NAME: $POOL not exported', TR),
 ('V7 quiet: skip before the session end', A, '    session_end\n    # With no daemon', '    # With no daemon', TR),
 ('V8 cleanup: timeout again', C, '    lim 120 zpool export -f "$POOL" > /dev/null 2>&1\n', '    timeout 120 zpool export -f "$POOL" 2>/dev/null\n', TRC),
 ('V9 cleanup: no export', C, '    lim 120 zpool export -f "$POOL" > /dev/null 2>&1\n', '', TRC),
 # 1eaa7ca: the CLI recorder's sweep.
 ('V10 held_names: any count', Z, "$2 ~ /^[0-9]+$/ && $2 > 0 { print $1 }", "$2 ~ /^[0-9]+$/ { print $1 }", TC),
 ('V11 scale: no cap of 1000', Z, 'zfs holds -H "${held[@]:0:HOLDS_AT_ONCE}"', 'zfs holds -H "${held[@]}"', TC),
 ('V12 sweep: gone not among the names', Z, 'rec "$x-sweep-holds-gone" zfs holds -H "${held[0]}" "$p@gone" "${held[@]:1}"', 'rec "$x-sweep-holds-gone" zfs holds -H "${held[@]}"', TC),
 ('V13 sweep: gone not destroyed', Z, '    rec "$x-sweep-gone-destroy" zfs destroy -r "$p@gone"\n', '', TC),
 ('V14 sweep: the odd tag as UTF-8', Z, "ODD_TAG=$'caf\\351'", "ODD_TAG=$'caf\\303\\251'", TC),
 ('V15 sweep: the odd hold not released', Z, '    rec "$x-sweep-release-odd" zfs release "$ODD_TAG" "${held[0]}"\n', '', TC),
 ('V16 main: tagged.lua required', Z, '    [ -r "$LUA_DIR/tagged.lua" ] || TAGGED_LUA=0\n', '    [ -r "$LUA_DIR/tagged.lua" ] || { echo "no tagged.lua in $LUA_DIR" >&2; exit 64; }\n', TC),
 ('V17 scale: the sweep list with no load beside', Z, '                writing "scale-$lbl-sweep-list" "$wdir" "$p" rec "scale-$lbl-sweep-list"', '                rec "scale-$lbl-sweep-list"', TC),
 ('V18 tagged run without tagged.lua', Z, '    if [ "$TAGGED_LUA" = 1 ]; then\n        program "$x-tagged-held"', '    if true; then\n        program "$x-tagged-held"', TC),
]
