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
equal the previous uberblock's), `group 0`. `rootbp` is the root block
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

## `check.tsv`

The check's lines for the recording (`tests/kernel/cut_check.py`), from
the run named in its first line. A line `cut_N FAIL` names a cut whose
volume did not open at its group; `cuts` counts those that passed.

## The recordings here

All from one job on ubuntu-26.04 (amd64, kernel 7.0.0-1019-azure, ZFS
2.4.1), run 43631961753, a pool of 512 MiB with ashift 12 on the whole
device. No write in them carries FUA.

| directory | what the pool did | cuts | the check |
|---|---|---|---|
| `life` | create, four rounds of files with a snapshot and a sync, export, import, three more rounds, a snapshot destroyed, export | 27 | every cut opened at its group, every copy passed, read-only import |
| `rewind` | create, eight rounds, export, `zpool import -T` at group 37 (`rewind-txg`), three rounds, export | 32 | cuts 1 to 22 and 29 to 32 passed. Cuts 23 to 28, the new history's groups 39 to 48 after the rewind, open at the abandoned history's group 51, whose uberblocks are still in the labels; the new history's cut 29 (group 51 again, another root block pointer) is the first that opens as itself |
| `config` | create, two rounds, `zpool set comment`, `zpool reguid`, a round, a mirror attached (waited for) and detached, two rounds, export | 26 | every cut passed |
| `stray` | create, six rounds while another process flushes the device every 2 ms, export | 17 | every cut passed |
| `autotrim` | autotrim=on with a trim batch of 4 groups, rounds, files and snapshots destroyed, a manual trim, export | 41 | every cut passed |

Not here yet: `multihost` (its check could not import the pool: the
rig's host id, to be fixed) and `full` (13 000 records; in the
artifact). Later runs add the other three images.

The session check of these runs compared the cuts with every group the
`txgs` kstat calls committed. ZFS 2.4 commits groups that wrote nothing
(`nwritten` 0) and writes no uberblock for them, so that comparison
failed everywhere; the cuts equal the committed groups that wrote, in
every session (see the pull request of M2.T1).
