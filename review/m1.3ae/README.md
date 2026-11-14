# M1.3ae (the home image's own file system against the write unit), pull request 75

Head a5f589f on 09959c2. Review posted on 2026-11-14 (issue comment
6871923326): nothing blocks; it can merge once CI is green.
`review-posted.md`. The lead put this package after M1.3t-b, from the
M1.3t review's finding on the untested check.

- `gates.txt`: fmt, clippy (plain and test hooks) and the tests of
  skyzfs-local and skyzfs-ublk: clean, rustc 1.100.0.
- `plants75.py`, `plants75.txt`: S1, S3, P1, P2 caught; S2 (a
  deallocation's reach on a file of its own) fails nothing.
- The sim keys a file's own block size by name, looked up at each use:
  a rename onto the name (create renames home.new onto home.img) would
  move the file to the other file system, which Linux never does
  (EBUSY onto a mount point; a rename onto a symlink replaces it).
- The direct-I/O check is a statx on the image's own descriptor
  (fs.rs:610): per-file by construction. The real directory opens with
  std::fs::OpenOptions, which follows a symlink.
- To do: the rigs on this head.

## Second look at 6fe1aba (posted: issuecomment-6872362561)

Both points settled. Plants (plants75b.*): S2, R1, R2, R3 caught; R4 inert
(inode numbers are not reused). Can merge once daemon rig 43559205689 reads
as expected; ci, coverage, e2e, control rig, replay through skyzfsd green.

Merged (#75). Daemon rig at 6fe1aba (run 43559205689), rebuilt and judged:
exit 0 on 24.04, 26.04, 26.04-arm, in-place; tables identical to 75c32a7's
(rig-6fe1aba/).
