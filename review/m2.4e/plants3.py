#!/usr/bin/env python3
"""Plants for the third next look at #125 (M2.4e), at d71869a: F4's fix.

    plants3.py WORKTREE TARGET_DIR [ID ...]

One change at a time is written into a worktree of the head, the tests
the author ran (`cargo test -p skyzfs-local -p skyzfs-upload --lib
--tests`) are run with a limit, and the file is restored on any exit
(try/finally). A plant is KILLED when a test fails with it or the run
is cut at its limit, SURVIVED when every test passes, NOT APPLIED when
its text is not there exactly once or it does not build. For a plant
that survives, the review's scenarios R10 to R13 (scenarios_f4.rs and
scenarios_f4_cost.rs, R13 with its bound on) are run with it, and the
result names those that fail. Results are appended to plants3.out. The
worktree must be clean at the start and is checked clean at the end.
"""
import os
import re
import signal
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
STATE = "crates/skyzfs-local/src/inplace/state.rs"
MAPS = "crates/skyzfs-local/src/inplace/state/maps.rs"
TESTS = "crates/skyzfs-local/src/inplace/state/tests.rs"
SUITE = ["cargo", "test", "-p", "skyzfs-local", "-p", "skyzfs-upload", "--lib", "--tests", "--no-fail-fast"]
SCEN = ["cargo", "test", "-p", "skyzfs-local", "--lib", "--no-fail-fast", "--", "review_m24e_r1", "--nocapture"]
LIMIT = 600

LOOK_MAP = "                .first_in(&self.region, maps, from, stretch)\n"
UNWRAP = "                .unwrap_or(stretch);\n"
HOLE_LOOK = "            let end = self.hole_run_end(from, stop);\n"
RETURN = "            if end < stretch || stretch >= self.blocks {\n"
SUMMARY_LOOP = "        for i in first_word / 64..=last_word / 64 {\n"
FIRST_MASK = ("                if word == first_word {\n"
              "                    bits &= u64::MAX << (from % 64);\n"
              "                }\n"
              "                if bits != 0 {\n")
LAST_MASK = "                summary &= u64::MAX >> (63 - last_word % 64);\n"
SUMMARY_FIRST_MASK = ("            if i == first_word / 64 {\n"
                      "                summary &= u64::MAX << (first_word % 64);\n"
                      "            }\n"
                      "            if i == last_word / 64 {\n")
UNTIL = "                    return (block < until).then_some(block);\n"

# (id, file, old, new, what)
PLANTS = [
    # A sample of the author's F4a to F4g, made again from their text.
    ("F4b", STATE, LOOK_MAP, "                .first_in(&self.region, maps, from, self.blocks)\n",
     "the look in the map is not bounded by the stretch (to the volume's end)"),
    ("F4c", STATE, UNWRAP, "                .unwrap_or(self.blocks);\n",
     "the look in the hole set is not bounded by the stretch where the map has no block in it"),
    ("F4d", MAPS, FIRST_MASK, "                if bits != 0 {\n",
     "the look in the map takes the blocks of from's word before from"),
    ("F4e", STATE, RETURN, "            if true {\n",
     "a run is cut where a stretch ends"),
    ("F4g", MAPS, SUMMARY_LOOP, "        for i in first_word / 64..=(first_word / 64).min(last_word / 64) {\n",
     "the look in the map stops at the first summary word"),
    # The review's own.
    ("Z01", STATE, "const RUN_LOOK: u64 = 64 * 64;\n", "const RUN_LOOK: u64 = 64 * 64 * 64;\n",
     "a stretch of 64 summary words, not one (a constant factor on each run's look)"),
    ("Z02", MAPS, SUMMARY_LOOP,
     "        for i in 0..=last_word / 64 {\n"
     "            if i < first_word / 64 {\n"
     "                let _ = maps.iter().fold(0, |acc, &map| acc | region.load(self.summary_base(map) + i));\n"
     "                continue;\n"
     "            }\n",
     "the look in the map reads the summary from its first word (the result as before; the cost of the volume before the run)"),
    ("Z03", MAPS, LAST_MASK, "                summary &= (u64::MAX >> (63 - last_word % 64)) >> 1;\n",
     "the summary's last word in the range dropped (a block in the last 64 of a range is missed)"),
    ("Z04", MAPS, UNTIL, "                    return Some(block);\n",
     "first_in gives a block past `until` (a block in the range's last word, after it)"),
    ("Z05", STATE, RETURN, "            if end < stop || stretch >= self.blocks {\n",
     "a run goes on past the stretch where it ends at the block of the map that bounds it"),
    ("Z06", STATE, HOLE_LOOK, "            let end = self.hole_run_end(block, stop);\n",
     "each stretch walks the hole set from the run's first block, not from the stretch (same ends; the cost of a run grows as its stretches squared)"),
    ("Z07", STATE, LOOK_MAP, "                .first_in(&self.region, maps, block, stretch)\n",
     "each stretch reads the map's summary from the run's first block, not from the stretch (same ends; the cost of a run grows as its stretches squared)"),
    ("Z08", STATE, RETURN, "            if end < stretch || stretch > self.blocks {\n",
     "the look does not stop at the volume's last stretch (a run to the volume's end never ends)"),
    ("Z09", MAPS, FIRST_MASK,
     "                if i == first_word / 64 {\n"
     "                    bits &= u64::MAX << (from % 64);\n"
     "                }\n"
     "                if bits != 0 {\n",
     "the mask of from's bit applied to every word of from's summary word (a block at a low offset of a later word is missed)"),
    ("Z10", MAPS, SUMMARY_FIRST_MASK, "            if i == last_word / 64 {\n",
     "the summary's first word not masked below from's word (a block before from is found)"),
]


def status(tree):
    return subprocess.run(["git", "-C", tree, "status", "--short"], capture_output=True, text=True).stdout.strip()


def run(cmd, tree, target, env_more=None, limit=LIMIT):
    env = dict(os.environ, CARGO_TARGET_DIR=target, CARGO_PROFILE_DEV_DEBUG="0",
               CARGO_PROFILE_TEST_DEBUG="0", CARGO_INCREMENTAL="0")
    env.update(env_more or {})
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
    if "could not compile" in out or (re.search(r"^error(\[E\d+\])?:", out, re.M) and "test result" not in out):
        return "build", out
    return "fail", out


def failed_tests(out):
    names = set(re.findall(r"^test (\S+) \.\.\. FAILED", out, re.M))
    names |= set(re.findall(r"^thread '([^']+)' (?:\(\d+\) )?panicked at", out, re.M))
    return sorted(n.rsplit("::", 1)[-1] for n in names)


def running_tests(out):
    return sorted(set(n.rsplit("::", 1)[-1] for n in re.findall(r"^test (\S+) has been running for", out, re.M)))


class Edited:
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


def appended(names):
    more = ""
    for name in names:
        with open(os.path.join(HERE, name)) as f:
            more += f.read()
    return lambda text: text + more


def scenarios(tree, target):
    with Edited(tree, TESTS, appended(["scenarios_f4_cost.rs", "scenarios_f4.rs"])):
        outcome, out = run(SCEN, tree, target, {"REVIEW_BOUND": "1"})
    if outcome == "build":
        return None
    failed = set(failed_tests(out))
    if outcome == "limit":
        failed |= {"(cut at the limit; running: %s)" % ", ".join(running_tests(out))}
    return sorted(re.sub(r"^review_m24e_(r\d+)_.*", r"\1", n) for n in failed)


def main():
    tree, target = sys.argv[1], sys.argv[2]
    wanted = set(sys.argv[3:])
    if status(tree):
        sys.exit("the worktree is not clean")
    out_path = os.path.join(HERE, "plants3.out")
    try:
        for pid, path, old, new, what in PLANTS:
            if wanted and pid not in wanted:
                continue
            result, detail = "SURVIVED", ""
            with Edited(tree, path, once(old, new)) as applied:
                if not applied:
                    result, detail = "NOT APPLIED", "its text is not there exactly once"
                else:
                    outcome, out = run(SUITE, tree, target)
                    if outcome == "build":
                        result = "NOT APPLIED"
                        detail = "does not build: " + " | ".join(l for l in out.splitlines() if l.startswith("error"))[:300]
                    elif outcome == "limit":
                        result = "KILLED"
                        detail = "by the limit of %d s (%d failed before it; running: %s)" % (
                            LIMIT, len(failed_tests(out)), ", ".join(running_tests(out)) or "?")
                    elif outcome == "fail":
                        names = failed_tests(out)
                        result = "KILLED"
                        detail = "%d failed: %s" % (len(names), ", ".join(names[:6]))
                    if result == "SURVIVED":
                        failing = scenarios(tree, target)
                        if failing is None:
                            detail = "scenarios: do not build with it"
                        elif not failing:
                            detail = "scenarios R10 to R13: all pass with it"
                        else:
                            detail = "scenarios: fail with it: " + ", ".join(failing)
            line = "%s %s: %s [%s]" % (pid, result, what, detail)
            print(line, flush=True)
            with open(out_path, "a") as f:
                f.write(line + "\n")
    finally:
        left = status(tree)
        if left:
            print("the worktree is not clean at the end:", left, file=sys.stderr)
            sys.exit(3)


if __name__ == "__main__":
    main()
