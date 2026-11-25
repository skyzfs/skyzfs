"""Runs the scratch engine tests of the next look at #146 in a clean
scratch worktree (it is left changed; use one for this alone):

python3 -I search146.py <worktree> <out> <file.rs>[,<file.rs>...] [filter]

Each file beside this script is appended to `engine/tests.rs`, then
`cargo test --release -p skyzfs-agent --lib <filter> -- --nocapture
--test-threads=1` (filter `review7` by default) runs, and its output goes
to <out>, with the worktree's path shortened to `<wt>`."""
import os, subprocess, sys

W, OUT, FILES = sys.argv[1], sys.argv[2], sys.argv[3].split(",")
FILTER = sys.argv[4] if len(sys.argv) > 4 else "review7"
HERE = os.path.dirname(os.path.abspath(__file__))
dst = os.path.join(W, "crates/skyzfs-agent/src/engine/tests.rs")
for f in FILES:
    with open(dst, "a") as h:
        h.write("\n" + open(os.path.join(HERE, f)).read())
head = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=W,
                      capture_output=True, text=True).stdout.strip()
r = subprocess.run(["cargo", "test", "--release", "-p", "skyzfs-agent", "--lib", FILTER,
                    "--", "--nocapture", "--test-threads=1"],
                   cwd=W, capture_output=True, text=True, env=dict(os.environ))
out = (r.stdout + r.stderr).replace(W, "<wt>").replace(os.path.expanduser("~"), "~")
with open(OUT, "w") as h:
    h.write(f"head {head}; files {','.join(FILES)}; filter {FILTER}; exit {r.returncode}\n")
    h.write("".join(l + "\n" for l in out.splitlines()
                    if not l.strip().startswith(("Compiling", "Downloaded", "Downloading"))))
print(f"exit {r.returncode}")
