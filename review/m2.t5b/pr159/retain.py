F = 'tests/kernel/zfs-cli-record.sh'
T = 'bash tests/kernel/test_zfs_cli_record.sh'
W = '.github/workflows/skyzfs-zfs-cli-record.yml'
P = [
 ('R01 per: 10000/N', F, '    per=$((10000 / (n + 1)))', '    per=$((10000 / n))'),
 ('R02 per may be 0', F, '    [ "$per" -ge 1 ] || per=1\n    t0=', '    t0='),
 ('R03 no fallback: stop', F, '            per=1\n            continue\n', '            break\n'),
 ('R04 fallback on any batch', F, '        if [ "$rc" != 0 ] && [ "$i" = 1 ] && [ "$per" -gt 1 ]; then', '        if [ "$rc" != 0 ] && [ "$per" -gt 1 ]; then'),
 ('R05 fallback skips the first batch', F, '            per=1\n            continue\n', '            per=1\n            i=$j\n            continue\n'),
 ('R06 one name too many a batch', F, '[ "$j" -lt $((i + per)) ]', '[ "$j" -le $((i + per)) ]'),
 ('R07 the last name dropped', F, '        while [ "$j" -le "$k" ] && [ "$j" -lt', '        while [ "$j" -lt "$k" ] && [ "$j" -lt'),
 ('R08 a later failure goes on', F, '''        if [ "$rc" != 0 ]; then\n            cut=" (stopped: zfs snapshot -r of @r$i''', '''        if false; then\n            cut=" (stopped: zfs snapshot -r of @r$i'''),
 ('R09 no budget check', F, '''        if ! scale_limit; then\n            cut=" (stopped: the budget was spent)"''', '''        if false; then\n            cut=" (stopped: the budget was spent)"'''),
 ('R10 no time limit a call', F, '        lim "$LIMIT" zfs snapshot -r "${names[@]}"', '        zfs snapshot -r "${names[@]}"'),
 ('R11 not recursive', F, '        lim "$LIMIT" zfs snapshot -r "${names[@]}"', '        lim "$LIMIT" zfs snapshot "${names[@]}"'),
 ('R12 the line says the first per', F, '($k of each of $((n + 1)) datasets asked, $per names a call)', '($k of each of $((n + 1)) datasets asked, $((10000 / (n + 1))) names a call)'),
 ('R13 count not recursive', F, '    have=$(lim 600 zfs list -H -t snapshot -o name -r "$p"', '    have=$(lim 600 zfs list -H -t snapshot -o name "$p"'),
 ('R14 retain always', F, '    [ "$k" = 0 ] || retain "$lbl" "$p" "$n" "$k"', '    retain "$lbl" "$p" "$n" "$k"'),
 ('R15 retain never', F, '    [ "$k" = 0 ] || retain "$lbl" "$p" "$n" "$k"', '    :'),
 ('R16 label always -sK', F, '    [ "$k" = 0 ] || lbl=$n-s$k', '    lbl=$n-s$k'),
 ('R17 retain after the steps', F, '    [ "$k" = 0 ] || retain "$lbl" "$p" "$n" "$k"\n    for step in', '    for step in'),
 ('R18 same names every batch', F, '            names+=("$p@r$j")', '            names+=("$p@r$((j - i + 1))")'),
 # scale_all's N:K
 ('A01 K 0 refused', F, '        case $r in 0) ;; *[!0-9]* | 0* | \'\')', '        case $r in *[!0-9]* | 0* | \'\')'),
 ('A02 K with a leading 0 accepted', F, '        case $r in 0) ;; *[!0-9]* | 0* | \'\')', '        case $r in 0) ;; *[!0-9]* | \'\')'),
 ('A03 bound 2000000', F, '[ $(((n + 1) * r)) -gt 200000 ]', '[ $(((n + 1) * r)) -gt 2000000 ]'),
 ('A04 no length check of K', F, '        if [ "${#r}" -gt 6 ] || [ $(((n + 1) * r)) -gt 200000 ]; then', '        if [ $(((n + 1) * r)) -gt 200000 ]; then'),
 ('A05 seen by N alone', F, '        case $seen in *" $n:$r "*)', '        case $seen in *" $n:"*)'),
 ('A07 K the last field', F, '        case $e in *:*) r=${e#*:} ;; esac', '        case $e in *:*) r=${e##*:} ;; esac'),
 ('A08 an empty N accepted', F, "        case $n in *[!0-9]* | 0* | '') echo \"scale: not a count: $e\"", "        case $n in *[!0-9]* | 0*) echo \"scale: not a count: $e\""),
 ('A09 an empty K accepted', F, "        case $r in 0) ;; *[!0-9]* | 0* | '')", "        case $r in 0) ;; *[!0-9]* | 0*)"),
 ('A10 bound N*K', F, '[ $(((n + 1) * r)) -gt 200000 ]', '[ $((n * r)) -gt 200000 ]'),
]
PLANTS = [(pid, fn, old, new, T) for pid, fn, old, new in P]
