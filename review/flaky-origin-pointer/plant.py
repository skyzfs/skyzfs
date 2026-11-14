#!/usr/bin/env python3
"""Plant: widen reconcile's window between reading the row and the swap.
Usage: plant.py TREE apply|restore"""
import subprocess, sys
tree, what = sys.argv[1], sys.argv[2]
path = f"{tree}/crates/skynbd-nbd/src/origin.rs"
old = "        Some(r) => repoint(registry, b, r.commit).await,\n"
new = ("        Some(r) => {\n"
       "            tokio::time::sleep(std::time::Duration::from_millis(100)).await;\n"
       "            repoint(registry, b, r.commit).await\n"
       "        }\n")
if what == "apply":
    t = open(path).read()
    assert t.count(old) == 1, "anchor"
    open(path, "w").write(t.replace(old, new))
    print("planted")
else:
    subprocess.run(["git", "checkout", "--", "crates/skynbd-nbd/src/origin.rs"], cwd=tree, check=True)
    print("restored")
