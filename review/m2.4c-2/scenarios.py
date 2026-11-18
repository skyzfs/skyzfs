#!/usr/bin/env python3
"""Run scenarios.rs against a scratch worktree of #118's head (e42ea6a).

    scenarios.py WORKTREE TARGET_DIR

The file is copied to bins/skyzfsd/tests/review_m24c2.rs, run as an
integration test of skyzfsd (one test at a time, output shown), and
removed again. The worktree must be clean before and is clean after.
The output goes to scenarios.out beside this file.
"""
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))


def main():
    tree, target = sys.argv[1], sys.argv[2]
    dirty = subprocess.run(["git", "-C", tree, "status", "--short"],
                           capture_output=True, text=True).stdout.strip()
    if dirty:
        sys.exit(f"the worktree is not clean:\n{dirty}")
    head = subprocess.run(["git", "-C", tree, "rev-parse", "HEAD"],
                          capture_output=True, text=True).stdout.strip()
    there = os.path.join(tree, "bins/skyzfsd/tests/review_m24c2.rs")
    env = dict(os.environ, CARGO_INCREMENTAL="0", CARGO_TARGET_DIR=target,
               RUST_BACKTRACE="0")
    out = [f"head {head}\n"]
    try:
        with open(os.path.join(HERE, "scenarios.rs")) as f, open(there, "w") as g:
            g.write(f.read())
        for features in ([], ["--features", "skyzfsd/test-hooks"]):
            cmd = ["cargo", "test", "-p", "skyzfsd", *features, "--test",
                   "review_m24c2", "--no-fail-fast", "--", "--nocapture",
                   "--test-threads=1"]
            out.append("$ " + " ".join(cmd) + "\n")
            try:
                run = subprocess.run(cmd, cwd=tree, env=env, capture_output=True,
                                     text=True, timeout=1500)
                keep = [line for line in (run.stdout + run.stderr).splitlines()
                        if not line.lstrip().startswith(("Compiling", "Finished",
                                                         "Running", "Blocking"))]
                out.append("\n".join(keep) + f"\nexit {run.returncode}\n\n")
            except subprocess.TimeoutExpired:
                out.append("TIMEOUT after 1500 s\n\n")
    finally:
        if os.path.exists(there):
            os.remove(there)
        with open(os.path.join(HERE, "scenarios.out"), "w") as f:
            f.write("".join(out))
    left = subprocess.run(["git", "-C", tree, "status", "--short"],
                          capture_output=True, text=True).stdout.strip()
    print("".join(out))
    print("worktree after:", left or "clean")


if __name__ == "__main__":
    main()
