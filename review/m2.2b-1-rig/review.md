## Review of #102 at 345f17e

**Verdict: the review says it can merge, once CI is green on the head.** I found no defect. One statement in the description is not exact (R1), and it changes nothing in the code.

### Your three questions

1. **Whether another refused-start pin should be widened: no, but one more pin does compare the volume's files.**
   - `volume_on_larger_blocks` (`daemon-rig.sh:919` to `:931`) digests every file of the copied volume before and after a refused serve, and fails on any change.
   - It needs no widening. That refusal comes in the open before the token is written (the unit checks in `Opened::open_unlocked` at #100's head), and the pin passed in all three daemon jobs of #100's run 43687498505 with "the copy's files after it: as before".
   - I found no other comparison of a volume's files under `tests/kernel`, `tests/lib` or `tests/bench`.
2. **Whether forbidding the token's removal is right: yes.** Nothing in the daemon removes it. A missing token is safe for the trust rule (the next start finds none, so the state's token does not match and a new state begins), but it would cost the held cuts, and no path should do it.
3. **The judge's tests: they hold the change.** I planted seven changes in `gj_files_judged`, one at a time (`plants.py`, `plants.out`), and each fails at least one test:
   - the token has no rule of its own;
   - the token may go as well;
   - the token is allowed by a pattern and not by its name;
   - the two lists are taken the other way round;
   - the token is put beside the status file in `GJ_MAY_CHANGE`;
   - a token that only appeared is named as removed;
   - an arm before the image's takes every name.

### What I checked

- **The judge's tests pass here:** `python3 tests/kernel/test_daemon_check.py`, 93 tests.
- **The dispatch on #100's daemon** ([run 43691419800](https://github.com/skyzfs/skyzfs/actions/runs/43691419800), `daemon_ref` 07a9f2d, ubuntu-26.04, check g): the daemon job passed. Its line reads `pin_data_journal_refused: PASS`, with `volume.start` in both lists, so the token was rewritten by the refused start and the judge let it through. On #100's own run the same line was the only one that differed from the table.
- **The name** `volume.start` is the daemon's (`START_NAME` in `volume.rs` at #100's head).
- **CI on 345f17e** was still running when I posted: 16 checks passed, 13 in progress, and 9 cancelled, which I take to be runs that the pull request's own runs replaced. Please confirm it green before the merge.

The files are in `review/m2.2b-1-rig/` on `skyzfs-review`.

### R1 (remark, does not block): the description says no other pin compares the volume's files at a refused start

`volume_on_larger_blocks` does, as above. It is right that it stays strict. It is worth a sentence in the rig's comment beside `GJ_START_TOKEN`: that pin will fail if the token's write ever moves ahead of the open's unit checks, and whoever sees it fail should know that it is a second place where this rule is held.
