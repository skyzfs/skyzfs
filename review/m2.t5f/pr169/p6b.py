# Plants on 54b666b's settings line: plant4.py WORKTREE p6b.py [IDS]
TZ = 'bash tests/kernel/test_zfs_cli_record.sh'
Z = 'tests/kernel/zfs-cli-record.sh'
PLANTS = [
 ('S10 settings line: groups default 3, settle_txgs still 2', Z, 'opens after ZCR_SETTLE_TXGS=${ZCR_SETTLE_TXGS:-2} groups', 'opens after ZCR_SETTLE_TXGS=${ZCR_SETTLE_TXGS:-3} groups', TZ),
 ('S11 settings line: limit default 3', Z, 'waiting at most ZCR_SETTLE_S=${ZCR_SETTLE_S:-30} s', 'waiting at most ZCR_SETTLE_S=${ZCR_SETTLE_S:-3} s', TZ),
 ('S12 settings line: margin default 5', Z, 'the margin of ZCR_WRITER_MARGIN=${ZCR_WRITER_MARGIN:-2} s alone', 'the margin of ZCR_WRITER_MARGIN=${ZCR_WRITER_MARGIN:-5} s alone', TZ),
 ('S13 settings line: said per entry', Z, '    for e in "$@"; do\n        n=${e%%:*}', '    for e in "$@"; do\n        [ "$#" = 0 ] || echo "scale settings: each window beside a writer opens after ZCR_SETTLE_TXGS=${ZCR_SETTLE_TXGS:-2} groups have committed since its probe began, waiting at most ZCR_SETTLE_S=${ZCR_SETTLE_S:-30} s (0 groups: no wait, the margin of ZCR_WRITER_MARGIN=${ZCR_WRITER_MARGIN:-2} s alone)" >> "$OUT/about.txt"\n        n=${e%%:*}', TZ),
]
