# #84 (M1.3ah-a) at d63eb6c

- `review-posted.md`: the review posted on #84.
- `gates84.sh` / `gates84.txt`: fmt, clippy and tests at the head.
- `plants84.py` / `plants84.txt`: the plants, one at a time (A8, the
  fetches in reverse order, did not build and is not in the review).
- The library compared against is libublk 0.4.8 from the cargo registry:
  `io.rs` `__queue_io_cmd_no_state`, `submit_fetch_commands`,
  `regiser_io_bufs`, `flush_and_wake_io_tasks`, `wait_for_buffer_registration`.
