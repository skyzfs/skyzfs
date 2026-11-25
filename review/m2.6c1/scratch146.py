"""Sets up the scratch tests of the look at #146 in a clean worktree of
the head and runs them:
python3 -I scratch146.py <worktree> <out> [filter [engine-tests]]

- the fake (`engine/fake.rs`) gets an uploader that stops as skyzfsd's
  does (`Publishing::stop`): every publish pending ends `refused
  stopped`, kept for its name; every `publish` after is refused
  `stopped` at once and not kept;
- `plan146.rs` is appended to `plan/tests.rs`, and `rec146.rs` (the
  tests of cd3a67c) or the file named (`rec146b.rs`, those of 4ff4549)
  to `engine/tests.rs`;
- `cargo test -p skyzfs-agent --lib <filter> -- --nocapture
  --test-threads=1` (the filter `review5` by default; `review` runs the
  grid and the file's tests at 4ff4549); the output goes to <out>, the table's rows to
  <out>.tsv.

The worktree is left changed; use a scratch one."""
import os, subprocess, sys

W, OUT = sys.argv[1], sys.argv[2]
FILTER = sys.argv[3] if len(sys.argv) > 3 else "review5"
REC = sys.argv[4] if len(sys.argv) > 4 else "rec146.rs"
HERE = os.path.dirname(os.path.abspath(__file__))
FK = os.path.join(W, "crates/skyzfs-agent/src/engine/fake.rs")


def edit(path, old, new):
    text = open(path).read()
    if text.count(old) != 1:
        sys.exit(f"{path}: anchor found {text.count(old)} times: {old!r}")
    open(path, "w").write(text.replace(old, new))


edit(FK, "    pub away_until: u64,\n",
     "    pub away_until: u64,\n"
     "    /// The uploader stopped (review: `Publishing::stop`).\n"
     "    pub uploader_stopped: bool,\n")
edit(FK, "            away_until: 0,\n", "            away_until: 0,\n            uploader_stopped: false,\n")
edit(FK, "    /// Advance the clock by `d`, a second at a time",
     "    /// The uploader stops for good, as skyzfsd's `Publishing::stop`:\n"
     "    /// every publish pending ends `refused stopped`, and every\n"
     "    /// `publish` after is refused so at once (review).\n"
     "    pub fn stop_uploader(&mut self) {\n"
     "        self.uploader_stopped = true;\n"
     "        self.end_where(|_| true, Ended::Refused(Code::Stopped));\n"
     "    }\n\n"
     "    /// Advance the clock by `d`, a second at a time")
edit(FK, "        let refused = |code| Ok(Reply::Refused(code, String::new()));\n        let daemon = &mut self.daemon;\n",
     "        let refused = |code| Ok(Reply::Refused(code, String::new()));\n"
     "        if self.uploader_stopped {\n            return refused(Code::Stopped);\n        }\n"
     "        let daemon = &mut self.daemon;\n")
for src, dst in [("plan146.rs", "crates/skyzfs-agent/src/plan/tests.rs"),
                 (REC, "crates/skyzfs-agent/src/engine/tests.rs")]:
    with open(os.path.join(W, dst), "a") as h:
        h.write("\n" + open(os.path.join(HERE, src)).read())
r = subprocess.run(["cargo", "test", "-p", "skyzfs-agent", "--lib", FILTER, "--",
                    "--nocapture", "--test-threads=1"], cwd=W, capture_output=True, text=True)
out = r.stdout + r.stderr
with open(OUT + ".tsv", "w") as h:
    h.write("".join(l + "\n" for l in out.splitlines() if l.startswith("TABLE\t")))
with open(OUT, "w") as h:
    h.write("".join(l + "\n" for l in out.splitlines() if not l.startswith("TABLE\t")))
print(f"exit {r.returncode}")
