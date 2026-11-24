"""Textual plants against the head of #136 (f2fcc02), one at a time, each
file restored from its saved text after the run (try/finally). Each
suite runs in a process group of its own and is killed whole at its
limit. For each failed test the first panic's place and message are
kept, so that it shows which assertion caught the plant.
Run from a clean worktree of the head:
WORKTREE=<path> CARGO_TARGET_DIR=<dir> python3 plants.py [names...]
Results: KILLED (the tests that failed), SURVIVED, NOT APPLIED, BUILD ERROR."""
import json, os, re, signal, subprocess, sys, time

W = os.environ.get("WORKTREE", ".")
ENV = dict(os.environ, CARGO_INCREMENTAL="0")
LIMIT = int(os.environ.get("PLANT_LIMIT", "1200"))
SUITES = {
    "l": ["cargo", "test", "-p", "skyzfs-local", "--lib", "--", "inplace::tests::cuts::"],
    "L": ["cargo", "test", "-p", "skyzfs-local", "--lib"],
    "fs": ["cargo", "test", "-p", "skyzfs-local", "--lib", "--", "fs::tests::"],
    "d": ["cargo", "test", "-p", "skyzfsd", "--lib"],
}
CT = "bins/skyzfsd/src/control.rs"
CLI = "bins/skyzfsd/src/cli.rs"
IP = "crates/skyzfs-local/src/inplace.rs"
FS = "crates/skyzfs-local/src/fs.rs"

COMMIT = """        intent.map.commit(intent::Base {
            state: id,
            number,
            group,
        });
        intent.commits += 1;
        intent.map.clear(|first, _| {
"""

P = [
 # The description's five, written again from its table.
 ("FA (author) a release on a failed store begins no header sync", IP,
  "        let failed = self.failed;\n        self.begin_intent_sync();\n        failed || !self.failed\n",
  "        let failed = self.failed;\n        if failed {\n            return true;\n        }\n        self.begin_intent_sync();\n        failed || !self.failed\n", "l"),
 ("J (author) the drop of a file does not join its helper", FS,
  "        if let Some(thread) = self.thread.take() {\n            let _ = thread.join();\n        }\n",
  "        if let Some(thread) = self.thread.take() {\n            drop(thread);\n        }\n", "fs"),
 ("CE (author) the pause keeps doubling past 50 ms", CT,
  "                pause = (pause * 2).min(CONNECT_PAUSE);\n",
  "                pause = (pause * 2).min(CONNECT_PAUSE * 1000);\n", "d"),
 ("Q9 (author) the help's paragraph names --intent-header-sync", CLI,
  "      the volume. With --no-intent-header-sync (only with --hold-cuts)\n",
  "      the volume. With --intent-header-sync (only with --hold-cuts)\n", "d"),
 ("CF (author) the pause starts at 50 ms", CT,
  "    let mut pause = Duration::from_millis(1);\n", "    let mut pause = CONNECT_PAUSE;\n", "d"),

 # Mine.
 ("FAc a release on a failed store does not name the cut in the map (the bits are cleared, the header written and its sync begun as before)", IP,
  COMMIT,
  "        if !self.failed {\n            intent.map.commit(intent::Base {\n                state: id,\n                number,\n                group,\n            });\n            intent.commits += 1;\n        }\n        intent.map.clear(|first, _| {\n", "L"),
 ("FAp a poll of a failed store does not collect the end of the map's sync", IP,
  "    fn poll(&mut self, out: &mut Vec<Completion>) -> usize {\n        self.release_early(false);\n        self.settle_intent(false);\n",
  "    fn poll(&mut self, out: &mut Vec<Completion>) -> usize {\n        self.release_early(false);\n        if !self.failed {\n            self.settle_intent(false);\n        }\n", "L"),
 ("J2 the helper's drop lets go of its results before the join (the sync is made, the wake is not called)", FS,
  "        self.jobs = None;\n        if let Some(thread) = self.thread.take() {\n",
  "        self.jobs = None;\n        self.results = mpsc::channel().1;\n        if let Some(thread) = self.thread.take() {\n", "fs"),
 ("CE5 the pause is capped at 500 ms", CT,
  "                pause = (pause * 2).min(CONNECT_PAUSE);\n",
  "                pause = (pause * 2).min(CONNECT_PAUSE * 10);\n", "d"),
 ("CE8 the pause is capped at 400 ms", CT,
  "                pause = (pause * 2).min(CONNECT_PAUSE);\n",
  "                pause = (pause * 2).min(CONNECT_PAUSE * 8);\n", "d"),
 ("Q9u the usage's doc line names --intent-header-sync", CLI,
  "//!                [--intent-region-mib N] [--no-intent-header-sync]\n",
  "//!                [--intent-region-mib N] [--intent-header-sync]\n", "d"),
]


def run_suite(cmd):
    proc = subprocess.Popen(cmd, cwd=W, env=ENV, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            text=True, start_new_session=True)
    try:
        out, _ = proc.communicate(timeout=LIMIT)
        return proc.returncode, out, False
    except subprocess.TimeoutExpired:
        os.killpg(proc.pid, signal.SIGKILL)
        out, _ = proc.communicate()
        return None, out, True


def failed_of(out):
    return [l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.endswith("FAILED")]


def panics_of(out):
    """The first panic of each failed test: place and first line of message."""
    lines = out.splitlines()
    found = {}
    for i, l in enumerate(lines):
        m = re.match(r"thread '([^']+)' (?:\(\d+\) )?panicked at (\S+):$", l)
        if m and m.group(1) not in found:
            msg = " | ".join(x.strip() for x in lines[i + 1:i + 4] if x.strip() and not x.startswith("note:"))
            found[m.group(1)] = f"{m.group(2)} {msg[:300]}"
    return found


only = sys.argv[1:]
res = []
dirty = subprocess.run(["git", "status", "--short"], cwd=W, capture_output=True, text=True).stdout
if dirty.strip():
    sys.exit("the worktree has changes:\n" + dirty)
for name, f, old, new, suites in P:
    if only and not any(name.startswith(o + " ") for o in only):
        continue
    path = os.path.join(W, f)
    text = open(path).read()
    n = text.count(old)
    if n != 1:
        res.append((name, f"NOT APPLIED (found {n})"))
        print(f"{name}: NOT APPLIED ({n})", flush=True)
        continue
    planted = text.replace(old, new)
    t = time.time()
    verdict = "SURVIVED"
    try:
        open(path, "w").write(planted)
        for s in suites.split(","):
            code, out, timed_out = run_suite(SUITES[s])
            if timed_out:
                verdict = f"KILLED ({s}: no end within {LIMIT} s)"
                break
            if "error[E" in out or "could not compile" in out:
                verdict = "BUILD ERROR"
                print(out[-3000:])
                break
            if code != 0:
                failed = failed_of(out)
                pan = panics_of(out)
                verdict = f"KILLED ({s}): " + ", ".join(failed[:8]) + (f" and {len(failed) - 8} more" if len(failed) > 8 else "")
                for test in failed[:8]:
                    key = next((k for k in pan if k.endswith(test.split("::")[-1])), None)
                    if key:
                        verdict += f"\n    {test}: {pan[key]}"
                break
    finally:
        open(path, "w").write(text)
    res.append((name, verdict))
    print(f"{name}: {verdict} ({time.time() - t:.0f}s)", flush=True)
left = subprocess.run(["git", "status", "--short"], cwd=W, capture_output=True, text=True).stdout
print("worktree after:", left.strip() or "clean")
json.dump(res, open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "plants-result.json"), "w"), indent=1)
print("done")
