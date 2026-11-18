#!/usr/bin/env python3
"""Plants and scenarios for the review of #125 (M2.4e) at 0f56669.

    plants.py WORKTREE TARGET_DIR plants [ID ...]
    plants.py WORKTREE TARGET_DIR scenarios
    plants.py WORKTREE TARGET_DIR scenarios-with ID ...

`plants`: one change at a time is written into a worktree of the head,
the tests of the crate it is in are run, and the file is restored (also
on an error: try/finally). A plant is

- KILLED       when a test of the pull request's tree fails with it (or
               the run is cut at its limit, which the result says),
- SURVIVED     when every test passes with it,
- NOT APPLIED  when its text is not found exactly once, or it does not
               build.

For a plant that survives and names a kind of scenario, the review's
scenarios of that kind are run as well, and the result names those that
fail with it.

`scenarios`: the scenarios at the head (all pass there): those of the
state (scenarios_state.rs, appended to the state's test module) and the
upload's (scenarios_upload.rs, appended to the uploader's holes tests).
`scenarios-with`: the scenarios with one plant in place.

Results are appended to plants.out / scenarios.out as they come. The
worktree must be clean at the start and is checked clean at the end.
"""
import os
import re
import signal
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))

INPLACE = "crates/skyzfs-local/src/inplace.rs"
STATE = "crates/skyzfs-local/src/inplace/state.rs"
STATE_TESTS = "crates/skyzfs-local/src/inplace/state/tests.rs"
FS = "crates/skyzfs-local/src/fs.rs"
DRIVER = "crates/skyzfs-upload/src/driver.rs"

# L: the store's library tests (state model, store on the simulated
# disk, real files). LT: the store's tests with the test hooks, the
# integration tests on real files among them. P: the uploader's tests.
L = ["cargo", "test", "-p", "skyzfs-local", "--lib", "--no-fail-fast"]
LT = ["cargo", "test", "-p", "skyzfs-local", "--features", "test-hooks", "--no-fail-fast"]
P = ["cargo", "test", "-p", "skyzfs-upload", "--no-fail-fast"]
SCEN = ["cargo", "test", "-p", "skyzfs-local", "--lib", "--no-fail-fast", "--", "review_m24e", "--nocapture"]
SCEN_U = ["cargo", "test", "-p", "skyzfs-upload", "--test", "holes", "--no-fail-fast", "--", "review_m24e"]
HOLES_TESTS = "crates/skyzfs-upload/tests/holes.rs"
# Where each kind of scenario is appended, and how it is run.
SCENARIOS = {
    "state": (STATE_TESTS, "scenarios_state.rs", SCEN),
    "upload": (HOLES_TESTS, "scenarios_upload.rs", SCEN_U),
}
LIMIT = 1500

WHOLE_MARK = ("        if self.held().is_empty() {\n"
              "            self.store(at::WHOLE, self.epoch());\n"
              "        }\n")

# (id, file, old, new, what the plant is, tests, scen)
PLANTS = [
    # ---- a mismatch while the set holds (abandon_all)
    ("X01", STATE, WHOLE_MARK,
     "        if self.held().is_empty() {\n"
     "            let open = self.open().first;\n"
     "            self.maps.clear(&mut self.region, open);\n"
     "            self.store(at::WHOLE, self.epoch());\n"
     "        }\n",
     "abandon_all clears the open map before it marks the open epoch whole (harmless before "
     "M2.4e: a whole cut's map was never read; now a lo-0 whole cut reads holes by it)", [L], "state"),
    ("X02", STATE, WHOLE_MARK,
     "        if self.held().is_empty() {\n"
     "            self.store(at::WHOLE, self.epoch());\n"
     "            if let Some(base) = self.holes_base {\n"
     "                self.store(base, Holes::Dropped.code());\n"
     "            }\n"
     "        }\n",
     "abandon_all drops the hole set (conservative: the next whole cut reads and copies every hole)",
     [L], "state"),
    # ---- the begin and the take-over
    ("X03", STATE,
     "            region.store(base + 1, chunks);\n            region.store(base, Holes::Held.code());\n        }\n"
     "        region.store(at::MAGIC, MAGIC);\n",
     "            region.store(base + 1, chunks);\n        }\n"
     "        region.store(at::MAGIC, MAGIC);\n"
     "        if let (None, Some(_), true) = (changed, holes, version >= 3) {\n"
     "            region.store(holes_base(blocks, capacity), Holes::Held.code());\n        }\n",
     "the set's word Held is stored after the state's magic (a death between leaves a state "
     "with no set: conservative)", [L], None),
    ("X04", STATE,
     "        self.free_unneeded(None);\n        self.drop_unneeded_holes();\n    }\n\n    // ---- records ----\n",
     "        self.free_unneeded(None);\n    }\n\n    // ---- records ----\n",
     "a take-up (settle) does not drop a set no cut needs", [L], None),
    # ---- the reads
    ("X05", STATE,
     "        self.in_parts(held.parts, block)\n            || (self.is_whole(&held.cut) && !self.hole_of(&held.cut, block))\n",
     "        self.is_whole(&held.cut) && !self.hole_of(&held.cut, block)\n            || (!self.is_whole(&held.cut) && self.in_parts(held.parts, block))\n",
     "reads_content forgets the map for a whole cut (a hole written before the instant is "
     "not copied when overwritten)", [L], None),
    ("X06", STATE, "                Some(Some(next)) if next > block => Some(next),\n",
     "                Some(Some(next)) => Some(next),\n",
     "a slice reuses a next block of the map it passed", [L], "state"),
    ("X07", STATE,
     "        (chunk.min(chunks) * HOLE_BLOCKS).min(self.blocks)\n",
     "        chunk.min(chunks) * HOLE_BLOCKS\n",
     "a run of holes at the volume's end goes past it (a last chunk in part)", [L], None),
    # ---- the hole scan
    ("X08", INPLACE, "                Err(_) => return None,\n",
     "                Err(_) => {\n                    hole(at, end);\n                    at = end;\n                }\n",
     "a failed or unsupported SEEK_DATA takes the rest of the image for a hole", [LT], None),
    ("X09", INPLACE, "                Ok(None) => {\n                    hole(at, end);\n",
     "                Ok(None) => {\n",
     "the hole after the last data is not taken (conservative)", [LT], None),
    ("X10", FS, "        let end = match retried(|| seek(&self.file, SeekFrom::Hole(start))) {\n",
     "        let end = match retried(|| seek(&self.file, SeekFrom::Hole(offset))) {\n",
     "SEEK_HOLE asked from the offset, not from the data found", [L], None),
    # ---- the upload of runs of zeros
    ("X11", DRIVER, "        remote.write_zeroes(first * BLOCK_SIZE, count * BLOCK_SIZE)?;\n",
     "        if count > 1 {\n            remote.write_zeroes(first * BLOCK_SIZE, (count - 1) * BLOCK_SIZE)?;\n        }\n",
     "the last block of each run of zeros is not written (object storage keeps its old bytes there)",
     [P], "upload"),
    ("X12", DRIVER, "    for &(first, count) in zeroes {\n",
     "    for &(first, count) in zeroes.iter().take(1) {\n",
     "only a slice's first run of zeros is written", [P], "upload"),
    ("X13", DRIVER, "    for &(first, count) in zeroes {\n",
     "    for &(first, count) in zeroes.iter().filter(|_| !blocks.is_empty()) {\n",
     "a slice with no block to read writes none of its runs of zeros", [P], "upload"),
]


def status(tree):
    return subprocess.run(["git", "-C", tree, "status", "--short"],
                          capture_output=True, text=True).stdout.strip()


def run(cmd, tree, target, limit=LIMIT):
    """(outcome, output): 'ok', 'fail', 'limit' or 'build'."""
    env = dict(os.environ, CARGO_TARGET_DIR=target, CARGO_PROFILE_DEV_DEBUG="0",
               CARGO_PROFILE_TEST_DEBUG="0", CARGO_INCREMENTAL="0")
    proc = subprocess.Popen(cmd, cwd=tree, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            text=True, start_new_session=True)
    try:
        out, _ = proc.communicate(timeout=limit)
    except subprocess.TimeoutExpired:
        os.killpg(proc.pid, signal.SIGKILL)
        out, _ = proc.communicate()
        return "limit", out
    if proc.returncode == 0:
        return "ok", out
    if "could not compile" in out or re.search(r"^error(\[E\d+\])?:", out, re.M) and "test result" not in out:
        return "build", out
    return "fail", out


def failed_tests(out):
    """The tests that failed: by their result line, and by the thread that
    panicked (with --nocapture a result line can be cut by other output)."""
    names = set(re.findall(r"^test (\S+) \.\.\. FAILED", out, re.M))
    names |= set(re.findall(r"^thread '([^']+)' (?:\(\d+\) )?panicked at", out, re.M))
    return sorted(names)


def running_tests(out):
    return sorted(set(re.findall(r"^test (\S+) has been running for", out, re.M)))


class Edited:
    """A file with other contents for a while; restored on any exit."""

    def __init__(self, tree, path, change):
        self.path = os.path.join(tree, path)
        self.change = change
        self.before = None

    def __enter__(self):
        with open(self.path) as f:
            before = f.read()
        after = self.change(before)
        if after is None:
            return False
        self.before = before
        with open(self.path, "w") as f:
            f.write(after)
        return True

    def __exit__(self, *exc):
        if self.before is not None:
            with open(self.path, "w") as f:
                f.write(self.before)


def once(old, new):
    def change(text):
        if text.count(old) != 1:
            return None
        return text.replace(old, new)
    return change


def appended(name):
    with open(os.path.join(HERE, name)) as f:
        more = f.read()
    return lambda text: text + more


def scenarios(tree, target, kind="state"):
    path, name, cmd = SCENARIOS[kind]
    with Edited(tree, path, appended(name)):
        outcome, out = run(cmd, tree, target)
    if outcome == "build":
        return None, out
    failed = set(n.rsplit("::", 1)[-1] for n in failed_tests(out))
    if outcome == "limit":
        failed |= {"(cut at the limit)"}
    return failed, out


def short(names):
    return ", ".join(sorted(re.sub(r"^review_m24e_(r\d+)_.*", r"\1", n) for n in names)) or "none"


def plants(tree, target, wanted):
    out_path = os.path.join(HERE, "plants.out")
    done = set()
    if os.path.exists(out_path):
        with open(out_path) as f:
            done = set(line.split()[0] for line in f if line.strip())
    for pid, path, old, new, what, tests, scen in PLANTS:
        if wanted and pid not in wanted:
            continue
        if pid in done and not wanted:
            continue
        result, detail = "SURVIVED", ""
        with Edited(tree, path, once(old, new)) as applied:
            if not applied:
                result, detail = "NOT APPLIED", "its text is not there exactly once"
            else:
                for cmd in tests:
                    outcome, out = run(cmd, tree, target)
                    if outcome == "build":
                        result = "NOT APPLIED"
                        detail = "does not build: " + " | ".join(
                            l for l in out.splitlines() if l.startswith("error"))[:300]
                        break
                    if outcome == "limit":
                        result = "KILLED"
                        hung = running_tests(out)
                        detail = "by the limit of %d s (%d failed before it; running: %s)" % (
                            LIMIT, len(failed_tests(out)), ", ".join(n.rsplit("::", 1)[-1] for n in hung) or "?")
                        break
                    if outcome == "fail":
                        names = failed_tests(out)
                        result = "KILLED"
                        detail = "%d failed: %s" % (len(names), ", ".join(n.rsplit("::", 1)[-1] for n in names[:6]))
                        break
                if result == "SURVIVED" and scen:
                    failing, out = scenarios(tree, target, scen)
                    if failing is None:
                        detail = "scenarios: do not build with it"
                    elif not failing:
                        detail = "scenarios: all pass with it"
                    else:
                        detail = "scenarios: KILLED; failing with it: " + short(failing)
        line = "%s %s: %s%s" % (pid, result, what, (" [" + detail + "]") if detail else "")
        print(line, flush=True)
        with open(out_path, "a") as f:
            f.write(line + "\n")


def head_scenarios(tree, target, label="head", out_name="scenarios.out", kind="state"):
    failing, out = scenarios(tree, target, kind)
    lines = ["# %s: %s" % (label, "build failed" if failing is None else "failing: " + short(failing))]
    lines += [l for l in out.splitlines() if re.match(r"^test .*review_m24e|^test result|^R7:", l)]
    lines += [l for l in out.splitlines() if "panicked at" in l or l.startswith("  left") or l.startswith(" right")
              or "review_m24e" in l and "assert" in l][:40]
    with open(os.path.join(HERE, out_name), "a") as f:
        f.write("\n".join(lines) + "\n")
    print("\n".join(lines))


def main():
    tree, target, what = sys.argv[1], sys.argv[2], sys.argv[3]
    rest = sys.argv[4:]
    if status(tree):
        sys.exit("the worktree is not clean")
    try:
        if what == "plants":
            plants(tree, target, set(rest))
        elif what == "scenarios":
            head_scenarios(tree, target, "head, state", kind="state")
            head_scenarios(tree, target, "head, upload", kind="upload")
        elif what == "scenarios-with":
            for pid in rest:
                plant = next(p for p in PLANTS if p[0] == pid)
                with Edited(tree, plant[1], once(plant[2], plant[3])) as applied:
                    if not applied:
                        print(pid, "NOT APPLIED")
                        continue
                    head_scenarios(tree, target, "with " + pid, "scenarios_with.out", plant[6] or "state")
        else:
            sys.exit("plants, scenarios or scenarios-with")
    finally:
        left = status(tree)
        if left:
            print("the worktree is not clean at the end:", left, file=sys.stderr)
            sys.exit(3)


if __name__ == "__main__":
    main()
