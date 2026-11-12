# M1.T11 (skyzfs/skyzfs#32): several builds of the daemon on one machine

Reviewed: e8746ae ... 2706f99 (skyzfs-testing-m1.t11). One look.

## What was done

- Read the whole diff (237 lines).
- Tried odd values of a ref against the two `git rev-parse` calls
  (`--all`, `-q`, `--short=7`, `--default=HEAD`, `HEAD:tests`, `-`,
  `--`, `..,`): all refused. `@` resolves to HEAD.
- Ran disk-sync.py on a disk and on a memory file system; ran
  test_daemon_check.py (passes).
- Not done: no workflow run; `summarize` not run on made-up rows; no
  artifact read.

## Findings: nothing that blocks

1. A ref cannot become a command or an option. The value's characters
   are letters, digits and `_.,:=+-`; it is always quoted; the first
   lookup has `origin/` before it and both end in `^{commit}`; what is
   used afterwards is the hash. A branch with `/` in its name cannot
   be named (a commit can).
2. One target directory for all refs: each build's binary is copied
   to a file of the ref's name right after its own build, and the
   step stops on a failed build. An entry gets the binary of the ref
   it names; the row records the ref and the binary's hash.
3. Notes: a `TB_SKYZFSD_REF=` with no value passes the run step's
   check and names a binary that is not there; two different
   `TB_SKZ_TIME_REF` in one sequence: the first wins, unsaid.
4. Judging: without a model row of the machine there is NO VERDICT as
   before; a model row of another machine is not in the list. Of
   several model rows of a sequence the lowest-numbered is judged
   against, the others say "not judged". The origin-delay rule still
   applies to the one judged.
5. `failed_first` is outside "could hold a step", so it cannot make
   the margin pass: with every volume in it the verdict is TOO_MANY.
   It is checked before `last_write` and `not_set`, so it can excuse
   such a volume; that only makes the count of those that could
   smaller.
6. disk-sync.py: its file is removed in a `finally`; the empty
   directory `disk-sync` stays on the work disk. The step's pipe ends
   in `tee` under the default shell (no pipefail), so the script's
   failure does not fail the job.
