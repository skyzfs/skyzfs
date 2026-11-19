#!/usr/bin/env python3
"""Plants for the next look at #129 (M2.4f), at ef8b227.

    plants2.py WORKTREE [--with=FILE ...] [ID ...]

As plants.py, with two differences. By default nothing is appended to
the tests: the review's R1 to R4 are now the author's own tests (lifted
under the names in LIFTED). With --with=FILE, FILE (beside this script,
scenarios_next.rs say) is appended to the upload crate's
tests/uploader.rs for that crate's plants. A plant may be several edits, applied in order, each of
whose text must be there exactly once when it is applied. A plant is
KILLED when a test fails with it or the run is cut at its limit, SURVIVED
when every test passes, NOT APPLIED when a text is not there exactly once
or it does not build. Every file is restored on any exit (try/finally),
and the worktree is checked clean at the end. Results are appended to
plants2.out, the lifted tests named by their review name.
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
DAEMON = "bins/skyzfsd/src/upload.rs"
STATUS = "crates/skyzfs-ublk/src/status.rs"
UTESTS = "crates/skyzfs-upload/tests/uploader.rs"

S_UPLOAD = ["cargo", "test", "-p", "skyzfs-upload", "--no-fail-fast"]
S_DAEMON = ["cargo", "test", "-p", "skyzfsd", "--no-fail-fast", "--", "upload"]
S_STATUS = ["cargo", "test", "-p", "skyzfs-ublk", "--lib", "--no-fail-fast", "--", "status"]
LIMIT = 900

LIFTED = {
    "the_leases_row_never_lapses_in_the_whole_waits": "R1",
    "a_publish_is_made_during_a_whole_wait": "R2",
    "a_whole_cut_lost_at_its_sync_waits": "R3",
    "a_whole_cut_gone_at_its_commit_waits": "R4",
    "a_cut_found_committed_ends_the_whole_waits": "T4-found",
    "a_cut_committed_and_gone_ends_the_whole_waits": "T4-gone",
}

LOST_WHOLE_ARM = ("                    wait,\n"
                  "                };\n"
                  "                self.next_wait(now, self.step_at)\n")
LEASE_TOP = ("        if now >= self.lease_at {\n"
             "            self.lease_at = Self::keep_lease(&mut self.lease, &mut self.lease_backoff, now).await;\n")
TURNS = "        let Some(publish) = self.turns.next(driver.publish_due(), now >= self.step_at) else {\n"
SHOWN = ("            out.state = UploadStateShown::WholeLost;\n"
         "            out.why = Text::new(why);\n"
         "            out.wait_ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX);\n")

# The reset of the whole wait, in the arm of the cut's other steps.
RESET = "                    self.whole_lost.succeeded();\n"
RESET_BLOCK = RESET + "                }\n"


def not_by(variant):
    """The whole wait is not ended by `variant` (the refusals' still is)."""
    return (UP, RESET,
            "                    if !matches!(step, Step::%s { .. }) {\n"
            "                        self.whole_lost.succeeded();\n"
            "                    }\n" % variant)


def also_by(pattern):
    """The whole wait is also ended by a step that `pattern` matches."""
    return (UP, RESET_BLOCK,
            RESET_BLOCK + "                if matches!(step, %s) {\n"
            "                    self.whole_lost.succeeded();\n"
            "                }\n" % pattern)


def before(text):
    """The whole wait is ended where `text` is (inserted before it)."""
    indent = text[:len(text) - len(text.lstrip(" "))]
    return (UP, text, indent + "self.whole_lost.succeeded();\n" + text)


GAVE_UP = "                let wait = self.refused.failed();\n"
ERROR = "                let wait = self.errors.failed();\n"
IDLE = "                self.turns.restart();\n"
REREAD = "                self.rereading = true;\n"
PUBLISHED = ("            Ok(_) => {}\n"
             "            Err(RemoteError::Fenced { holder, epoch }) => return self.fenced(holder, epoch),\n")

# (id, [(file, old, new), ...], what, suite)
PLANTS = [
    # The steps that end the whole wait, one at a time (T4).
    ("V1", [not_by("FoundCommitted")],
     "a cut found committed does not end the whole wait (the author's W14)", S_UPLOAD),
    ("V2", [not_by("UploadedNotHeld")],
     "a cut committed and no longer held does not end the whole wait (the author's W15)", S_UPLOAD),
    ("V3", [not_by("Uploaded")],
     "a cut uploaded does not end the whole wait (the two others still do)", S_UPLOAD),
    ("U4", [(UP, RESET, "                    if matches!(step, Step::Uploaded { .. }) {\n"
                        "                        self.whole_lost.succeeded();\n"
                        "                    }\n")],
     "only Uploaded ends the whole wait, not UploadedNotHeld or FoundCommitted", S_UPLOAD),
    # Steps that must not end the whole wait, one at a time.
    ("X1", [also_by("Step::Mismatch { .. }")],
     "a mismatch ends the whole wait", S_UPLOAD),
    ("X2", [before(GAVE_UP)],
     "a commit given up ends the whole wait", S_UPLOAD),
    ("X3", [before(ERROR)],
     "a step that fails with an error ends the whole wait", S_UPLOAD),
    ("X4", [also_by("Step::Lost { rule: Rule::Delta, .. }")],
     "a delta cut lost ends the whole wait", S_UPLOAD),
    ("X5", [before(IDLE)],
     "an idle step ends the whole wait", S_UPLOAD),
    ("X6", [also_by("Step::Began { .. }")],
     "a cut begun ends the whole wait (a part of U1)", S_UPLOAD),
    ("X7", [before(REREAD)],
     "a commit refused, read again at once, ends the whole wait", S_UPLOAD),
    ("X8", [(UP, PUBLISHED, PUBLISHED.replace("Ok(_) => {}", "Ok(_) => self.whole_lost.succeeded(),"))],
     "a publish's step that succeeds ends the whole wait", S_UPLOAD),
    # A step missing and a step between that wrongly stands in for it.
    ("C1", [(UP, RESET_BLOCK,
             "                    if !matches!(step, Step::FoundCommitted { .. }) {\n"
             "                        self.whole_lost.succeeded();\n"
             "                    }\n"
             "                }\n"
             "                if matches!(step, Step::Mismatch { .. }) {\n"
             "                    self.whole_lost.succeeded();\n"
             "                }\n")],
     "V1 with X1: the mismatch after the found cut stands in for it", S_UPLOAD),
    ("C2", [before(GAVE_UP), not_by("FoundCommitted")],
     "V1 with X2: the give-up before the found cut stands in for it", S_UPLOAD),
    ("C3", [before(ERROR), not_by("UploadedNotHeld")],
     "V2 with X3: the failed head swap before the cut gone stands in for it", S_UPLOAD),
    # The review's U1 to U10 at this head (U6's text after N1).
    ("U1", [(UP, "        if step.is_ok() {\n            self.errors.succeeded();\n        }\n",
             "        if step.is_ok() {\n            self.errors.succeeded();\n            self.whole_lost.succeeded();\n        }\n")],
     "any step of the cut that succeeds (a Began, a Read) ends the whole wait", S_UPLOAD),
    ("U2", [(UP, TURNS,
             "        if matches!(self.state, State::WholeLost { .. }) && now < self.step_at {\n"
             "            return self.step_at.min(self.lease_at).saturating_duration_since(now);\n"
             "        }\n" + TURNS)],
     "no publish steps during a whole wait (the lease is still kept)", S_UPLOAD),
    ("U3", [(UP, LEASE_TOP,
             "        if now >= self.lease_at && matches!(self.state, State::WholeLost { .. }) {\n"
             "            self.lease_at = now + Duration::from_millis(self.lease.ttl_ms() / 3);\n"
             "        } else if now >= self.lease_at {\n"
             "            self.lease_at = Self::keep_lease(&mut self.lease, &mut self.lease_backoff, now).await;\n")],
     "during a whole wait the uploader wakes at each renewal's time but renews nothing", S_UPLOAD),
    ("U5", [(DRV, "            Err(e) => (Stage::Check, Ok(self.lost(&upload, e.to_string()))),\n",
             "            Err(e) => (Stage::Check, Ok(self.lost(&Upload { rule: Rule::Delta, ..upload }, e.to_string()))),\n")],
     "a whole cut lost at its sync is taken for a delta (no wait, not counted)", S_UPLOAD),
    ("U6", [(DRV, "Ok(self.lost(&upload, \"the store no longer held it at its commit\".into())),",
             "Ok(self.lost(&Upload { rule: Rule::Delta, ..upload }, \"the store no longer held it at its commit\".into())),")],
     "a whole cut gone at its commit is taken for a delta (no wait, not counted)", S_UPLOAD),
    ("U7", [(DRV, "            Err(e) => return (Stage::Check, Ok(self.lost(&upload, e.to_string()))),\n",
             "            Err(e) => return (Stage::Check, Ok(self.lost(&Upload { rule: Rule::Delta, ..upload }, e.to_string()))),\n")],
     "a whole cut lost at a read is taken for a delta (no wait, not counted)", S_UPLOAD),
    ("U8", [(BACK, "        self.next = Some(wait.saturating_mul(2).min(self.last));\n",
             "        self.next = Some(wait.saturating_mul(2).min(LAST));\n")],
     "the whole wait capped at the error wait's five minutes", S_UPLOAD),
    ("U9", [(STATUS, "lost_whole: (version >= 19).then(|| field(1408)),",
             "lost_whole: (version >= 18).then(|| field(1408)),")],
     "a file of the eighteenth version is read with the count (past its end)", S_STATUS),
    ("U10", [(DAEMON, SHOWN, SHOWN.replace(
        "            out.wait_ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX);\n",
        "            let _ = wait;\n"))],
     "the daemon does not show the whole wait in upload_wait_ms", S_DAEMON),
    # The author's W1 to W8 at this head (a sample made again).
    ("W1", [(UP, RESET, "")], "no commit ends the whole wait", S_UPLOAD),
    ("W4", [(UP, "                self.step_at = ended + wait;\n                self.state = State::WholeLost {",
             "                self.step_at = ended;\n                self.state = State::WholeLost {")],
     "no wait after a whole cut lost, the state kept", S_UPLOAD),
    ("W5", [(UP, LOST_WHOLE_ARM, LOST_WHOLE_ARM.replace("self.next_wait(now, self.step_at)",
                                                        "self.step_at.saturating_duration_since(now)"))],
     "the turn that loses a whole cut sleeps out its wait (no renewal, no publish meanwhile)", S_UPLOAD),
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


def named(n):
    return "%s (%s)" % (n, LIFTED[n]) if n in LIFTED else n


class Edited:
    """Every edit of one plant, file by file; all restored on exit."""

    def __init__(self, tree, edits):
        self.tree = tree
        self.edits = edits
        self.saved = {}

    def __enter__(self):
        texts = {}
        for path, old, new in self.edits:
            full = os.path.join(self.tree, path)
            if full not in texts:
                with open(full) as f:
                    texts[full] = f.read()
                self.saved[full] = texts[full]
            if texts[full].count(old) != 1:
                return False
            texts[full] = texts[full].replace(old, new)
        for full, text in texts.items():
            with open(full, "w") as f:
                f.write(text)
        return True

    def __exit__(self, *exc):
        for full, text in self.saved.items():
            with open(full, "w") as f:
                f.write(text)


def main():
    tree = sys.argv[1]
    wanted = [a for a in sys.argv[2:] if not a.startswith("--with=")]
    # --with=FILE: FILE (beside this script) is appended to the upload
    # crate's tests/uploader.rs for the plants of that crate.
    extra = [a[len("--with="):] for a in sys.argv[2:] if a.startswith("--with=")]
    if status(tree):
        sys.exit("the worktree is not clean")
    out_path = os.path.join(HERE, "plants2.out")
    try:
        for pid, edits, what, suite in PLANTS:
            if wanted and pid not in wanted:
                continue
            result, detail = "SURVIVED", ""
            if extra and suite is S_UPLOAD:
                more = ""
                for name in extra:
                    with open(os.path.join(HERE, name)) as f:
                        more += f.read()
                with open(os.path.join(tree, UTESTS)) as f:
                    tests = f.read()
                edits = [(UTESTS, tests[-200:], tests[-200:] + more)] + edits
                what += " [with %s]" % ", ".join(extra)
            with Edited(tree, edits) as applied:
                if not applied:
                    result, detail = "NOT APPLIED", "a text is not there exactly once"
                else:
                    outcome, out = run(suite, tree)
                    failed = failed_tests(out)
                    if outcome == "build":
                        result = "NOT APPLIED"
                        detail = "does not build: " + " | ".join(
                            l for l in out.splitlines() if l.startswith("error"))[:300]
                    elif outcome == "limit":
                        result = "KILLED"
                        detail = "by the limit of %d s (failed before it: %s; running: %s)" % (
                            LIMIT, ", ".join(failed) or "none", ", ".join(running_tests(out)) or "?")
                    elif outcome == "fail":
                        result = "KILLED"
                        detail = ", ".join(named(n) for n in failed)
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
