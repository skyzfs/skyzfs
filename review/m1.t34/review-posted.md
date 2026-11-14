## Review of M1.T34 at abf384d

**One finding blocks the merge: the new check for a 7 between kills is not reached in the case it is for.** The store options are right: nothing in the table is let through to a current build, and an older build is refused only what this PR means to refuse. `test_skz_store.sh` cannot get past a broken check to touch the machine.

I read the diff before the description. At abf384d these pass:
- `test_skz_store.sh`, run as root here;
- `test_daemon_check.py` (76 tests).

I planted seven changes (table below), and ran a reproducer of the finding against the rig's own lines.

### Blocking: a daemon that ended with 7 before a kill gives "kill -9 failed", the rig's ERROR

**Where.** `tests/kernel/daemon-rig.sh`, the kill loop in `kill_test`. The line `kill -9 "$pid" 2>/dev/null || { KT_ERR="kill $n: kill -9 $pid failed"; break; }` comes before the new check.

**Why the check is not reached.** The description says that before this change "the next `kill -9` succeeded, since the process was dead but not yet reaped". That does not hold:
- bash reaps a background child as soon as it ends, while it waits for any foreground command (the loop's `sleep`s).
- After that, `kill -0` and `kill -9` on its pid fail with "no such process". `wait` still returns its status.
- I checked this with bash 5.2.21. `serve_wait` itself depends on it: its `kill -0` would never fail on an unreaped process.

**The scenario.**
1. The daemon says `ready` after a kill and serves.
2. The driver then hands over a tag whose request is still out. The pump checks every hand-over, at each turn, for as long as it serves (`take_over` at `pump.rs:720`, called from every turn at `pump.rs:659`). So the daemon ends with 7.
3. At the next kill, `server_pid` gives the dead daemon's pid, and `kill -9` fails.
4. The loop breaks with `KT_ERR="kill N: kill -9 PID failed"`, and `KT_REFUSED` stays empty.
5. `kill_verdict pin` gives ERROR, the rig's own error.

The 7 is never read: the very loss the comment above the new check describes. The check fires only in a race of microseconds, when the daemon ends after bash last reaped and before the kill.

**Reproducer.** It cuts the rig's own lines of the loop, from `pid=$(server_pid` to the new check, out of `daemon-rig.sh` by text. It runs them with `server_pid`, `serve_wait`, `kt_refusal` and `kill_verdict` from the rig, against a made-up server. No kernel and no root are needed.

| the made-up server | at abf384d | with the change below |
|---|---|---|
| ends with 7 before the kill | `KT_ERR='kill 1: kill -9 PID failed'`, no refusal, verdict **ERROR**; `wait` still gives 7 | "before kill 1: the server ended with exit 7: …", verdict **FAIL** |
| ends with 1 before the kill | "kill -9 failed", ERROR | "kill -9 PID failed (the server had ended by itself, with exit 1)", ERROR |
| alive at the kill | status 137, no refusal, PASS | the same |

Each row is the same in two runs. The script is in my notes on `skyzfs-review` (`review/m1.t34/repro82.sh`).

**Why it blocks.** The run does not go green either way, since ERROR is not PASS. But the case the PR names, with the lead's ruling behind it, ends as the rig's error and loses the daemon's 7. `test_the_kill_loop_looks_for_a_7_between_kills` checks only the text, so it cannot see this.

**One way to fix it.** Read the status when the kill fails. This is the change I tried:

```bash
        if ! kill -9 "$pid" 2>/dev/null; then
            # A server that ended by itself is reaped by the shell at
            # once, so the kill fails; wait still gives its status.
            if [ -n "$SERVE_PID" ] && ! kill -0 "$SERVE_PID" 2>/dev/null; then wait "$SERVE_PID"; SERVE_RC=$?; SERVE_PID=""; fi
            if [ "$SERVE_RC" = "$EXIT_BROKEN_RULE" ]; then
                KT_REFUSED=$(kt_refusal "$n" "$SERVE_RC" "$dir.serve.log" before)
                KT_ERR="before kill $n: the server had ended by itself, with exit $SERVE_RC"
            else
                KT_ERR="kill $n: kill -9 $pid failed${SERVE_RC:+ (the server had ended by itself, with exit $SERVE_RC)}"
            fi
            break
        fi
```

The check after `serve_wait` can stay for the race. A test that runs the loop's lines, as the reproducer does, would hold this where the text test cannot.

### What you asked me to judge

**1. `skz-store.sh`'s table. It is right.**
- **Spellings:** both forms of `--store` (a separate word, and `=`), a `--store` with no value, `--store=` (empty), and both forms of every log store option. Each is refused for a current build. For an older build, everything but `in-place` is refused.
- **An older build is refused only what this PR means to refuse:** `--store` with another value, and the log store's options.
  - `--store in-place` is taken, and a named one is not added twice (P4).
  - A build from before the option existed has no `[--store` in its help. It is read as current, so `--store in-place` is refused, which is right: it does not take it.
  - A build whose help fails is read as current too, which errs on the safe side.
- **A bench row with `TB_SKYZFSD_REF`.** The workflow points `TB_SKYZFSD` at that row's build before the script starts (`topology-bench.yml:406`). So the early check reads the right build's help.
- **The hole and replay rigs never read `SKZ_STORE_ADD`.** They rely on the workflow's own `cli.rs` probe to add `--store in-place` for an older build, as before. That is consistent, not a gap.
- **Suggestion.** The test named "a value that looks like an option is a value" expects a refusal. The trailing `--store` is read as an option with no value. The name says the opposite of what it checks.

**2. The status read between kills.**
- **Yes, the status is the dead server's own, when the check is reached.** `serve_wait` reaps `SERVE_PID`, the serve started last. For the daemon that is the killed process: 137. For the stand-in it is its serve script: 1.
- **No, neither 1 nor 137 can be taken as 7.** The comparison is exact. The stand-in's statuses are 0 to 5 only (`skyzfsd-ref.sh`).
- **A stale 7 cannot be read either.** `serve_start` resets `SERVE_RC` at each serve, and a `serve_wait` that times out leaves it empty.
- **But the check is not reached in the case that matters** (above).

**3. Can `test_skz_store.sh`, with a check broken, get past it and touch the machine? No.** With each caller's check gone:
- **The replay rig.** Before its check it only makes `OUT` and `WORK`, under the test's temporary directory. Past the check it sets its cleanup trap and calls the made-up `modprobe`, which leaves a mark. It then stops at the missing `arcstats`, or at the made-up `dmsetup targets`, with 64. The cleanup only calls made-up `zpool` and `dmsetup` and reads `dmesg`.
- **The hole rig.** It calls the made-up `modprobe`, which leaves a mark, then stops at the made-up `dmsetup targets`.
- **The bench.** It stops at `no-such-topology` with 64, before `useradd` and `mkdir`. Nothing before its topology loop has a side effect.
- **No tool is called by an absolute path,** and `gnu-dd.sh` only picks a `dd`.
- **The marks matter.** P5 moves the replay rig's check after its `modprobe`. The rig still ends with 2, and only "no caller ran anything" fails.

### Planted changes

One at a time, with `sudo bash tests/lib/test_skz_store.sh` after each:

| plant | result |
|---|---|
| P1: the help probe never finds `[--store` | 4 checks fail (an older build's add and its `--store in-place`) |
| P2: the value of `--store` is not checked | 5 checks fail |
| P3: a log option is matched only without `=VALUE` | 10 checks fail |
| P4: a named `--store in-place` is not remembered | 2 checks fail (it would be added twice) |
| P5: the replay rig's check comes after its trap and `modprobe` | "no caller ran anything" fails (the made-up `modprobe` left its mark) |
| P6: the hole rig ends with 64, not 2 | "the hole rig: the rig's own error" fails |
| P7: the bench's early check is removed | "and says why" fails (the row still ends with 64, at the unknown topology) |

### CI on abf384d, when I wrote this

- **Green:** the control rig's three kernel jobs (43573567459) and the kernel probe's two image jobs (43573567509).
- **Still running or queued:** the daemon rig, the replay rig, ci.yml, e2e and coverage.

---
_Generated by [Claude Code](https://claude.ai/code)_
