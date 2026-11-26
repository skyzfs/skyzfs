# Plants on the recorder's delta since 1a40bfc, at 11f02ef (part 1).
T = 'bash tests/kernel/test_zfs_cli_record.sh'
F = 'tests/kernel/zfs-cli-record.sh'
PLANTS = [
 ('R13 the count not recursive', F, '    have=$(lim 600 zfs list -H -t snapshot -o name -r "$p"', '    have=$(lim 600 zfs list -H -t snapshot -o name "$p"', T),
 ('R14 retain always', F, '    [ "$k" = 0 ] || retain "$lbl" "$p" "$n" "$k"', '    retain "$lbl" "$p" "$n" "$k"', T),
 ('A07 K the last field', F, '        case $e in *:*) r=${e#*:} ;; esac', '        case $e in *:*) r=${e##*:} ;; esac', T),
 ('A08 an empty N accepted', F, "        case $n in *[!0-9]* | 0* | '') echo \"scale: not a count: $e\"", "        case $n in *[!0-9]* | 0*) echo \"scale: not a count: $e\"", T),
 ('A09 an empty K accepted', F, "        case $r in 0) ;; *[!0-9]* | 0* | '')", "        case $r in 0) ;; *[!0-9]* | 0*)", T),
 ('L1 the free read with no limit', F, '            free=$(lim "$LIMIT" zpool list -Hp -o free "$p" 2> /dev/null)', '            free=$(zpool list -Hp -o free "$p" 2> /dev/null)', T),
 ('L2 the tenth read with no limit', F, 'allocated and free $(lim "$LIMIT" zpool list -Hp -o alloc,free', 'allocated and free $(zpool list -Hp -o alloc,free', T),
 ('P1 pools-one not recorded', F, '    rec pools-one zpool list -H -o name\n', '', T),
 ('P2 pools-three of skzq alone', F, '    rec pools-three zpool list -H -o name\n', '    rec pools-three zpool list -H -o name skzq\n', T),
 ('P3 none-pools-named with no pool', F, '    rec "$x-pools-named" zpool list -H -o name "$p"', '    rec "$x-pools-named" zpool list -H -o name', T),
]
