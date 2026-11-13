# What the ublk driver returns for a fetch or a commit (read for pull request 64)

`drivers/block/ublk_drv.c` at v6.11, v6.17 and v7.0, from
raw.githubusercontent.com/torvalds/linux. The daemon's queue asks for
`UBLK_F_USER_RECOVERY | UBLK_F_USER_RECOVERY_REISSUE` only
(`crates/skyzfs-ublk-sys/src/lib.rs`, `FLAGS`): no user copy, no
`NEED_GET_DATA`, no automatic buffer registration, no zones, no batches.

## A refused commit completed nothing: true on all three

- v6.11 `__ublk_ch_uring_cmd`: every `goto out` of the commit's case
  comes before `ublk_commit_completion`; after it only `ublk_prep_cancel`
  and `-EIOCBQUEUED`.
- v6.17 `__ublk_ch_uring_cmd` and v7.0 `ublk_ch_uring_cmd_local`: the
  checks, then `ublk_config_io_buf`, then `__ublk_complete_rq`, then
  `if (ret) goto out`. `ublk_config_io_buf` returns non-zero only through
  `ublk_handle_auto_buf_reg`, that is with automatic buffer
  registration. Without it the commit cannot be refused after the
  completion.
- The abort (`UBLK_IO_RES_ABORT`, -19) comes from `ublk_cancel_cmd`
  only, for a command that is waiting; a commit completed its request
  when it was submitted.

## Which result for which mistake

A tag that is served is in one of two states (v6.17 lines 1174, 1180,
2119): a command waits on it (`ACTIVE`), or the server holds its request
(`OWNED_BY_SRV`). The hand-over of a request sets the second and clears
the first; a taken commit does the reverse.

| mistake | v6.11 | v6.17, v7.0 |
|---|---|---|
| a second commit of a tag (its first commit's command waits) | `-EBUSY` (the `ACTIVE` check comes first) | `-EBUSY` (not `OWNED_BY_SRV`) |
| a commit of a tag never fetched | `-EINVAL` (not `OWNED_BY_SRV`) | `-EINVAL` (`io->task != current`) |
| a commit from another thread | `-EINVAL` | `-EINVAL` |
| a read's commit with no buffer address | `-EINVAL` | `-EINVAL` |
| tag or queue out of range | `-EINVAL` | `-EINVAL` |
| a fetch after the queue is ready | `-EBUSY` | `-EBUSY` |
| a fetch of a tag already fetched, queue not ready | `-EBUSY` (`ACTIVE`) | `-EINVAL` |
| a legacy command code on a kernel without them | `-EOPNOTSUPP` | `-EOPNOTSUPP` |

So the result does not tell "the tag held no request" from "the commit
was malformed" on any of the three: `-EINVAL` covers both.

## Where the pull request's text is not right

"No command is outstanding on that tag afterwards, so no request comes
on it again until the queue stops" (`pump.rs:181`, `record.rs:60`, the
description's table). True for a malformed commit: the server holds the
request and nothing waits. Not true for a commit of a tag that holds no
request while an earlier command waits, which is every `-EBUSY` of a
commit: the refusal returns before `ublk_fill_io_cmd`, the earlier
command stays, and the next request comes on the tag as before.

Results that are io_uring's and not the driver's (a bad descriptor, a
bad address) also end a command otherwise than OK and are counted as
failed. The command never reached the driver, so they also completed
nothing.

## The library's user data

libublk 0.4.8 `io.rs`: `build_user_data(tag, op, tgt_data, is_target_io)`
puts the tag in bits 0 to 15, `op & 0xff` in bits 16 to 23 and bit 63
for a target's own I/O; `queue_io_cmd` (line 1919) builds every fetch
and commit of the path the daemon uses with it. `ended()` reads it back
with the library's `user_data_to_tag` and `user_data_to_op`. The low
byte of the ioctl-encoded code is the legacy code (0x20 fetch, 0x21
commit, 0x22 get data). The library waits with a time-out in the call's
arguments and puts no time-out entry on the ring, so the only entries
without bit 63 are the driver's commands.
