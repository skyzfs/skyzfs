# M1.T27, the hole rig (pull request 63), head d0efbfd on a234314

Review posted on the pull request. Nothing blocks.

What was run here (no kernel):

- `test_replay_hole.py` (16), `test_replay.py`, `test_replay_daemon.py`: pass.
- `plants63.py`: 13 defects planted in `replay_hole.py` and `hole-rig.sh`,
  one at a time; `plants63.txt` has which test catches each. Ten are
  caught. Not caught: "on the disk" one prefix late (`data_at < n`), no
  course checked gives exit 0, the rig does not ask for the control.
- `zero-fill-detect.sh`: `skyzfsd help 2>&1 | grep -q -- --zero-fill`
  under `pipefail`, 4,000 tries, half of them beside busy loops: never
  read as missing. Not a finding.

Read from CI at d0efbfd (run 43498059751), both hole jobs green:

| image | fs | kind | checked of | zeros after return | of them on the disk | control |
|---|---|---|---|---|---|---|
| 26.04 | ext4 | fallocate | 69 of 102 | 43 | 26 | caught |
| 26.04 | ext4 | sparse | 69 of 110 | 44 | 27 | caught |
| 26.04 | ext4 | zero-fill | 69 of 74 | 25 | 0 | caught |
| 26.04 | xfs | fallocate | 69 of 73 | 36 | 19 | caught |
| 26.04 | xfs | sparse | 69 of 72 | 37 | 20 | caught |
| 26.04 | xfs | zero-fill | 66 of 66 | 27 | 0 | caught |
| 24.04 | ext4 | fallocate | 70 of 119 | 43 | 25 | caught |
| 24.04 | ext4 | sparse | 69 of 108 | 44 | 27 | caught |
| 24.04 | ext4 | zero-fill | 69 of 73 | 26 | 0 | caught |
| 24.04 | xfs | fallocate | 69 of 73 | 36 | 19 | caught |
| 24.04 | xfs | sparse | 69 of 72 | 37 | 20 | caught |
| 24.04 | xfs | zero-fill | 66 of 66 | 27 | 0 | caught |

The courses' lengths are the notes' except ext4 fallocate on 24.04 (119
here, 100 in the notes): a course's length is not fixed from run to run.

Points of the reading:

- `logwrites.index` gives a discard record no `pos`, so `data_prefixes`
  never reads a discard's length.
- The third count is 0 on a written-through image by construction: a
  block there needs no commit of the file system, so any prefix that
  holds the data record shows the data. A figure above 0 would mean the
  image was not written through.
- dm-log-writes records whole bios, so the checker's "block whole" rule
  cannot differ here from §4.1's tear at a sector.
