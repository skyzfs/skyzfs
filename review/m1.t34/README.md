# M1.T34 (#82): a --store a build does not take; exit 7 in the kill test

- `review-posted.md`: the review posted on #82 at head abf384d
  (https://github.com/skyzfs/skyzfs/pull/82#issuecomment-6874306530).
  Verdict: blocked by one finding. A daemon that ended with 7 by
  itself before a kill is reaped by bash at once, so the loop's
  `kill -9` fails and the loop breaks with "kill -9 failed" (the rig's
  ERROR) before the new check reads the status.
- `reap.sh`: bash reaps a background child that ended by itself (kill -0
  and kill -9 fail after it; `wait` still gives its status).
- `repro82.sh DAEMON_RIG`: runs the rig's own kill-loop lines (cut by
  text from `pid=$(server_pid` to `# KT_WAIT_S:`) with the rig's
  `server_pid`, `serve_wait`, `kt_refusal` and `kill_verdict`, against
  a made-up server that ends with 7 or 1 before the kill, or is alive.
  `repro82.txt`: its output at abf384d. No kernel, no root.
- `plants82.py`, `plants82.txt`: P1-P7 on `tests/lib/skz-store.sh` and
  its three callers, with `test_skz_store.sh` run as root; all caught.
