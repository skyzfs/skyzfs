"""Append the review's scratch tests to a worktree of the head:
python3 apply.py NOTES_DIR WORKTREE"""
import sys
notes, tree = sys.argv[1:3]
for src, dst in [("scratch-program-tests.rs", "crates/skyzfs-agent/src/program/tests.rs"),
                 ("scratch-retain-tests.rs", "crates/skyzfs-agent/src/engine/retain/tests.rs")]:
    with open(f"{tree}/{dst}", "a") as f:
        f.write("\n" + open(f"{notes}/{src}").read())
