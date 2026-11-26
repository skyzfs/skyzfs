# The recorder survivors at 1a40bfc not already in n2_rec1.py (from n2_old_rec.py).
PLANTS = [
 ('C16 32 GiB without K too', 'tests/kernel/zfs-cli-record.sh', '    if [ "$k" = 0 ]; then truncate -s 16G "$WORK/scale.img"; else', '    if false; then truncate -s 16G "$WORK/scale.img"; else', 'bash tests/kernel/test_zfs_cli_record.sh'),
 ('C22 began not called', 'tests/kernel/zfs-cli-record.sh', '    if [ -n "$fpid" ]; then began "$log.tsv" "$log.fill.tsv" "$log.txg.tsv"; else began "$log.tsv" "$log.txg.tsv"; fi\n', '', 'bash tests/kernel/test_zfs_cli_record.sh'),
 ('C23 began waits for nothing', 'tests/kernel/zfs-cli-record.sh', '        for f in "$@"; do [ -s "$f" ] || ok=0; done', '        :', 'bash tests/kernel/test_zfs_cli_record.sh'),
 ('C24 began: the fill log not waited for', 'tests/kernel/zfs-cli-record.sh', 'then began "$log.tsv" "$log.fill.tsv" "$log.txg.tsv"; else', 'then began "$log.tsv" "$log.txg.tsv"; else', 'bash tests/kernel/test_zfs_cli_record.sh'),
]
