#!/usr/bin/env python3
"""Plants and scenarios for the review of #122 at 70d0f23 (M2.5-1).

    plants.py WORKTREE TARGET_DIR plants [ID ...]
    plants.py WORKTREE TARGET_DIR scenarios

`plants`: one change at a time is written into a scratch worktree of the
head, the tests of `skyzfs-upload` are run (its library's and all its
integration tests), and the file is restored. A plant is

- KILLED       when a test of the pull request's tree fails with it (or
               hangs: the run is cut at its limit, and the result says
               "by the limit"),
- SURVIVED     when every test passes with it,
- NOT APPLIED  when a text of it is not found exactly once, or it does
               not build.

For a plant that survives, the review's scenarios are run as well
(scenarios.rs appended to tests/publish.rs, scenarios_up.rs to
tests/uploader.rs), and the result names the scenarios that fail with it
beyond those that fail at the head (BASELINE: the findings).

`scenarios`: the scenarios at the head, into scenarios.out.

Each result is appended to plants.out as it comes; a plant already in
plants.out is skipped on a rerun. The worktree must be clean at the start
and is checked clean at the end.
"""
import os
import re
import signal
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))

PUBLISH = "crates/skyzfs-upload/src/publish.rs"
DRIVER = "crates/skyzfs-upload/src/driver.rs"
REMOTE = "crates/skyzfs-upload/src/remote.rs"
LOCAL = "crates/skyzfs-upload/src/local.rs"
UPLOADER = "crates/skyzfs-upload/src/uploader.rs"
T_PUBLISH = "crates/skyzfs-upload/tests/publish.rs"
T_UPLOADER = "crates/skyzfs-upload/tests/uploader.rs"

TESTS = ["cargo", "test", "-p", "skyzfs-upload", "--no-fail-fast"]
SCEN = ["cargo", "test", "-p", "skyzfs-upload", "--test", "publish", "--test", "uploader", "--no-fail-fast",
        "--", "review_m25", "--nocapture", "--test-threads=1"]
LIMIT = 300
# The scenarios that fail at the head: each shows a finding of review.md.
BASELINE = {"r1", "r10", "r2", "r3", "r8", "u3", "u4"}

CARRIED = ("        cut.cut.state == self.state\n"
           "            && cut.cut.number > self.after\n"
           "            && cut.cut.group >= self.group\n"
           "            && cut.itself\n")
RECORDED = ("        self.carried_by(&cut)\n"
            "            && row.manifest_len as usize == self.manifest.len()\n"
            "            && row.manifest_hash == *blake3::hash(&self.manifest).as_bytes()\n")
GIVEN = "            if pending.stage == Stage::Wait && pending.request.carried_by(&known) {\n"

# (id, [(file, old, new), ...], what the plant is)
PLANTS = [
    # ---- the rule: Request::carried_by
    ("A01", [(PUBLISH, CARRIED, CARRIED.replace("        cut.cut.state == self.state\n            && ", "        "))],
     "a cut of another cut state carries a publish"),
    ("A02", [(PUBLISH, "            && cut.cut.number > self.after\n", "            && cut.cut.number >= self.after\n")],
     "the cut counted as C itself carries a publish"),
    ("A03", [(PUBLISH, "            && cut.cut.number > self.after\n", "")],
     "a cut counted before C carries a publish (the rewind case)"),
    ("A04", [(PUBLISH, "            && cut.cut.group >= self.group\n", "            && cut.cut.group > self.group\n")],
     "a cut of group G itself does not carry a publish"),
    ("A05", [(PUBLISH, "            && cut.cut.group >= self.group\n", "")],
     "a cut of a group before G carries a publish"),
    ("A06", [(PUBLISH, "            && cut.cut.group >= self.group\n            && cut.itself\n",
              "            && cut.cut.group >= self.group\n")],
     "a cut that does not open as itself carries a publish"),
    ("A07", [(DRIVER, "            itself: cut.itself,\n", "            itself: true,\n")],
     "the driver hands every committed cut on as one that opens as itself"),
    ("A08", [(LOCAL, "            itself: first.itself,\n", "            itself: true,\n")],
     "the store's cut is offered as one that opens as itself, whatever it is"),
    # ---- what records a publish: Request::recorded_in, Publishing::found
    ("B01", [(PUBLISH, RECORDED, "        self.carried_by(&cut)\n")],
     "a ref found records the request whatever its manifest"),
    ("B02", [(PUBLISH, "            && row.manifest_hash == *blake3::hash(&self.manifest).as_bytes()\n", "")],
     "a ref found records the request where the manifest's length is the same"),
    ("B03", [(PUBLISH, RECORDED,
              "        let _ = cut;\n        row.manifest_len as usize == self.manifest.len()\n"
              "            && row.manifest_hash == *blake3::hash(&self.manifest).as_bytes()\n")],
     "a ref found records the request whatever cut its row names"),
    ("B04", [(PUBLISH, "            .and_then(|bytes| SnapRow::decode(bytes).ok())\n"
              "            .filter(|row| pending.request.recorded_in(row, found.commit));\n",
              "            .and_then(|bytes| SnapRow::decode(bytes).ok());\n")],
     "any ref found with a row that reads is taken as this publish, done"),
    ("B05", [(PUBLISH, "                return if request.recorded_in(row, *commit) {\n",
              "                return if true || request.recorded_in(row, *commit) {\n")],
     "a name this uploader published answers done to any request"),
    # ---- the answers at once
    ("C01", [(PUBLISH, "        if self.attachment != Some(request.attachment) {\n"
              "            return Answer::Refused(Refusal::Attachment);\n        }\n", "")],
     "a request of another attachment is accepted"),
    ("C02", [(PUBLISH, "        if self.state.is_some_and(|state| state != request.state) {\n"
              "            return Answer::Refused(Refusal::State);\n        }\n", "")],
     "a request of another cut state is accepted (and waits for ever)"),
    ("C03", [(DRIVER, "        self.publishing.set_state(cut.state);\n", "")],
     "the driver never tells the publishes the store's cut state"),
    ("C04", [(PUBLISH, "        if request.manifest.len() > MANIFEST_MAX {\n",
              "        if request.manifest.len() > MANIFEST_MAX + 1 {\n")],
     "a manifest one byte over the limit is accepted"),
    ("C05", [(PUBLISH, "            return if pending.request == request {\n",
              "            return if pending.request.name == request.name {\n")],
     "a pending name asked with other parameters is accepted again, and the first parameters stay"),
    ("C06", [(PUBLISH, "        if self.pending.len() >= PENDING_MAX {\n", "        if self.pending.len() > PENDING_MAX {\n")],
     "one more publish than the bound may pend"),
    ("C07", [(PUBLISH, "            Some(Outcome::Refused(_)) => {\n"
              "                self.outcomes.remove(&request.name);\n"
              "                self.order.retain(|name| *name != request.name);\n            }\n",
              "            Some(Outcome::Refused(_)) => return Answer::Refused(Refusal::Taken),\n")],
     "a name refused once is refused for good, not looked up anew"),
    ("C08", [(PUBLISH, "    !name.is_empty()\n        && name.len() <= 128\n",
              "    !name.is_empty()\n        && name.len() <= 129\n")],
     "a name of 129 bytes is valid"),
    # ---- the cuts in view and the first that carries
    ("E01", [(PUBLISH, "        if self.known.len() == KNOWN_CUTS {\n            self.known.pop_front();\n        }\n", "")],
     "the cuts in view have no bound"),
    ("E02", [(PUBLISH, GIVEN, "            if pending.stage == Stage::Wait {\n")],
     "a publish that waits takes the next committed cut, whatever it is"),
    ("E03", [(PUBLISH, GIVEN,
              "            if matches!(pending.stage, Stage::Wait | Stage::Ready(_)) && pending.request.carried_by(&known) {\n")],
     "a later cut takes a publish from the first cut that carries it"),
    ("E04", [(PUBLISH, GIVEN,
              "            if matches!(pending.stage, Stage::Wait | Stage::Look) && pending.request.carried_by(&known) {\n")],
     "a cut is given to a publish not looked up yet: its transaction is made with no lookup"),
    ("E05", [(PUBLISH, GIVEN, "            if false && pending.request.carried_by(&known) {\n")],
     "a cut committed is given to no publish that waits"),
    ("E06", [(PUBLISH, "            .iter()\n            .find(|known| self.pending[i].request.carried_by(known))\n",
              "            .iter()\n            .rev()\n            .find(|known| self.pending[i].request.carried_by(known))\n")],
     "a publish asked late goes on the newest known cut that carries it, not the first"),
    ("E07", [(PUBLISH, "            (_, Some(known)) => Stage::Ready(known),\n", "            (_, Some(_)) => Stage::Wait,\n")],
     "a publish asked late never goes on a cut known: it waits for the next"),
    ("E08", [(PUBLISH, "            (Stage::Recheck(known), _) => Stage::Ready(known),\n", "")],
     "after a failed transaction the publish goes on the first known cut, not on its own "
     "(no difference expected: no cut is committed while a publish has work)"),
    ("E09", [(PUBLISH, "            .filter(|p| p.stage != Stage::Wait)\n            .min_by_key(|p| p.asked)\n",
              "            .filter(|p| p.stage != Stage::Wait)\n            .max_by_key(|p| p.asked)\n")],
     "the newest publish with work is stepped first, not the oldest"),
    # ---- the transaction, its failure and its lookup
    ("G01", [(PUBLISH, "        if let Stage::Ready(known) = self.pending[i].stage {\n"
              "            self.pending[i].stage = Stage::Recheck(known);\n        }\n", "")],
     "a failed transaction is made again with no lookup"),
    ("G02", [(DRIVER, "            PublishStage::Look | PublishStage::Recheck(_) => {\n", "            PublishStage::Look => {\n"),
             (DRIVER, "            PublishStage::Ready(known) => known,\n",
              "            PublishStage::Ready(known) | PublishStage::Recheck(known) => known,\n")],
     "the lookup after a failed transaction is not made"),
    ("G03", [(DRIVER, "                self.publishing.published(&name, row, known.commit);\n", "")],
     "a transaction that landed leaves the publish pending"),
    ("G04", [(DRIVER, "                self.committed(&cut, seen.head);\n", "")],
     "a cut found committed at its check is not handed to the publishes"),
    ("G05", [(DRIVER, "        self.committed(&upload.cut, commit);\n", "")],
     "a cut committed by this driver is not handed to the publishes"),
    ("G06", [(REMOTE, "            TxnOutcome::Conflict { .. } => Err(RemoteError::Conflict),\n",
              "            TxnOutcome::Conflict { .. } => Ok(()),\n")],
     "a transaction refused because the name exists reads as published"),
    ("G07", [(REMOTE, "            RefOp::MetaPut {\n                key: snap_row_key(snapshot.name),\n"
              "                value: snapshot.row.encode(),\n            },\n", "")],
     "the transaction writes the ref and no row"),
    ("G08", [(REMOTE, "        self.store\n            .put(\n                &manifest_key(self.vol, snapshot.snap),\n"
              "                snapshot.manifest.to_vec().into(),\n            )\n            .await?;\n", "")],
     "the ZFS manifest is not PUT"),
    ("G09", [(REMOTE, "        self.store\n            .put(\n"
              "                &skynbd_engine::keys::snapshot(self.vol, snapshot.snap),\n"
              "                encoded.into(),\n            )\n            .await?;\n", "        let _ = encoded;\n")],
     "SkyNBD's manifest is not PUT"),
    ("G10", [(REMOTE, "            ref_name: ref_name.clone(),\n            commit,\n",
              "            ref_name: snapshot.name.to_string(),\n            commit,\n")],
     "SkyNBD's manifest names the snapshot without the snap/ prefix"),
    ("G11", [(REMOTE, "        let Some(value) = self.refs.get(self.vol, &snap_ref(name)).await? else {\n",
              "        let Some(value) = self.refs.get(self.vol, &snap_ref(name)).await?.filter(|_| false) else {\n")],
     "the lookup never finds the ref"),
    ("G12", [(REMOTE, "        let row = self.refs.meta_get(self.vol, &snap_row_key(name)).await?;\n",
              "        let row = self.refs.meta_get(self.vol, &snap_row_key(name)).await?.filter(|_| false);\n")],
     "the lookup never finds the row"),
    ("G13", [(PUBLISH, "            manifest_hash: *blake3::hash(manifest).as_bytes(),\n        };\n"
              "        Some((pending.snap, row, manifest))",
              "            manifest_hash: *blake3::hash(b\"\").as_bytes(),\n        };\n"
              "        Some((pending.snap, row, manifest))")],
     "the row carries the hash of an empty manifest"),
    # ---- the commit a publish names
    ("H01", [(REMOTE, "        Ok(engine.commit_with(CommitKind::Flush, ops).await?)\n",
              "        let before = engine.head_commit_hash();\n"
              "        engine.commit_with(CommitKind::Flush, ops).await?;\n        Ok(before)\n")],
     "a commit answers with the hash of the commit before it"),
    ("H02", [(REMOTE, "                    current: false,\n                    head: head.commit,\n",
              "                    current: false,\n                    head: engine.head_commit_hash(),\n")],
     "the read after a failed commit gives the engine's head, the commit before the cut's, as head"),
    # ---- the row's format
    ("I01", [(PUBLISH, "        if version != SNAP_ROW_VERSION {\n            return Err(SnapRowError::Version(version));\n        }\n"
              "        if bytes.len() != SNAP_ROW_LEN {\n            return Err(SnapRowError::Length(bytes.len()));\n        }\n",
              "        if bytes.len() != SNAP_ROW_LEN {\n            return Err(SnapRowError::Length(bytes.len()));\n        }\n"
              "        if version != SNAP_ROW_VERSION {\n            return Err(SnapRowError::Version(version));\n        }\n")],
     "a row's length is judged before its version"),
    ("I02", [(PUBLISH, "pub const SNAP_ROW_PREFIX: &str = \"skyzfs/snap/\";\n", "pub const SNAP_ROW_PREFIX: &str = \"skyzfs/snaps/\";\n")],
     "the row's key has another prefix"),
    # ---- the figures and the view of outcomes
    ("J01", [(PUBLISH, "        self.figures.published += 1;\n", "")], "the publishes made are not counted"),
    ("J02", [(PUBLISH, "                self.figures.found += 1;\n", "")], "the publishes found are not counted"),
    ("J03", [(PUBLISH, "        self.figures.accepted += 1;\n", "")], "the requests accepted are not counted"),
    ("J04", [(PUBLISH, "        if matches!(answer, Answer::Refused(_)) {\n            self.figures.refused += 1;\n        }\n", "")],
     "the requests refused at once are not counted"),
    ("J05", [(PUBLISH, "                self.figures.refused += 1;\n                self.end(name, Outcome::Refused(Refusal::Taken));\n",
              "                self.end(name, Outcome::Refused(Refusal::Taken));\n")],
     "the requests refused once looked up are not counted"),
    ("J06", [(PUBLISH, "            .min_by_key(|p| p.asked)\n            .map(|p| (p.request.name.as_str(), p.asked.elapsed()));\n",
              "            .max_by_key(|p| p.asked)\n            .map(|p| (p.request.name.as_str(), p.asked.elapsed()));\n")],
     "the status names the newest pending publish as the oldest"),
    ("J07", [(PUBLISH, "        while self.order.len() > OUTCOMES_KEPT {\n", "        while self.order.len() > OUTCOMES_KEPT + 1 {\n")],
     "one more outcome than the bound is kept"),
    ("J08", [(PUBLISH, "        if let Outcome::Done(row, commit) = &outcome {\n"
              "            self.figures.last = Some((name.to_string(), row.cut, *commit));\n        }\n", "")],
     "the last snapshot published is not kept for the status"),
    # ---- through the uploader
    ("K01", [(UPLOADER, "                let publishing = self.publishing.take().unwrap_or_default();\n",
              "                let publishing = Publishing::default();\n")],
     "the publishes asked before the driver is made, and the attachment set then, are dropped"),
    ("K02", [(UPLOADER, "            Some(ref mut driver) => driver.publishing(),\n"
              "            None => self.publishing.get_or_insert_with(Publishing::default),\n",
              "            _ => self.publishing.get_or_insert_with(Publishing::default),\n")],
     "once the driver is made, the uploader's publishes are not the driver's: nothing asked then is ever made"),
]


def status(tree):
    return subprocess.run(["git", "-C", tree, "status", "--short"],
                          capture_output=True, text=True).stdout.strip()


def run(cmd, tree, target, limit=LIMIT):
    """(outcome, output): 'ok', 'fail', 'limit' or 'build'."""
    env = dict(os.environ, CARGO_TARGET_DIR=target, RUST_BACKTRACE="0")
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
    if "could not compile" in out:
        return "build", out
    return "fail", out


def all_failed(out):
    names = set(re.findall(r"^test (\S+) \.\.\. FAILED", out, re.M))
    for block in out.split("\nfailures:\n")[1:]:
        for line in block.splitlines():
            m = re.match(r"^    (\S+)$", line)
            if m:
                names.add(m.group(1))
            elif line.strip() and not line.startswith("    "):
                break
    return sorted(names)


class Edited:
    """Files with other contents for a while."""

    def __init__(self, tree, changes):
        self.tree = tree
        self.changes = changes
        self.before = {}

    def __enter__(self):
        after = {}
        for path, change in self.changes:
            full = os.path.join(self.tree, path)
            if full not in after:
                with open(full) as f:
                    self.before[full] = f.read()
                after[full] = self.before[full]
            text = change(after[full])
            if text is None:
                self.before = {}
                return False
            after[full] = text
        for full, text in after.items():
            with open(full, "w") as f:
                f.write(text)
        return True

    def __exit__(self, *exc):
        for full, text in self.before.items():
            with open(full, "w") as f:
                f.write(text)


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


SCENARIOS = [(T_PUBLISH, appended("scenarios.rs")), (T_UPLOADER, appended("scenarios_up.rs"))]


def short(name):
    return re.sub(r"^.*review_m25_([ru]\d+)_.*", r"\1", name)


def scenarios(tree, target):
    """The scenarios that fail (r1, u3, ...), or None for a run that did not build; and the output."""
    with Edited(tree, SCENARIOS):
        outcome, out = run(SCEN, tree, target)
    if outcome == "build":
        return None, out
    failed = set(short(n) for n in all_failed(out))
    if outcome == "limit":
        failed.add("(cut at the limit)")
    return failed, out


def plants(tree, target, wanted):
    out_path = os.path.join(HERE, "plants.out")
    done = set()
    if os.path.exists(out_path):
        with open(out_path) as f:
            done = set(line.split()[0] for line in f if line.strip())
    for pid, edits, what in PLANTS:
        if wanted and pid not in wanted:
            continue
        if pid in done and not wanted:
            continue
        result, detail = "SURVIVED", ""
        with Edited(tree, [(path, once(old, new)) for path, old, new in edits]) as applied:
            if not applied:
                result, detail = "NOT APPLIED", "a text of it is not there exactly once"
            else:
                outcome, out = run(TESTS, tree, target)
                if outcome == "build":
                    result = "NOT APPLIED"
                    detail = "does not build: " + " | ".join(
                        l for l in out.splitlines() if l.startswith("error"))[:300]
                elif outcome == "limit":
                    result = "KILLED"
                    detail = "by the limit of %d s (%d failed before it)" % (LIMIT, len(all_failed(out)))
                elif outcome == "fail":
                    names = all_failed(out)
                    result = "KILLED"
                    detail = "%d failed: %s" % (len(names), ", ".join(n.rsplit("::", 1)[-1] for n in names[:5]))
                else:
                    failing, out = scenarios(tree, target)
                    if failing is None:
                        detail = "scenarios: do not build with it"
                    elif failing == BASELINE:
                        detail = "scenarios: as at the head"
                    else:
                        more = sorted(failing - BASELINE)
                        fewer = sorted(BASELINE - failing)
                        detail = "scenarios: " + "; ".join(
                            x for x in ["fail with it: " + ", ".join(more) if more else "",
                                        "pass with it, fail at the head: " + ", ".join(fewer) if fewer else ""] if x)
        line = "%s %s: %s%s" % (pid, result, what, (" [" + detail + "]") if detail else "")
        print(line, flush=True)
        with open(out_path, "a") as f:
            f.write(line + "\n")


def head_scenarios(tree, target):
    failing, out = scenarios(tree, target)
    keep = re.compile(r"^test .*review_m25|^test result|^[ru]\d+: |.*panicked at|^assertion|^  left|^ right|^\d+ publishes")
    lines = []
    for l in out.splitlines():
        if keep.match(l):
            l = re.sub(r"\(\d+\) panicked", "panicked", l)
            l = re.sub(r"commit: \[(\d+), (\d+), [0-9, ]+\]", r"commit: [\1, \2, ..]", l)
            l = re.sub(r"Done\(\[(\d+), (\d+), [0-9, ]+\]\)", r"Done([\1, \2, ..])", l)
            lines.append(l)
    lines.append("# failing at the head: %s" % (", ".join(sorted(failing)) if failing is not None else "did not build"))
    with open(os.path.join(HERE, "scenarios.out"), "w") as f:
        f.write("\n".join(lines) + "\n")
    print("\n".join(lines))


def main():
    tree, target, mode = sys.argv[1], sys.argv[2], sys.argv[3]
    if status(tree):
        sys.exit("the worktree is not clean:\n" + status(tree))
    try:
        if mode == "plants":
            plants(tree, target, set(sys.argv[4:]))
        elif mode == "scenarios":
            head_scenarios(tree, target)
        else:
            sys.exit("plants or scenarios")
    finally:
        left = status(tree)
        print("the worktree after: %s" % ("clean" if not left else "NOT CLEAN:\n" + left))


if __name__ == "__main__":
    main()
