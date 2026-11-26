# The recorder survivors at 1a40bfc not already in n2_rec1.py (from n2_old_rec.py).
PLANTS = [
 ('A04a no length check of K', 'tests/kernel/zfs-cli-record.sh', '        if [ "${#r}" -gt 6 ] || [ "$r" -gt 20000 ]; then', '        if [ "$r" -gt 20000 ]; then', 'bash tests/kernel/test_zfs_cli_record.sh'),
 ('C06 a free that is not a number stops it', 'tests/kernel/zfs-cli-record.sh', "                '' | *[!0-9]*) ;;", '                \'\' | *[!0-9]*) cut=" (free not read)"; break ;;', 'bash tests/kernel/test_zfs_cli_record.sh'),
 ('C09 no line at the last', 'tests/kernel/zfs-cli-record.sh', '        if [ $((i % step)) = 0 ] || [ "$i" = "$k" ]; then', '        if [ $((i % step)) = 0 ]; then', 'bash tests/kernel/test_zfs_cli_record.sh'),
 ('R10a no time limit a call', 'tests/kernel/zfs-cli-record.sh', '        lim "$LIMIT" zfs snapshot -r "$p@r$i"', '        zfs snapshot -r "$p@r$i"', 'bash tests/kernel/test_zfs_cli_record.sh'),
]
