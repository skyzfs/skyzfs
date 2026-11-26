F = 'tests/kernel/agent-record.sh'
T = 'bash tests/kernel/test_agent_record.sh'
# published() planted wrong at ea39b27: does the rewritten test fail?
P = [
 ('T01 published: no gone path (old code)', '''    if ! grep -qxF "$POOL@$n" "$CDIR/snapshots.txt"; then''', '''    if false; then'''),
 ('T02 published: substring match', '''grep -qxF "$POOL@$n" "$CDIR/snapshots.txt"''', '''grep -qF "$n" "$CDIR/snapshots.txt"'''),
 ('T03 published: gone line not written', '''        printf '%s\\t%s\\n' "$(basename "$CDIR")" "$n" >> "$RDIR/published-gone.tsv"\n''', ''),
 ('T04 published: a list that fails goes on', '''        || rig_error "zfs list of $POOL's snapshots failed: $(tail -n 2 "$CDIR/snapshots.err")"''', '''        || true'''),
 ('T05 published: no per-case early return', '''    grep -qxF "$(printf '%s\\t%s' "$(basename "$CDIR")" "$n")" "$RDIR/published-gone.tsv" 2>/dev/null && return 0\n''', ''),
 ('T06 published: snapshot of a descendant counts', '''grep -qxF "$POOL@$n" "$CDIR/snapshots.txt"''', '''grep -qE "@$n\\$" "$CDIR/snapshots.txt"'''),
 ('T08 published: gone line without the case', '''        printf '%s\\t%s\\n' "$(basename "$CDIR")" "$n" >> "$RDIR/published-gone.tsv"''', '''        printf '%s\\n' "$n" >> "$RDIR/published-gone.tsv"'''),
 ('T09 published: gone line overwrites', '''"$n" >> "$RDIR/published-gone.tsv"''', '''"$n" > "$RDIR/published-gone.tsv"'''),
 ('T13 published: list error message lost', '''failed: $(tail -n 2 "$CDIR/snapshots.err")"''', '''failed"'''),
 ('T14 published: gone check after the read (rig error)', '''    if ! grep -qxF "$POOL@$n" "$CDIR/snapshots.txt"; then''', '''    if ! (cd "$MNT/.zfs/snapshot/$n" 2>/dev/null); then rig_error "reading the files of $POOL@$n failed"; fi\n    if false; then'''),
]
PLANTS = [(pid, F, old, new, T) for pid, old, new in P]
