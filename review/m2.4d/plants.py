#!/usr/bin/env python3
"""Plants and scenarios for the review of #120 at 0b0789a (M2.4d).

    plants.py WORKTREE TARGET_DIR plants [ID ...]
    plants.py WORKTREE TARGET_DIR scenarios
    plants.py WORKTREE TARGET_DIR variant
    plants.py WORKTREE TARGET_DIR scenarios-with ID ...

`plants`: one change at a time is written into a scratch worktree of the
head, the tests of the crate it is in are run, and the file is restored.
A plant is

- KILLED       when a test of the pull request's tree fails with it (or
               hangs: the run is cut at its limit, and the result says
               "by the limit"),
- SURVIVED     when every test passes with it,
- NOT APPLIED  when its text is not found exactly once, or it does not
               build.

For a plant that survives and is marked `scen`, the review's scenarios
(scenarios.rs, appended to the store's test module for the run) are run
as well, and the result names the scenarios that fail with it; at the
head only `changed_r7` fails.

`scenarios`: the scenarios at the head, in the store's test module and,
with the test hooks, on real files (scenarios_real.rs).

`scenarios-with`: the scenarios with one plant in place, whatever the
head's tests say of it (scenarios_with.out).

`variant`: every store of the cut tests is started with the header's
sync on (the tests' `start_as` passes true), and the tests that fail are
listed: the store-level power-cut tests must not be among them.

Each result is appended to plants.out (scenarios.out, variant.out) as it
comes; a plant already in plants.out is skipped on a rerun. The worktree
must be clean at the start and is checked clean at the end.
"""
import os
import re
import signal
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))

INPLACE = "crates/skyzfs-local/src/inplace.rs"
STATE = "crates/skyzfs-local/src/inplace/state.rs"
CUTS = "crates/skyzfs-local/src/inplace/tests/cuts.rs"
REAL = "crates/skyzfs-local/tests/inplace_real.rs"
STATUS = "crates/skyzfs-ublk/src/status.rs"
SERVE = "bins/skyzfsd/src/serve.rs"
CLI = "bins/skyzfsd/src/cli.rs"

# L: the store's tests (the whole library: the cut tests and the rest).
# U: the transport's. D: the daemon's, in the build with the test hooks.
L = ["cargo", "test", "-p", "skyzfs-local", "--lib", "--no-fail-fast"]
U = ["cargo", "test", "-p", "skyzfs-ublk", "--lib", "--no-fail-fast"]
D = ["cargo", "test", "-p", "skyzfsd", "--features", "test-hooks", "--no-fail-fast"]
SCEN = ["cargo", "test", "-p", "skyzfs-local", "--lib", "--no-fail-fast", "--", "review_m24d"]
SCEN_REAL = ["cargo", "test", "-p", "skyzfs-local", "--features", "test-hooks", "--test", "inplace_real",
             "--no-fail-fast", "--", "review_m24d", "--nocapture"]
LIMIT = 300
BASELINE = {"review_m24d_changed_r7_the_drop_of_a_failed_store_waits_for_the_maps_sync_with_the_option_off"}

DROP = ("                !intent.waiting.is_empty()\n"
        "                    || intent.syncing.is_some()\n"
        "                    || (intent.header_due && !failed)\n")
COUNTED = ("            if std::mem::take(&mut intent.syncing_header) {\n"
           "                intent.header_syncs.add(began.elapsed());\n"
           "            } else {\n")

# (id, file, old, new, what the plant is, tests, scen)
PLANTS = [
    # ---- inplace.rs: where the sync begins
    ("A01", INPLACE,
     "        let failed = self.failed;\n        self.begin_intent_sync();\n        failed || !self.failed\n",
     "        true\n",
     "sync_header begins no sync (the option does nothing)", [L], False),
    ("A02", INPLACE,
     "        if intent.syncing.is_some() {\n            intent.header_due = true;\n            return true;\n        }\n",
     "        if intent.syncing.is_some() {\n            return true;\n        }\n",
     "a header written while a sync is under way is not marked due", [L], False),
    ("A03", INPLACE,
     "        if intent.syncing.is_some() {\n            intent.header_due = true;\n            return true;\n        }\n",
     "",
     "a header's sync is begun beside the one under way", [L], False),
    ("A04", INPLACE,
     "        if !intent.header_sync {\n            return true;\n        }\n", "",
     "the header is synced whatever the option says", [L], False),
    ("A05", INPLACE,
     "        self.sync_header()\n    }\n\n    /// Cut `number` of group `group` is committed in object storage:",
     "        true\n    }\n\n    /// Cut `number` of group `group` is committed in object storage:",
     "name_committed_cut begins no sync", [L], False),
    ("A06", INPLACE,
     "        self.sync_header()\n    }\n\n    /// The header was just written",
     "        true\n    }\n\n    /// The header was just written",
     "a release begins no sync (a naming does)", [L], False),
    ("A07", INPLACE, "        failed || !self.failed\n", "        let _ = failed;\n        true\n",
     "a header's sync that fails at once is not reported by sync_header", [L], True),
    # ---- inplace.rs: the bookkeeping of the sync under way
    ("B01", INPLACE,
     "        intent.header_due = false;\n        intent.syncing_header = intent.waiting.is_empty();\n",
     "        intent.syncing_header = intent.waiting.is_empty();\n",
     "a sync that begins does not clear the header due (a sync after every sync)", [L], True),
    ("B02", INPLACE, "        intent.syncing_header = intent.waiting.is_empty();\n",
     "        intent.syncing_header = false;\n",
     "a header's sync is counted with the write path's", [L], False),
    ("B03", INPLACE, "        intent.syncing_header = intent.waiting.is_empty();\n",
     "        intent.syncing_header = true;\n",
     "every sync of the map is counted as a header's", [L], True),
    ("B04", INPLACE, "        intent.syncing_header = intent.waiting.is_empty();\n",
     "        intent.syncing_header = intent.header_sync;\n",
     "with the option on, every sync of the map is counted as a header's", [L], True),
    ("B05", INPLACE, COUNTED,
     "            if std::mem::take(&mut intent.syncing_header) {\n"
     "                intent.header_syncs.add(began.elapsed());\n"
     "                if result.is_err() {\n                    return self.run_deferred();\n                }\n"
     "            } else {\n",
     "a header's sync that fails does not fail the volume", [L], False),
    ("B06", INPLACE, "            if std::mem::take(&mut intent.syncing_header) {\n",
     "            if intent.syncing_header {\n",
     "the mark of a header's sync is not taken at its end (set again at the next begin: no difference expected)",
     [L], True),
    # ---- inplace.rs: the next sync
    ("C01", INPLACE, "        if (waits || header) && !self.failed {\n",
     "        let _ = header;\n        if waits && !self.failed {\n",
     "a sync that ends begins none for a header due alone (the drop still would)", [L], True),
    ("C02", INPLACE, "                idle && intent.header_due,\n", "                intent.header_due,\n",
     "the header due is looked at whether or not a sync is under way (no difference expected: none is)",
     [L], True),
    # ---- inplace.rs: the drop
    ("E01", INPLACE, "                    || intent.syncing.is_some()\n", "",
     "the drop does not wait for a sync of the map under way with no write waiting", [L], True),
    ("E02", INPLACE, "                    || (intent.header_due && !failed)\n", "",
     "the drop does not look at a header due", [L], True),
    ("E03", INPLACE, DROP, "                !intent.waiting.is_empty()\n",
     "the drop's loop as before the pull request", [L], True),
    ("E04", INPLACE, "                    || (intent.header_due && !failed)\n",
     "                    || intent.header_due\n",
     "the drop goes on for a header due on a failed volume", [L], True),
    # ---- the option's way to the store, and the figures
    ("F01", STATE, "            intent_header_sync: hold.intent_header_sync,\n",
     "            intent_header_sync: false,\n",
     "the option does not reach the store", [L], False),
    ("F02", INPLACE, "                header_sync: intent.header_sync,\n", "                header_sync: false,\n",
     "the figures say the option is off", [L], False),
    ("F03", INPLACE, "                header_syncs: intent.header_syncs,\n",
     "                header_syncs: intent.syncs,\n",
     "the figures give the write path's syncs as the header's", [L], False),
    # ---- status.rs
    ("S01", STATUS, "            header: (version >= 16).then(|| HeaderSyncShown {",
     "            header: (version >= 15).then(|| HeaderSyncShown {",
     "the header's sync is read from a file of version 15", [U], False),
    ("S02", STATUS, "        header.syncs.count,\n        header.syncs.sum_ns,\n",
     "        header.syncs.sum_ns,\n        header.syncs.count,\n",
     "the count and the summed time are written in each other's place", [U], False),
    ("S03", STATUS, "    } else if is(FIFTEENTH_LEN, FIFTEENTH_MAGIC) {\n",
     "    } else if is(FIFTEENTH_LEN, FIFTEENTH_MAGIC) && bytes.is_empty() {\n",
     "a file of version 15 is not read", [U], False),
    ("S04", STATUS, "                on: field(976) == 1,\n", "                on: field(976) != 0,\n",
     "any value but 0 reads as on", [U], False),
    ("S05", STATUS, "                let on = if header.on { \"on\" } else { \"off\" };\n",
     "                let on = if header.on { \"on\" } else { \"on\" };\n",
     "status always prints intent_header_sync on", [U], False),
    ("S06", STATUS, "            if let Some(header) = intent.header {\n",
     "            if let Some(header) = intent.header.or(Some(HeaderSyncShown::default())) {\n",
     "status prints the header's keys for a file of any version", [U], False),
    ("S07", STATUS, "        u64::from(header.on),\n", "        u64::from(!header.on),\n",
     "the file says on where the option is off", [U], False),
    ("S08", STATUS, "const FIFTEENTH_LEN: usize = 976;\n", "const FIFTEENTH_LEN: usize = 984;\n",
     "a file of version 15 is taken to be 984 bytes", [U], False),
    # ---- the daemon
    ("D01", SERVE, "        intent_header_sync: serve.intent_header_sync,\n", "        intent_header_sync: false,\n",
     "serve does not hand the option to the store", [D], False),
    ("D02", SERVE, "            on: figures.header_sync,\n", "            on: false,\n",
     "the daemon's status says off whatever the store says", [D], False),
    ("D03", SERVE, "            syncs: figures.header_syncs,\n", "            syncs: figures.syncs,\n",
     "the daemon's status gives the write path's syncs as the header's", [D], False),
    ("D04", CLI, "            if intent_header_sync && !hold_cuts {\n", "            if false {\n",
     "--intent-header-sync is taken without --hold-cuts", [D], False),
    ("D05", CLI, "            let intent_header_sync = given.flag(\"--intent-header-sync\");\n",
     "            let intent_header_sync = given.flag(\"--hold-cuts\");\n",
     "the option is on wherever cuts are held", [D], False),
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
    return sorted(set(re.findall(r"^test (\S+) \.\.\. FAILED", out, re.M)))


def running_tests(out):
    """Tests that began and never ended: what a run cut at its limit hung in."""
    began = set(re.findall(r"^test (\S+) has been running for", out, re.M))
    return sorted(began)


class Edited:
    """A file with other contents for a while."""

    def __init__(self, tree, path, change):
        self.path = os.path.join(tree, path)
        self.change = change

    def __enter__(self):
        with open(self.path) as f:
            self.before = f.read()
        after = self.change(self.before)
        if after is None:
            self.before = None
            return False
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


def scenarios(tree, target):
    """The names of the scenarios that fail, or a word for a run that did not end as a run."""
    with Edited(tree, CUTS, appended("scenarios.rs")):
        outcome, out = run(SCEN, tree, target)
    if outcome == "build":
        return None, out
    failed = set(n.rsplit("::", 1)[-1] for n in failed_tests(out))
    if outcome == "limit":
        failed |= {"(cut at the limit)"}
    return failed, out


def short(names):
    return ", ".join(sorted(re.sub(r"^review_m24d_((?:changed_)?r\d+)_.*", r"\1", n) for n in names)) or "none"


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
                    failing, out = scenarios(tree, target)
                    if failing is None:
                        detail = "scenarios: do not build with it"
                    elif failing == BASELINE:
                        detail = "scenarios: as at the head (SURVIVED)"
                    else:
                        detail = "scenarios: KILLED; failing with it: " + short(failing)
        line = "%s %s: %s%s" % (pid, result, what, (" [" + detail + "]") if detail else "")
        print(line, flush=True)
        with open(out_path, "a") as f:
            f.write(line + "\n")


def head_scenarios(tree, target):
    out_path = os.path.join(HERE, "scenarios.out")
    lines = []
    with Edited(tree, CUTS, appended("scenarios.rs")):
        outcome, out = run(SCEN, tree, target)
    lines.append("# on the simulated disk: %s" % outcome)
    lines += [l for l in out.splitlines() if re.match(r"^test .*review_m24d|^test result|^---- |.*panicked at|^\S.*the drop", l)
              or "review_m24d" in l and "assert" in l]
    lines += re.findall(r"^(?:  )?(?:left|right):.*$", out, re.M)
    with Edited(tree, REAL, appended("scenarios_real.rs")):
        outcome, out = run(SCEN_REAL, tree, target)
    lines.append("# on real files, with the test hooks: %s" % outcome)
    lines += [l for l in out.splitlines() if re.match(r"^test .*review_m24d|^test result: .* [1-9]\d* (passed|failed)", l)
              or l.startswith("review_m24d_r8") or l.startswith("skipped:")]
    text = "\n".join(lines) + "\n"
    print(text, flush=True)
    with open(out_path, "w") as f:
        f.write(text)


def variant(tree, target):
    out_path = os.path.join(HERE, "variant.out")
    old = "    start_syncing_header(disk, mutants, region, io, coming, false)\n"
    new = "    start_syncing_header(disk, mutants, region, io, coming, true)\n"
    with Edited(tree, CUTS, once(old, new)) as applied:
        if not applied:
            text = "NOT APPLIED\n"
        else:
            outcome, out = run(L + ["--", "inplace::tests::cuts::"], tree, target, limit=900)
            names = failed_tests(out)
            results = re.findall(r"^test result: .*$", out, re.M)
            text = "# every store of the cut tests syncs the header at a release: %s\n" % outcome
            text += "".join(r + "\n" for r in results)
            text += "".join("FAILED %s\n" % n for n in names)
            if outcome == "limit":
                text += "".join("RUNNING AT THE LIMIT %s\n" % n for n in running_tests(out))
            with open(os.path.join(HERE, "variant.full"), "w") as f:
                f.write(out)
    print(text, flush=True)
    with open(out_path, "w") as f:
        f.write(text)


def scenarios_with(tree, target, wanted):
    """The scenarios with one plant in place, whatever the head's tests say of it."""
    out_path = os.path.join(HERE, "scenarios_with.out")
    for pid, path, old, new, what, _tests, _scen in PLANTS:
        if pid not in wanted:
            continue
        with Edited(tree, path, once(old, new)) as applied:
            if not applied:
                line = "%s NOT APPLIED" % pid
            else:
                failing, _out = scenarios(tree, target)
                line = "%s (%s): scenarios failing with it: %s" % (
                    pid, what, "do not build" if failing is None else short(failing))
        print(line, flush=True)
        with open(out_path, "a") as f:
            f.write(line + "\n")


def main():
    tree, target, mode = sys.argv[1], sys.argv[2], sys.argv[3]
    if status(tree):
        sys.exit("the worktree is not clean")
    try:
        if mode == "plants":
            plants(tree, target, set(sys.argv[4:]))
        elif mode == "scenarios":
            head_scenarios(tree, target)
        elif mode == "variant":
            variant(tree, target)
        elif mode == "scenarios-with":
            scenarios_with(tree, target, sys.argv[4:])
        else:
            sys.exit(__doc__)
    finally:
        left = status(tree)
        print("worktree clean" if not left else "WORKTREE NOT CLEAN:\n" + left, flush=True)


if __name__ == "__main__":
    main()
