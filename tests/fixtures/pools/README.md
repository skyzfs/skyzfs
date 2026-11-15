# Recorded pools: fixtures for the uberblock parser and the count of cuts

Each directory here is one pool recorded from its creation by the cut
rig (M2.T1: `tests/kernel/cut-record.sh`, workflow
`skyzfs-cut-rig.yml`), with dm-log-writes between ZFS and the pool's
device. Its files are written by `tests/kernel/cut_ref.py fixture`.
The reference's cuts in them were checked on real ZFS: at each cut's
instant the volume opened at the cut's group, every block it reaches
passed its checksum in every copy, and it imported read-only. Where a
directory comes from (image, kernel, ZFS) is in the `# about` lines of
its files.

The record is in the order the writes completed at dm-log-writes, which
is after the device applied them. So a write's place in the record is
the moment it counts from under design §5.2's rule.

## `requests.tsv.gz`

Gzip text. Lines starting with `#` are comments: the format version
(`# skyzfs cut fixture, version 1`), `# dev_bytes N` (the device's
size, which places the back labels), `# sectorsize`, `# records`, and
`# about` lines. Then one line per record, tab-separated:

```text
i  op  offset  bytes  data
```

- `i`: the record's number, from 0.
- `op`: `write`, `write+fua`, `write+flush` (a write with REQ_PREFLUSH:
  the flush comes before its data), `flush` (no data), `discard`, or
  `mark` (a mark the rig put in the record; `data` is its text). The
  reference takes `write+fua` as the daemon serves it: the write, then
  a flush (the pump's), whose barrier comes after the write.
- `offset`, `bytes`: on the device, in bytes.
- `data`: the write's bytes in hex, for every write that touches one of
  the four labels; `-` for any other write and for the other ops.

So the label writes are whole, and every other request is there with
its place, its range and its kind, but not its content.

## `uberblocks.tsv`

Every uberblock the reference found, in record order:

```text
record  label  slot  order  txg  mmp  taken  rootbp
```

`slot` is the slot's byte offset on the device, `order` `le` or `be`,
`mmp` 1 when ub_mmp_magic is set. `taken` is what the rule made of it:
`marks` (a new uberblock), `repeat` (its group and root block pointer
equal those of the pending mark or of the newest cut, design §5.2: a
repeat makes no cut and changes no mark), `group 0`. `rootbp` is the root block
pointer's sixteen words, each as 16 hex digits, in the writer's byte
order read as numbers.

## `cuts.tsv`

The reference's cuts:

```text
n  txg  prefix  ub_record  flush_record  rootbp
```

`n` is the cut's number in the count, from 1. `ub_record` is the write
that marked it. `flush_record` is the first flush after it. `prefix` is
the number of records before the cut's instant: the volume at the
instant is the device after records `0 .. prefix-1`.

## `opens.tsv` (from the run after 71e2509)

What real ZFS made of each reference cut, a line a cut:

```text
n  txg  prefix  zdb_txg  ring_txg  itself  whole
```

`zdb_txg` is the group `zdb -e -u` selected on the volume at the cut's
instant, `ring_txg` the best uberblock in the labels' rings by the
reference (the highest group, then the newest timestamp, following
every write and discard of the ring through the record), `itself` 1 when
that is the cut's own state (design §5.2, "Whether a cut opens as
itself"), and `whole` 1 when the cut passed every copy and the read-only
import at its own group (with `zdb -t` and `zpool import -T` where it
does not open as itself). M2.1b's test: the cuts the store says open as
themselves are exactly those with `itself` 1.

## `check.tsv`

The check's lines for the recording (`tests/kernel/cut_check.py`), from
the run named in its first line. A line `cut_N FAIL` names a cut whose
volume did not open at its group; `cuts` counts those that passed.

## Other files

- `rewind-txg` (rewind): the group `zpool import -T` rewound to.
- `rewind-sync.txt` (rewindsync): the group rewound to, the abandoned
  history's newest, and the syncs until a group past it wrote.
- `rewind-f.txt` (rewindf): the copies of the root block that were
  zeroed, what `zpool import` listed, and what `-F` and then `-FX` said.
- `mmp.txt` (multihost): MMP's timings for the recording.

`tests/kernel/test_cut_ref.py` (class `Fixtures`) reads every directory
here back with `cut_ref.read_fixture` and holds the reference's cuts and
uberblocks to `cuts.tsv` and `uberblocks.tsv`, and `opens.tsv` and
`check.tsv` to the check having passed every cut.

## The recordings here

All from one job on ubuntu-26.04 (amd64, kernel 7.0.0-1019-azure, ZFS
2.4.1), run 43647779663 at 3a2859b, a pool of 512 MiB with ashift 12 on
the whole device. No write in them carries FUA. Every cut in every one
was whole at its own group: every copy of every block it reaches passed
its checksum, and the pool imported at it.

| directory | what the pool did | cuts | opens as itself |
|---|---|---|---|
| `life` | create, four rounds of files with a snapshot and a sync, export, import, three more rounds, a snapshot destroyed, export | 26 | all |
| `slog` | the product's layout: a log device beside the data device (not recorded); create, export, import, load with synchronous writes, export | 29 | all; zdb needed the log device's copy beside the data device for some calls (`obs_zdb_with_log_device_copy` in `check.tsv`) |
| `rewind` | create, eight rounds, export, `zpool import -T` at group 37, three rounds, export | 32 | all but cuts 23 to 28 (the new history's groups 39 to 48), which open at the abandoned history's group 51, whole |
| `rewindsync` | as `rewind`, then a small write and a `zpool sync` at a time until a group past 51 wrote (4 syncs, 77 ms) | 32 | all but cuts 23 to 27 (groups 39 to 49), which open at the abandoned group 51, whole here; in run 43638676548 that state was not whole (13 to 16 of 783 copies failed) |
| `rewindf` | create, rounds, export, the three copies of the newest group's (43) root block zeroed, `zpool import -F` (refused: the export rewrote the labels' config in group 43), then `-FX`, which wrote group 43 again with another root block pointer (cut 19), rounds, export | 29 | all |
| `multihost` | multihost=on, MMP at 100 ms, rounds for 20 s, export | 93 | all but cuts 10 and 27, stale (the states of cuts 8 and 25, rewritten by MMP after a newer cut), which open at the next group |
| `config` | create, two rounds, `zpool set comment`, `zpool reguid`, a round, a mirror attached (waited for) and detached, two rounds, export | 26 | all |
| `stray` | create, six rounds while another process flushes the device every 2 ms, the back labels' writes held 20 ms | 17 | all |
| `autotrim` | autotrim=on with a trim batch of 4 groups, rounds, files and snapshots destroyed, a manual trim, export | 40 | all |

Not here: `full` (13 500 records, 219 cuts, all whole; in the run's
artifact). The pull request of M2.T1 runs the rig on the other three
images too.

The check's lines in `check.tsv` are those of that run. Three of its
controls read DIFFERS there against the table of that commit and not
against the table now: `later` was caught in `life`, `config` and
`rewindsync` (a write a few hundred records after an early cut), which
the table now allows, and `txg-alone` in `multihost` dropped only the
two stale cuts, which the check now counts as the same list.
