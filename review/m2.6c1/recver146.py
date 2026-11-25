"""Item 2 of the look at #146: how agents read version 2 with `Stopped`
appended. python3 -I recver146.py <worktree at 4ff4549> <worktree at
8663cd7> <out>

Appends `recver146.rs` to `record/tests.rs` in each scratch worktree and
runs its tests (`cargo test -p skyzfs-agent --lib recver`): at 4ff4549
first, which prints the bytes of a record whose only abandoned entry is
`Stopped`; then at the base, with those bytes pasted in. The RECVER lines
go to <out>. The worktrees are left changed; use scratch ones."""
import os, re, subprocess, sys

NEW, OLD, OUT = sys.argv[1], sys.argv[2], sys.argv[3]
HERE = os.path.dirname(os.path.abspath(__file__))
SRC = open(os.path.join(HERE, "recver146.rs")).read()
lines = []


def run(w, src, label):
    with open(os.path.join(w, "crates/skyzfs-agent/src/record/tests.rs"), "a") as h:
        h.write("\n" + src)
    r = subprocess.run(["cargo", "test", "-p", "skyzfs-agent", "--lib", "recver", "--",
                        "--nocapture", "--test-threads=1"], cwd=w, capture_output=True, text=True)
    head = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=w,
                          capture_output=True, text=True).stdout.strip()
    got = [l for l in (r.stdout + r.stderr).splitlines() if "RECVER" in l or "test result" in l]
    lines.append(f"## at {head} ({label}), exit {r.returncode}")
    lines.extend(got)
    return got


got = run(NEW, SRC, "the head")
one = next((l.split(": ")[-1] for l in got if "one Stopped bytes" in l), None)
if one is None or not re.fullmatch(r"[0-9a-f]+", one):
    sys.exit("no bytes of a record with one Stopped entry")
run(OLD, SRC.replace('"ONE_STOPPED";', f'"{one}";'), "the base: an agent before the variant")
open(OUT, "w").write("\n".join(lines) + "\n")
print("\n".join(lines))
