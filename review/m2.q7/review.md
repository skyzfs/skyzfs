## Review of #133 at d040d95 (Q7, F-A and F-B of #124, the follow-ups of #131)

**Verdict: can merge. Nothing blocks.** I found no defect in the code. Q7 does what the lead decided, on every path I could find, and nothing turns the sync off for a volume that uploads unless `--hold-cuts` is given as well. The answer to F-A is sound: the sync a release begins on a failed store cannot outlive the store, write after its drop, leak its thread or harm a take-over. The G2 and G3 tests hold the plants I asked them to hold, U8 and U9 included, and I could not make them flake.

What I have is non-blocking: one gap in the tests that bears on F-A (N1), a gap in the uploader's tests now that the default has changed (N2), a bound in the G2 test that does not hold the pause's cap (N3), and text that is stale in the description, in the help and in files this pull request does not own (N4 to N6).

The head had not moved when I began or when I ended: `head.sha` is d040d95, base e155dbf. I read the diff against e155dbf first, then my reviews of #124 and #131 and the author's answer on #131, then the description. Codex's code review and security review of d040d95 are complete, with no finding and no comment on the pull request, so there is nothing of Codex's to judge.

DRAFT
