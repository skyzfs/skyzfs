# M1.3a (skyzfs/skyzfs#10) at 5716df5: review notes

Held against `docs/skyzfs-design.md` §4.2 at zfs-topology 2551937.

- `probes.diff`: five tests added to the pull request's own test files
  (apply to 5716df5; `cargo test -p skyzfs-ublk probe -- --include-ignored`).
  Four fail and show a finding each; the fifth passes.
  - `probe_the_position_is_the_one_stored_when_the_lock_was_got`: the
    acknowledged position is read once, before the lock is tried; a start
    that waits for the lock opens with the position as it first saw it.
  - `probe_a_detach_overtaken_by_a_start_deletes_no_served_device`: a
    `detach` that has looked deletes the device a daemon has since taken
    over and serves.
  - `probe_a_detach_overtaken_by_a_start_leaves_the_new_record`: a
    `detach` removes the attachment record a daemon has just written; after
    the next kill the volume has two devices.
  - `probe_a_gone_target_with_a_request_waiting_is_reported_as_gone`: with
    a target that answers "busy" once its worker is gone, as `Served`
    does, the pump reports `TargetFault`.
  - `probe_chains_of_kills_starts_and_detaches` (passes): 20,000 chains of
    eight steps: starts and detaches killed at any step, clients holding
    devices, strangers' devices (twins and others, with and without
    another volume's note), a lost run directory.
- `probes.out`: their output.
- `planted-defects.py`, `planted-defects.out`: 59 defects where a rule has
  no crate-private switch. 51 are noticed by a test. Not noticed: a failed
  volume deleting by a record of another boot; this volume's noted device
  that is deleted and still held being taken over; the three syncs of the
  attachment record (no test with real files can show them); a completion
  nobody waits for; a tag taken twice; and one that is equivalent (an
  orderly stop and a flush waiting for room).
