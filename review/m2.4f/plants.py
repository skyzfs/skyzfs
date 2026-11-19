#!/usr/bin/env python3
"""Plants for the review of #129 (M2.4f), at b75a128.

    plants.py WORKTREE [ID ...]

One change at a time is written into a worktree of the head, the tests
of its crate are run with a limit (the upload crate's with the review's
scenarios R1 to R4 appended to tests/uploader.rs), and every file is
restored on any exit (try/finally). A plant is KILLED when a test fails
with it or the run is cut at its limit, SURVIVED when every test passes,
NOT APPLIED when its text is not there exactly once or it does not
build. For each result the failing tests are named, the author's apart
from the review's (review_m24f_*). Results are appended to plants.out.
The worktree must be clean at the start and is checked clean at the end.
"""
import os
import re
import signal
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
UP = "crates/skyzfs-upload/src/uploader.rs"
DRV = "crates/skyzfs-upload/src/driver.rs"
BACK = "crates/skyzfs-upload/src/backoff.rs"
UTESTS = "crates/skyzfs-upload/tests/uploader.rs"
DAEMON = "bins/skyzfsd/src/upload.rs"
STATUS = "crates/skyzfs-ublk/src/status.rs"
STATE = "crates/skyzfs-local/src/inplace/state.rs"

S_UPLOAD = ["cargo", "test", "-p", "skyzfs-upload", "--no-fail-fast"]
S_DAEMON = ["cargo", "test", "-p", "skyzfsd", "--no-fail-fast", "--", "upload"]
S_STATUS = ["cargo", "test", "-p", "skyzfs-ublk", "--lib", "--no-fail-fast", "--", "status"]
S_STATE = ["cargo", "test", "-p", "skyzfs-local", "--lib", "--no-fail-fast"]
LIMIT = 900

LOST_WHOLE_ARM = ("                    wait,\n"
                  "                };\n"
                  "                self.next_wait(now, self.step_at)\n")
LEASE_TOP = ("        if now >= self.lease_at {\n"
             "            self.lease_at = Self::keep_lease(&mut self.lease, &mut self.lease_backoff, now).await;\n")
TURNS = "        let Some(publish) = self.turns.next(driver.publish_due(), now >= self.step_at) else {\n"
SHOWN = ("            out.state = UploadStateShown::WholeLost;\n"
         "            out.why = Text::new(why);\n"
         "            out.wait_ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX);\n")

# (id, file, old, new, what, suite)
PLANTS = [
    # A sample of the author's W1 to W8, made again from their text.
    ("W1", UP, "                    self.whole_lost.succeeded();\n", "",
     "a landed commit does not end the wait", S_UPLOAD),
    ("W2", UP, "                rule: Rule::Whole,\n", "                rule: _,\n",
     "every cut lost waits, a delta cut too", S_UPLOAD),
    ("W3", UP, "let wait = self.whole_lost.failed();", "let wait = self.errors.failed();",
     "a whole cut lost waits as an error does (1 s doubling)", S_UPLOAD),
    ("W4", UP, "                self.step_at = ended + wait;\n                self.state = State::WholeLost {",
     "                self.step_at = ended;\n                self.state = State::WholeLost {",
     "no wait after a whole cut lost, the state kept", S_UPLOAD),
    ("W5", UP, LOST_WHOLE_ARM, LOST_WHOLE_ARM.replace("self.next_wait(now, self.step_at)",
                                                      "self.step_at.saturating_duration_since(now)"),
     "the turn that loses a whole cut sleeps out its wait (no renewal, no publish meanwhile)", S_UPLOAD),
    ("W6", BACK, "Duration::from_secs(15 * 60);", "Duration::from_secs(60 * 60);",
     "the longest whole wait is an hour", S_UPLOAD),
    ("W7", BACK, "pub const WHOLE_FIRST: Duration = Duration::from_secs(60);",
     "pub const WHOLE_FIRST: Duration = Duration::from_secs(1);",
     "the first whole wait is a second", S_UPLOAD),
    ("W8", DRV, "            self.figures.lost_whole += 1;\n", "",
     "a whole cut lost is not counted", S_UPLOAD),
    # The review's own.
    ("U1", UP, "        if step.is_ok() {\n            self.errors.succeeded();\n        }\n",
     "        if step.is_ok() {\n            self.errors.succeeded();\n            self.whole_lost.succeeded();\n        }\n",
     "any step of the cut that succeeds (a Began, a Read) ends the whole wait", S_UPLOAD),
    ("U2", UP, TURNS,
     "        if matches!(self.state, State::WholeLost { .. }) && now < self.step_at {\n"
     "            return self.step_at.min(self.lease_at).saturating_duration_since(now);\n"
     "        }\n" + TURNS,
     "no publish steps during a whole wait (the lease is still kept)", S_UPLOAD),
    ("U3", UP, LEASE_TOP,
     "        if now >= self.lease_at && matches!(self.state, State::WholeLost { .. }) {\n"
     "            self.lease_at = now + Duration::from_millis(self.lease.ttl_ms() / 3);\n"
     "        } else if now >= self.lease_at {\n"
     "            self.lease_at = Self::keep_lease(&mut self.lease, &mut self.lease_backoff, now).await;\n",
     "during a whole wait the uploader wakes at each renewal's time but renews nothing", S_UPLOAD),
    ("U4", UP, "                    self.whole_lost.succeeded();\n",
     "                    if matches!(step, Step::Uploaded { .. }) {\n"
     "                        self.whole_lost.succeeded();\n"
     "                    }\n",
     "only Uploaded ends the whole wait, not UploadedNotHeld or FoundCommitted", S_UPLOAD),
    ("U5", DRV, "            Err(e) => (Stage::Check, Ok(self.lost(&upload, e.to_string()))),\n",
     "            Err(e) => (Stage::Check, Ok(self.lost(&Upload { rule: Rule::Delta, ..upload }, e.to_string()))),\n",
     "a whole cut lost at its sync is taken for a delta (no wait, not counted)", S_UPLOAD),
    ("U6", DRV, "Ok(self.lost(&upload, \"it went before its commit\".into())),",
     "Ok(self.lost(&Upload { rule: Rule::Delta, ..upload }, \"it went before its commit\".into())),",
     "a whole cut gone at its commit is taken for a delta (no wait, not counted)", S_UPLOAD),
    ("U7", DRV, "            Err(e) => return (Stage::Check, Ok(self.lost(&upload, e.to_string()))),\n",
     "            Err(e) => return (Stage::Check, Ok(self.lost(&Upload { rule: Rule::Delta, ..upload }, e.to_string()))),\n",
     "a whole cut lost at a read is taken for a delta (no wait, not counted)", S_UPLOAD),
    ("U8", BACK, "        self.next = Some(wait.saturating_mul(2).min(self.last));\n",
     "        self.next = Some(wait.saturating_mul(2).min(LAST));\n",
     "the whole wait capped at the error wait's five minutes", S_UPLOAD),
    ("U9", STATUS, "lost_whole: (version >= 19).then(|| field(1408)),",
     "lost_whole: (version >= 18).then(|| field(1408)),",
     "a file of the eighteenth version is read with the count (past its end)", S_STATUS),
    ("U10", DAEMON, SHOWN, SHOWN.replace(
        "            out.wait_ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX);\n",
        "            let _ = wait;\n"),
     "the daemon does not show the whole wait in upload_wait_ms", S_DAEMON),
    # #125's G3 and G4 (b75a128).
    ("Z06", STATE, "            let end = self.hole_run_end(from, stop);\n",
     "            let end = self.hole_run_end(block, stop);\n",
     "each stretch walks the hole set from the run's first block (cost only)", S_STATE),
    ("Z07", STATE, "                .first_in(&self.region, maps, from, stretch)\n",
     "                .first_in(&self.region, maps, block, stretch)\n",
     "each stretch reads the map's summary from the run's first block (cost only)", S_STATE),
    ("G12", STATE, "const RUN_LOOK: u64 = 64 * 64;\n", "const RUN_LOOK: u64 = 64;\n",
     "a stretch of one word of the map, not one summary word (cost only: 64 times the looks)", S_STATE),
    ("G5", STATE, "        (chunk.min(chunks) * HOLE_BLOCKS).min(stop)\n",
     "        chunk.min(chunks) * HOLE_BLOCKS\n",
     "a hole run is not cut at the stop (a written block inside a hole chunk read as zeros)", S_STATE),
    ("G8", STATE, "        while chunk < chunks && (chunk * HOLE_BLOCKS + HOLE_BLOCKS).min(blocks) <= end {\n",
     "        while chunk < chunks && chunk * HOLE_BLOCKS + HOLE_BLOCKS <= end {\n",
     "a partial last chunk is never a hole", S_STATE),
    ("G11", STATE, "            let end = self.zero_run_end(&maps, block);\n",
     "            let end = self.zero_run_end(&maps, block).min(block + RUN_LOOK);\n",
     "a run of zeros is cut a stretch's length from its start", S_STATE),
    ("G13", STATE, "            if ones < left {\n", "            if ones <= left {\n",
     "a run of hole chunks ends at the end of a word of the set (the author's E3, made again)", S_STATE),
]


def status(tree):
    return subprocess.run(["git", "-C", tree, "status", "--short"], capture_output=True, text=True).stdout.strip()


def run(cmd, tree, limit=LIMIT):
    proc = subprocess.Popen(cmd, cwd=tree, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
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


def appended(name):
    with open(os.path.join(HERE, name)) as f:
        more = f.read()
    return lambda text: text + more


def main():
    tree = sys.argv[1]
    wanted = set(sys.argv[2:])
    if status(tree):
        sys.exit("the worktree is not clean")
    out_path = os.path.join(HERE, "plants.out")
    try:
        for pid, path, old, new, what, suite in PLANTS:
            if wanted and pid not in wanted:
                continue
            result, detail = "SURVIVED", ""
            with Edited(tree, UTESTS, appended("scenarios_upload.rs") if suite is S_UPLOAD else (lambda t: None)):
                with Edited(tree, path, once(old, new)) as applied:
                    if not applied:
                        result, detail = "NOT APPLIED", "its text is not there exactly once"
                    else:
                        outcome, out = run(suite, tree)
                        failed = failed_tests(out)
                        theirs = [n for n in failed if not n.startswith("review_m24f")]
                        ours = [re.sub(r"^review_m24f_(r\d+)_.*", r"\1", n) for n in failed
                                if n.startswith("review_m24f")]
                        if outcome == "build":
                            result = "NOT APPLIED"
                            detail = "does not build: " + " | ".join(
                                l for l in out.splitlines() if l.startswith("error"))[:300]
                        elif outcome == "limit":
                            result = "KILLED"
                            detail = "by the limit of %d s (failed before it: %s; running: %s)" % (
                                LIMIT, ", ".join(failed) or "none", ", ".join(running_tests(out)) or "?")
                        elif outcome == "fail":
                            if theirs:
                                result = "KILLED"
                                detail = "author's: %s" % ", ".join(theirs)
                                if ours:
                                    detail += "; review's: %s" % ", ".join(ours)
                            else:
                                result = "SURVIVED the author's tests"
                                detail = "caught by the review's %s" % ", ".join(ours)
                        elif outcome == "ok":
                            result = "SURVIVED"
            line = "%s\t%s\t%s\t%s\n" % (pid, result, what, detail)
            print(line, end="", flush=True)
            with open(out_path, "a") as f:
                f.write(line)
    finally:
        left = status(tree)
        if left:
            print("THE WORKTREE IS NOT CLEAN:\n" + left, flush=True)
        else:
            print("worktree clean", flush=True)


if __name__ == "__main__":
    main()
