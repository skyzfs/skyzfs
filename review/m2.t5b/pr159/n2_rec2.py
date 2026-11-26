# Plants on the recorder's delta since 1a40bfc, at 11f02ef (part 2: count-snapshots and holds).
T = 'bash tests/kernel/test_zfs_cli_record.sh'
F = 'tests/kernel/zfs-cli-record.sh'
L = '    for step in count count-snapshots snapshot snapshot-cli members hold holds tagged release destroy take-cli'
PLANTS = [
 ('H1 holds not run', F, L, L.replace(' hold holds ', ' hold '), T),
 ('H2 holds before the hold', F, L, L.replace(' hold holds ', ' holds hold '), T),
 ('H3 holds after the release', F, L, L.replace(' hold holds tagged release ', ' hold tagged release holds '), T),
 ('H4 holds without -r', F, '                rec "scale-$lbl-holds" zfs holds -H -r "$p@t"', '                rec "scale-$lbl-holds" zfs holds -H "$p@t"', T),
 ('H5 holds: no lines line', F, '                lines_line "scale-$lbl-holds"', '                :', T),
 ('S1 count-snapshots without -r', F, '                rec "scale-$lbl-count-snapshots" zfs list -r -H -o name -t snapshot "$p"', '                rec "scale-$lbl-count-snapshots" zfs list -H -o name -t snapshot "$p"', T),
 ('S2 count-snapshots after the snapshot', F, L, L.replace('count count-snapshots snapshot snapshot-cli', 'count snapshot count-snapshots snapshot-cli'), T),
 ('S3 count-snapshots: no lines line', F, '                lines_line "scale-$lbl-count-snapshots"', '                :', T),
 ('S4 count-snapshots not run', F, L, L.replace('count count-snapshots snapshot', 'count snapshot'), T),
]
