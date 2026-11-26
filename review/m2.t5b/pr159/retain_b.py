F = 'tests/kernel/zfs-cli-record.sh'
T = 'bash tests/kernel/test_zfs_cli_record.sh'
W = '.github/workflows/skyzfs-zfs-cli-record.yml'
P = [
 ('R14 retain always', 'tests/kernel/zfs-cli-record.sh', '    [ "$k" = 0 ] || retain "$lbl" "$p" "$n" "$k"', '    retain "$lbl" "$p" "$n" "$k"'),
 ('R15 retain never', 'tests/kernel/zfs-cli-record.sh', '    [ "$k" = 0 ] || retain "$lbl" "$p" "$n" "$k"', '    :'),
 ('R16 label always -sK', 'tests/kernel/zfs-cli-record.sh', '    [ "$k" = 0 ] || lbl=$n-s$k', '    lbl=$n-s$k'),
 ('R17 retain after the steps', 'tests/kernel/zfs-cli-record.sh', '    [ "$k" = 0 ] || retain "$lbl" "$p" "$n" "$k"\n    for step in', '    for step in'),
 ('R18 same names every batch', 'tests/kernel/zfs-cli-record.sh', '            names+=("$p@r$j")', '            names+=("$p@r$((j - i + 1))")'),
 ('A01 K 0 refused', 'tests/kernel/zfs-cli-record.sh', "        case $r in 0) ;; *[!0-9]* | 0* | '')", "        case $r in *[!0-9]* | 0* | '')"),
 ('A02 K with a leading 0 accepted', 'tests/kernel/zfs-cli-record.sh', "        case $r in 0) ;; *[!0-9]* | 0* | '')", "        case $r in 0) ;; *[!0-9]* | '')"),
 ('A03 bound 2000000', 'tests/kernel/zfs-cli-record.sh', '[ $(((n + 1) * r)) -gt 200000 ]', '[ $(((n + 1) * r)) -gt 2000000 ]'),
 ('A04 no length check of K', 'tests/kernel/zfs-cli-record.sh', '        if [ "${#r}" -gt 6 ] || [ $(((n + 1) * r)) -gt 200000 ]; then', '        if [ $(((n + 1) * r)) -gt 200000 ]; then'),
 ('A05 seen by N alone', 'tests/kernel/zfs-cli-record.sh', '        case $seen in *" $n:$r "*)', '        case $seen in *" $n:"*)'),
 ('A07 K the last field', 'tests/kernel/zfs-cli-record.sh', '        case $e in *:*) r=${e#*:} ;; esac', '        case $e in *:*) r=${e##*:} ;; esac'),
 ('A08 an empty N accepted', 'tests/kernel/zfs-cli-record.sh', '        case $n in *[!0-9]* | 0* | \'\') echo "scale: not a count: $e"', '        case $n in *[!0-9]* | 0*) echo "scale: not a count: $e"'),
 ('A09 an empty K accepted', 'tests/kernel/zfs-cli-record.sh', "        case $r in 0) ;; *[!0-9]* | 0* | '')", '        case $r in 0) ;; *[!0-9]* | 0*)'),
 ('A10 bound N*K', 'tests/kernel/zfs-cli-record.sh', '[ $(((n + 1) * r)) -gt 200000 ]', '[ $((n * r)) -gt 200000 ]'),
]
PLANTS = [(pid, fn, old, new, T) for pid, fn, old, new in P]
