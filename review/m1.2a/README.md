# Review notes for M1.2a (skyzfs-local write log, pull request 6)

`lost-superblock-after-reclaim.diff` adds one test to
`crates/skyzfs-local/src/log/tests.rs` at e747492. Apply it and run

    cargo test -p skyzfs-local --lib review_lost -- --nocapture

The test fills segments, reclaims twice, lets the log reuse two segment
files, reclaims again, reuses the file of a segment that lay between the
two newest starts, syncs, and then flips one bit in the newest superblock
copy. `Log::open` returns Ok with the records of an old, already
reclaimed segment and none of the synced ones, and erases the headers of
the live segments. The test prints what it finds; it asserts nothing.
