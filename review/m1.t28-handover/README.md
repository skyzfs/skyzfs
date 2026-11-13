# M1.T28 follow-up (pull request 66), head 0a78798 on a234314

The tool hands the suite the result of a call that returned before the
kill, through `Harness::completed_before_loss`.

## The driver's order in `kill` (the author's first question)

The order (account what was handed over, then tell the model of the
loss) is right, and it can be pinned. `order_probe.rs` is a test for
`conformance.rs`'s test module: two writes of one block in flight, the
in-memory target on its seeded schedule applying at submit, the harness
`HandsOver` of the pull request, and `Fault::SkipReissuedIfLogged`.
Over 200 seeds (`order-probe.txt`):

- at 0a78798 the fault is caught at 29 seeds;
- with the two lines swapped it is caught at none.

Why: when one write was answered before the loss and the other is
issued again, the second is applied after the first, so the block must
hold the second. A target that skips the reissue leaves the first. Told
of the loss first, the model takes both as issued again, in either
order, and accepts the first.

## The replayer and the kept traces (the second question)

`replay-kept.sh`: every volume of the three kept passes of run
43476467345 replayed with the tool at 8e8cb23 and at 0a78798
(`replay-kept.txt`): 179 volumes, 16 `before_kill` lines, no
`answered_before_kill` line, every output identical byte for byte and
every exit 0. In pass 7 all six `before_kill` ids are submitted again
later in the trace, which is what "dropped and resubmitted" means.

A result the tool collected always has its `done` line earlier in the
trace (the call's thread writes the line before it sends the result),
so the replayer finds it at `settled`.

## Plants

`plants66.py`: eight, results in `plants66.txt`.

Five of eight are caught. Not caught: the driver's order (the probe
above pins it), the replayer's same order, and the tool's trace still
saying `before_kill` (nothing reads the tool's own trace in a test).

## Second look, head 8a0dd14

Posted: can merge once CI is green. The order test is in
(`what_is_handed_over_is_accounted_before_the_loss`: a correct target at
all 200 seeds, the faulty one caught at least once); with the two steps
swapped it fails (`plants66-8a0dd14.txt`). The trace's word is one
constant for writer and replayer. The replayer's own order is still
held by nothing, and I did not ask for it.
