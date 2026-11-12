# M1.T13 (skyzfs/skyzfs#35): an orderly stop is given the kill's wait

Reviewed: 728d9a9 ... 90ca4da (skyzfs-testing-m1.t13). One look.

- Can merge. Nothing found that blocks.
- The claim holds in the daemon's code: a start, a detach's settle and
  the giving up of a device each look again at a device that still
  reads live, `LIVE_RETRIES` = 50 times, 100 ms apart, 5 s
  (bins/skyzfsd/src/serve.rs; `settled` in lifecycle.rs). So nothing
  the daemon does needs the device to wait at the moment its server's
  process ends.
- Neither pin can pass where it should fail: the wait ends only on
  the state wanted or at 5 s, and the pin still asks for that state
  and the block device after it. A device that never waits, or is
  deleted, is FAIL after 5 s.
- The new observation is written on every way out of the control
  rig's function that comes before it; "not run" there is the rig's
  error, as for the kill's.
- The tables' texts say what the rigs do. Both judges' tests pass.
- What is less: a device that waits only after, say, 4 s passes. The
  time is in the pin's text and in an observation, not judged.
- Not done: no kernel run.
