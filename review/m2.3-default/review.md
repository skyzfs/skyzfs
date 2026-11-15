## Review of #109 at 618f536 (M2.3c)

**Verdict: can merge once CI on 618f536 is green. I found no defect, and nothing else names or depends on 64 MiB.** One small gap in the tests (G1), which I would not wait for.

### What I was asked to judge: does anything else still name or depend on 64 MiB?

No. I searched the head for the size in every spelling (`64 MiB`, `64 << 20`, `67108864`, `--intent-region-mib 64`), for "provisional", and for every use of the default and of the region's size.

| Where | What I found |
|---|---|
| The store and the daemon | The constant, its use (`state.rs:345`), the derived `DEFAULT_INTENT_REGION_MIB`, the help text: all changed or derived. No other number stands in for the default. "Provisional" is left only on the copies' bound, which is another default. |
| Callers that give no region | `inplace/harness.rs:145`, `tests/inplace_real.rs:791`, `cut_commands/tests.rs:487`. They now run at 4 MiB, and none asserts a region count or a size. |
| The rigs, the benchmark, the workflows | Nothing under `tests/kernel`, `tests/bench`, `tests/lib` or `.github` names the map or its region; the expected tables have no line that depends on it. The benchmark's rows name the size in their sequence, not in a file. |
| The documents | The notes' remaining "64 MiB" are the measurements and the decision itself. The design names no default. |

The one other dependence is the one the description states: a volume whose map was kept at another size counts the whole volume once at its next start.

### What I ran

`skyzfs-local` and `skyzfsd` with `test-hooks`, as the head has them: 254 and 99 tests pass. Then three plants (`review/m2.3-default/plants.py`, `plants.out`):

| Plant | Result |
|---|---|
| P1: a store given no region takes 64 MiB, whatever the constant says | **passes every test** (G1) |
| P2: the constant is 64 MiB again | KILLED by `the_intent_region_is_a_power_of_two_of_mib_with_held_cuts` |
| P3: the help says 64 unless given | KILLED by the same test |

### G1 (gap, small). No test holds that a store given no region uses the constant

The test holds the constant to 4 and the help text to the constant. Nothing holds the store to the constant: with `unwrap_or(64 << 20)` at `state.rs:345` every test passes, the help says 4, and the daemon runs at 64. The gap is #106's and not new here, but this pull request is the one about the default. One assertion would close it: a store opened with `intent_region: None` shows `DEFAULT_INTENT_REGION` as its region in `intent_figures`.

### A note on the constant's comment

"At 1 MiB its page writes on the queue's thread cost 8% of tpcb" gives the page writes all of the cost. The notes say the cost is more than the summed sync time explains, so the page writes hold the queue's thread too. "Its syncs and its page writes" would say what was measured.

### CI and Codex

- On 618f536 the fast jobs I can see are green or still running (`test`, `unit-arm64`, `coverage`, the daemon rig's three, the replay jobs through `skyzfsd`). The rigs are the first runs at 4 MiB regions, so their tables are worth the lead's look before the merge, as always.
- Codex has no findings on this head.

Notes: `review/m2.3-default/` on `skyzfs-review`.
