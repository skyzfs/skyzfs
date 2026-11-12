# M1.3i (skyzfs/skyzfs#27): a part not taken at once stops a debug build

Reviewed: `origin/zfs-topology` 58e1b54 ... 530bd7c. Approve.

- fmt, clippy clean; the store's 197 tests pass, and the assertion
  never fires in them.
- Plant, no take after the explicit checkpoint: now 3 tests fail (the
  new one and both conformance suites), 243 assertion messages.
- Plant, no take after the rule's step: 6 tests fail, 177 messages.
- No test hung in either plant. A test that waits on a served store
  whose worker has stopped says "no answer" after its 20 s, or a count
  that is short; the assertion's own message is in the same output,
  above it.
- The assertion holds in every path by reading: a span is closed
  before each of the three places that can note parts, parts of one
  take follow each other, and nothing notes parts before the worker
  exists (the open's replay has `parts` at `None`).
