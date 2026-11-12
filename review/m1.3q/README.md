# M1.3q (skyzfs/skyzfs#38): the ublk features the daemon needs, and their kernels

`skyzfs-coding-m1.3q` at 0ddab81, three commits on `zfs-topology` dd4fcf8.

**Verdict: one finding blocks. The release the start enforces for the
asynchronous delete, 6.9, is the header's; the drivers of 6.9 and 6.10
cannot execute the command. The first release whose driver does is 6.11.
The rest of the table, the start, the detach, the parse and the messages
are right.**

## What I did

- Read the diff before the description (the description was not read).
- Read the kernel myself, from the release tags of github.com/torvalds/linux
  (a git fetch of each tag): `include/uapi/linux/ublk_cmd.h` at v6.0, v6.1,
  v6.3, v6.4, v6.5, v6.8, v6.9; `drivers/block/ublk_drv.c` at v6.1, v6.3,
  v6.4, v6.5, v6.8 to v6.14.
- Read libublk 0.4.8 as the build uses it (`src/io.rs`, `src/ctrl.rs`) and
  the daemon's binding for every flag and command sent.
- Tests of `skyzfs-ublk`, `skyzfs-ublk-sys`, `skyzfsd`: all pass (138, 51
  and the small suites; 9 kernel tests ignored here).
- Four plants, each caught and removed: a detach with no record still asks
  the driver; 6.9 itself refused; the override also skips the features; an
  unreadable release passes.

## What I did not do

- Nothing ran on a kernel: no ublk here. The kernel's behaviour is from its
  source.
- The stable point releases (6.9.y, 6.10.y) are not in that repository and
  were not read; v6.2 and v7.0 were not read either.
- No clippy or fmt. CI runs were not looked at.

## The lead's questions

1. **The table.** Checked independently:
   - `UBLK_F_USER_RECOVERY`, `_REISSUE`: not in v6.0's header, in v6.1's. Right.
   - `UBLK_F_CMD_IOCTL_ENCODE`: not in v6.3's, in v6.4's. Right.
   - `UBLK_U_CMD_GET_FEATURES`: not in v6.4's, in v6.5's; the v6.5 driver
     answers it before it looks for a device, with the three flags. Right.
   - `UBLK_U_CMD_DEL_DEV_ASYNC`: not in v6.8's header, in v6.9's. **The
     driver is another matter: finding 1.**
   - Nothing is missing from it. The daemon sends ADD_DEV, GET_DEV_INFO,
     GET_PARAMS, SET_PARAMS (6.0), START and END_USER_RECOVERY (6.1),
     GET_FEATURES, DEL_DEV_ASYNC, all ioctl-encoded (6.4). The library sends
     START_DEV and the queue's FETCH and COMMIT_AND_FETCH (6.0), ioctl-encoded
     when the device reports the flag (`io.rs:1524`, `:1843`). It asks for
     no flag of its own, and its quiesce and update-size calls are not on
     the daemon's paths.
2. **The start.** The check is the first statement of `start_with`. Before
   it `serve` only makes its run directory and stats the control device. No
   device is added or taken over and the volume's directory is not written.
   A take-over on the same boot is refused only on a kernel below the
   floor; see finding 3.
3. **The detach.** After the lock and after the record of this boot is
   read; with no such record it returns before the driver is asked (planted
   and caught by `detach_touches_nothing_it_may_not`).
4. **The parse.** Right, and its tests fail for each plant I made.
5. **The messages.** Each refusal names what is missing, its release and,
   for the delete, the override; none says "not supported". One message
   outside the check does not: finding 2.

## Findings

1. **Blocking. The 6.9 and 6.10 drivers do not execute the asynchronous
   delete.** `features.rs`, `DELETE_KERNEL = (6, 9)` and `OLDEST_KERNEL`.
   - v6.9 `drivers/block/ublk_drv.c:2890` switches on `_IOC_NR(cmd_op)`,
     which is 0x14 for this command; the case at `:2907` is
     `UBLK_U_CMD_DEL_DEV_ASYNC`, the whole ioctl number 0x80207514. It
     cannot match, and the command falls to `default: ret = -ENOTSUPP`
     (`:2926`). v6.10 is the same (`:2907`).
   - v6.11 defines `UBLK_CMD_DEL_DEV_ASYNC _IOC_NR(...)` (`:52`) and
     matches it (`:2911`); so do v6.12 to v6.14.
   - What goes wrong: on Linux 6.9.0 or 6.10.0 the start passes the check
     and serves. When the write log fails, the delete is answered with
     errno 524, the device stays, and the daemon cannot give it up: the
     case the check was written to refuse. The tests assert that "6.9.0",
     "6.9" and "6.10-rc1" serve.
   - Sure of the source; not run. A later 6.10.y may carry the fix; the
     first two numbers of a release cannot tell.
   - I drop it when the floor is 6.11 in the constant, the messages, the
     help and the tests, and the module says that the header has the name
     from 6.9 and the driver executes it from 6.11.
2. **Not blocking. A delete that the driver does not know says "Unknown
   error 524".** `raw.rs:189-191`, passed on by `lifecycle.rs:1209`. With
   the override given wrongly (or on 6.9 and 6.10) that is all the operator
   gets. It should name the command and the kernel, as the refusals do.
3. **Not blocking, the lead's decision at work.** A daemon with this check
   refuses to take over a waiting device on a kernel below the floor, where
   the daemon before it served it; the pool then waits until the override
   is given. The refusal comes before the attachment record is read, so its
   message cannot say that a device of this volume is waiting.
4. **Not blocking. Two sentences say more than is so.**
   - `features.rs`, "the driver looks the device up before it matches the
     command": true of v6.3 and v6.4. In v6.1 nothing is looked up; the
     answer is preset to `ENODEV` and the switch's default keeps it. The
     same answer.
   - The help, "Before serve or detach touches anything": `serve` makes its
     run directory first, and `detach` takes the lock and reads the record
     first.
5. **Low.** `raw::features` opens the control device itself; an `ENODEV`
   from that open (a node left behind with the driver not loaded) reads as
   "the driver cannot say its features".

## Suggestion

A probe can tell what the release cannot. Add a device that is never
started, send it the asynchronous delete: 0 says the driver has it, 524
that it has not (the plain delete then removes it, nobody holds it). That
would cover backports and fixed point releases without the operator's
word. Not run; it adds and removes a device at each start, which the
"touches nothing" rule may not allow.
