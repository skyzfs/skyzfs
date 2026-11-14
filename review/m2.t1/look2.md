## Next look at #94 (M2.T1), head b3b724b

**All five suggestions are settled, and nothing blocks the merge.** What remains is the four-image cut rig run on this head, [43654937146](https://github.com/skyzfs/skyzfs/actions/runs/43654937146). It was still recording when I wrote this. Its table should read as `EXPECTED` on all four images: `rewindf` is checked on 24.04 for the first time. The run then goes into the description, where the placeholder `(CUT_RIG2)` stands.

**How I checked.**
- I read 0445de2, ab245d5 and b3b724b.
- Both test files pass: 32 and 22 tests.
- I re-ran the planted changes on the new head (`review/m2.t1/plants2.py` on `skyzfs-review`): my first 30, minus the four whose code changed, plus 17 for the new code.
  - Of 43, 37 are caught.
  - Two survivors are equivalent: `stale` flagging consecutive states, as before, and N4 below.
  - One is nearly equivalent: N5 below.
  - Three are in code the unit tests do not reach: L2 needs a kernel, E2 is in `main`, and X2 has no stale cut in its test. All three below.

**S1, the `-T` import.** You are right, and my suggested fix was wrong. In `SPA_LOAD_RECOVER`, `spa_load_best` adds `ZPOOL_CONFIG_REWIND_INFO` only outside that state (`spa.c:6394`), so `zpool_rewind_exclaim` prints nothing.

`loaded_txg` is exact:
- it takes the last `using uberblock with txg=` note (`spa.c:4767`);
- a retry always goes to a lower group, because `spa_load_retry` sets the maximum to `ub_txg − 1` (`spa.c:6280`).

The plant that takes the first note instead of the last (L1) is caught. The plant that drops the comparison in `kimport` (L2) needs a kernel to be seen.

One inaccuracy, not blocking: the test's sample names the tryimport's loads `$import`, and 2.4.1 does not do that.
- `spa_tryimport` sets `spa_load_name` to the pool's own name (`spa.c:7541`).
- Run 43647779663's `rewindf/dbgmsg-FX.txt` starts with `spa_load(skzcut, config untrusted): using uberblock with txg=43`, from the tryimport, before `spa_import: importing skzcut`.
- So the tryimport's notes match the pattern too. What keeps `loaded_txg` right is that they come first.
- The comment "another pool's load (the tryimport's name) is not this one's" describes something 2.4.1 does not do. A verbatim sample would pin the real reason.

**S2, the order outside the kstat.** Settled.
- The line now gives the coverage.
- The strict order reaches the 54 cuts outside the kstats' windows.
- The plants that make the order non-strict, stop the span at the end mark, or drop the count (X1, X3, X4) are caught.
- The plant that keeps stale cuts in the span (X2) passes the unit tests. A stale cut breaks the strict order, though, so the `multihost` recording would fail on it.

**S3, the order among uberblocks.** It matches `vdev_uberblock_compare` (`vdev_label.c`) and `MMP_SEQ` (`uberblock_impl.h`: mask `0x0000FFFF00000000`, valid bit `0x02`).
- Caught: the plants that drop the timestamp (N1), drop the MMP sequence (N2), let a tie count as itself (N3), or read the sequence without the MMP magic (N6).
- N4, which collects the cut's own uberblocks by group alone, is equivalent. Another root of the group then lands in both sets, so it ties and reads "not itself", which is right.
- N5, which reads the sequence without its valid bit, survives. ZFS sets the bit whenever it sets a sequence (`MMP_SEQ_SET`), so it is nearly equivalent.
- `Fixtures` recomputes `opens` against `opens.tsv`.

**S4 and S5.** Settled.
- Caught: the plants that expect `any` stale cuts everywhere (E1), drop the root or the instant from `rule_same` (Q1, Q2), let `EXPECTED` hold `missed` (C13), or skip the version check (N7).
- The plant that ignores the stale verdict in `main` (E2) survives, because `main` has no unit test. The rig's lines show that verdict.

**The first four-image run** (43651302466, at 8a131ee) concluded `failure`. That was only `rewindf`'s rig error on the two 24.04 images, as the description says: zdb 2.2's root block pointer at `-uuuu`, fixed in b3b724b.
