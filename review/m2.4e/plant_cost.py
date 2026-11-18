#!/usr/bin/env python3
"""plant_cost.py WORKTREE TARGET_DIR SIZES ID ...: R13's figures with
each plant in place (one at a time), and at the head ("head"). The file
is restored on any exit; the worktree is checked clean."""
import os
import sys
sys.path.insert(0, __import__("os").path.dirname(__import__("os").path.abspath(__file__)))
import plants3 as p

tree, target, sizes = sys.argv[1:4]
ids = sys.argv[4:]
test = os.environ.get("TEST", "review_m24e_r13")
cmd = ["cargo", "test", "-p", "skyzfs-local", "--lib", "--", test, "--nocapture"]
try:
    for pid in ids:
        with p.Edited(tree, p.TESTS, p.appended(["scenarios_f4_cost.rs"])):
            if pid == "head":
                outcome, out = p.run(cmd, tree, target, {"REVIEW_SIZES": sizes}, limit=1800)
            else:
                plant = next(x for x in p.PLANTS if x[0] == pid)
                with p.Edited(tree, plant[1], p.once(plant[2], plant[3])) as ok:
                    assert ok, pid
                    outcome, out = p.run(cmd, tree, target, {"REVIEW_SIZES": sizes}, limit=1800)
        for line in out.splitlines():
            if line.startswith(("R13", "R9")):
                print(pid, line, flush=True)
        print(pid, "outcome", outcome, flush=True)
finally:
    left = p.status(tree)
    if left:
        print("NOT CLEAN", left)
        sys.exit(3)
