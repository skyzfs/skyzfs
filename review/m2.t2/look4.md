## Look at #97 (M2.T2), head 83d41e4

**Settled. I have no finding left on #97: it can merge once the rest of CI on this head is green.** When I wrote this the node B rig had passed, and `ci`, `coverage`, `e2e`, the control rig and the kernel probe were still running.

### The change, `0f2f8c7..83d41e4`

Six lines in `snapshot_files`, and `SnapshotPaths` in the tests.

- **The listing.** A `zfs list` that ends with any exit code but 0 raises. An empty list is still a missing snapshot.
- **The mount.** A mount that times out or cannot start raises. A mount that fails is still the stage `mount`.
- **After a raise** the `finally` still unmounts, exports and frees the loop device.

### How I checked

- **The tests here:** `test_nodeb_check.py` (28) passes.
- **Planted changes:** four on the two new conditions, each caught by `SnapshotPaths`:
  - a list that did not come read as names;
  - only a timed-out list raising, a failed one read as names;
  - a timed-out mount recorded as a failed mount;
  - any failed mount raising.
- **My reproducer** (`review/m2.t2/verify_paths2.py`) now ends as the rig's error where it read "caught" at 0f2f8c7.
- **The head's run**, [43671750087](https://github.com/skyzfs/skyzfs/actions/runs/43671750087): both jobs end with `worst exit 0`. For `life` and `slog`, on ubuntu-26.04 and ubuntu-26.04-arm:

  | Line | Result |
  |---|---|
  | published cut | PASS, all seven stages |
  | `digest` | caught at files |
  | `early` | caught at snapshot; the import and the copies pass; the pool has `s6`, `s12`, `s18` |
  | `lacks` | caught at copies |
  | `later` | caught at import; kind: an uberblock of a later group |
  | `hole` | caught at files |

### One consequence of the choice, not a finding

A control whose damage makes `zfs list` itself fail after a good import now ends the run as the rig's error, exit 2, and not as a catch. That errs to the safe side, since it can never read "caught" wrongly. No run has shown it. If one does, the listing wants a stage of its own that `early` does not allow.

### For the lead's read of the rigs

- **Node B rig:** green on this head, on both images.
- **Cut, replay and daemon rigs:** cancelled by testing on the three heads since 5de05aa, which change none of their files. Their last complete runs are on 5de05aa, all green.
- **Codex:** both of its findings are answered on their threads and fixed, in 0f2f8c7 and 83d41e4.
