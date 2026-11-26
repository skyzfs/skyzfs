# Set rec at fafc771: the CLI recorder's survivors of #159's look at
# 11f02ef (n2_rec1.py's L1, L2, P1, P2; n2_old_rec_a.py; n2_old_rec_b.py)
# and new plants on this delta (G3, N3, N-a). plant2.py WORKTREE p3_rec.py
T = 'bash tests/kernel/test_zfs_cli_record.sh'
F = 'tests/kernel/zfs-cli-record.sh'
PLANTS = [
 ('L1 the free read with no limit', F, '            free=$(lim "$LIMIT" zpool list -Hp -o free "$p" 2> /dev/null)', '            free=$(zpool list -Hp -o free "$p" 2> /dev/null)', T),
 ('L2 the tenth read with no limit', F, 'allocated and free $(lim "$LIMIT" zpool list -Hp -o alloc,free', 'allocated and free $(zpool list -Hp -o alloc,free', T),
 ('P1 pools-one not recorded', F, '    rec pools-one zpool list -H -o name\n', '', T),
 ('P2 pools-three of skzq alone', F, '    rec pools-three zpool list -H -o name\n', '    rec pools-three zpool list -H -o name skzq\n', T),
 ('A04a no length check of K', F, '        if [ "${#r}" -gt 6 ] || [ "$r" -gt 20000 ]; then', '        if [ "$r" -gt 20000 ]; then', T),
 ('C06 a free that is not a number stops it', F, "                '' | *[!0-9]*) ;;", '                \'\' | *[!0-9]*) cut=" (free not read)"; break ;;', T),
 ('C09 no line at the last', F, '        if [ $((i % step)) = 0 ] || [ "$i" = "$k" ]; then', '        if [ $((i % step)) = 0 ]; then', T),
 ('R10a no time limit a call', F, '        lim "$LIMIT" zfs snapshot -r "$p@r$i"', '        zfs snapshot -r "$p@r$i"', T),
 ('C16 32 GiB without K too', F, '    if [ "$k" = 0 ]; then truncate -s 16G "$WORK/scale.img"; else', '    if false; then truncate -s 16G "$WORK/scale.img"; else', T),
 ('C22 began not called', F, '    if [ -n "$fpid" ]; then began "$log.tsv" "$log.fill.tsv" "$log.txg.tsv"; else began "$log.tsv" "$log.txg.tsv"; fi\n', '', T),
 ('C23 began waits for nothing', F, '        for f in "$@"; do [ -s "$f" ] || ok=0; done', '        :', T),
 ('C24 began: the fill log not waited for', F, 'then began "$log.tsv" "$log.fill.tsv" "$log.txg.tsv"; else', 'then began "$log.tsv" "$log.txg.tsv"; else', T),
 # New.
 ('W01 no K reached line', F, '    echo "scale $lbl: K reached: $((i - 1)) of the $k asked" >> "$OUT/about.txt"\n', '', T),
 ('W02 K reached one over', F, 'K reached: $((i - 1)) of the $k asked', 'K reached: $i of the $k asked', T),
 ('W03 lines_line without secs', F, ' lines in $(cat "$OUT/$1/secs" 2> /dev/null || echo \'?\') s"', ' lines"', T),
 ('W04 holds with no load beside', F, '                writing "scale-$lbl-holds" "$wdir" "$p" rec "scale-$lbl-holds" zfs holds -H -r "$p@t"', '                rec "scale-$lbl-holds" zfs holds -H -r "$p@t"', T),
 ('W05 K reached: K asked', F, 'K reached: $((i - 1)) of the $k asked', 'K reached: $k of the $k asked', T),
 ('W06 32 GiB at K 0 too (re-anchored C16)', F, '    if [ "$k" = 0 ]; then truncate -s 16G "$WORK/scale.img"; else truncate -s 32G "$WORK/scale.img"; fi', '    truncate -s 32G "$WORK/scale.img"', T),
]
