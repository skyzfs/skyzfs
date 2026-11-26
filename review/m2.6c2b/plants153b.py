#!/usr/bin/env python3
"""Next look at #153 (M2.6c-2b), head 3d40045: plants on a scratch copy of
the head, each applied alone as text, then `cargo test -p skyzfs-agent
--lib` (600 s limit), then restored in a finally. One JSON line per
plant: KILLED (a test failed, named; or the build failed), SURVIVED, or
an error.

T1: the review's P3, P5, P7 (and P2, P6, P8 again) at their sites in this
head. D*: the bound on failed checks (D1 of the review at 65d72ee): the
author's eleven, re-run, and the review's own. L*: the limits (N2).

usage: python3 -I plants153b.py WORKTREE [ID,ID...]
"""
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(sys.argv[1])
AG = "crates/skyzfs-agent/src"
PLAN = f"{AG}/plan.rs"
ENG = f"{AG}/engine.rs"
ZFS = f"{AG}/zfs.rs"
CLI = f"{AG}/zfs/cli.rs"
REC = f"{AG}/record.rs"

GIVE_UP_DESTROY = """        self.point(Effect::Destroy, false)?;
        let destroyed = self.zfs.destroy(&self.pool, name);
        self.point(Effect::Destroy, true)?;
        let why = if destroyed.is_ok() {"""

PLANTS = [
    # T1 and T2, at this head's sites.
    ("P2", "as_before: the GUID compare dropped", ENG,
     "    dataset.guid == member.guid\n        && dataset.name == member.name",
     "    dataset.name == member.name"),
    ("P3", "as_before: the name compare dropped", ENG,
     "        && dataset.name == member.name\n",
     "\n"),
    ("P5", "as_before: the count of datasets not compared", ENG,
     "    members.len() == before.len()\n        && members",
     "    members"),
    ("P6", "as_before: the type not compared", ENG,
     "        && dataset.kind == member.kind\n",
     "\n"),
    ("P7", "take(): no early return where the reading shows pool@t", ENG,
     "        if zfs::taken(&before, &self.pool) != zfs::Taken::Absent {",
     "        if false && zfs::taken(&before, &self.pool) != zfs::Taken::Absent {"),
    ("P8", "rename(): the reading before the old name's take kept", ENG,
     "        memory.before = None;\n        if failed {",
     "        if failed {"),
    # The author's D1 plants, re-run.
    ("D1a", "no bound (the loop of the review)", ENG,
     "        if failed < plan::CHECKS_FAILED_MAX {",
     "        if true || failed < plan::CHECKS_FAILED_MAX {"),
    ("D1b", "the bound one more", ENG,
     "        if failed < plan::CHECKS_FAILED_MAX {",
     "        if failed <= plan::CHECKS_FAILED_MAX {"),
    ("D1c", "the bound one fewer", ENG,
     "        if failed < plan::CHECKS_FAILED_MAX {",
     "        if failed + 1 < plan::CHECKS_FAILED_MAX {"),
    ("D1d", "no pause (the deadline never set)", ENG,
     "            memory.retake_after = Some(self.clock.mono_ms().saturating_add(ms(plan::RETAKE_PAUSE)));",
     "            let _ = plan::RETAKE_PAUSE;"),
    ("D1e", "a pause of 1 s", PLAN,
     "pub const RETAKE_PAUSE: Duration = ASSIST_AFTER;",
     "pub const RETAKE_PAUSE: Duration = Duration::from_secs(1);"),
    ("D1f", "the pause ignored in take", ENG,
     "        if retake_after.is_some_and(|after| self.clock.mono_ms() < after) {",
     "        if false && retake_after.is_some_and(|after| self.clock.mono_ms() < after) {"),
    ("D1g", "the last take kept (not destroyed at the give-up)", ENG,
     GIVE_UP_DESTROY,
     """        self.point(Effect::Destroy, false)?;
        let destroyed: Result<(), ZfsError> = Ok(());
        self.point(Effect::Destroy, true)?;
        let why = if destroyed.is_ok() {"""),
    ("D1h", "no report at the give-up", ENG,
     "        self.reports\n            .push((name.to_string(), Report::NotWhole(names)));",
     "        let _ = names;"),
    ("D1i", "a failed destroy at the give-up taken for done", ENG,
     "            Why::NotWhole\n        } else {\n            Why::ManifestLost\n        };",
     "            Why::NotWhole\n        } else {\n            Why::NotWhole\n        };"),
    ("D1j", "the failed checks not counted in the record", ENG,
     "                if failed {\n                    p.attempts = p.attempts.saturating_add(1);\n                }",
     "                let _ = failed;"),
    ("D1k", "every retake counted (a take with no reading too)", ENG,
     "            None => return self.retake(name, None),",
     "            None => return self.failed(name, Vec::new()),"),
    # The review's own.
    ("D1l", "a take not whole with no reading in this process not counted", ENG,
     "            zfs::Taken::NotWhole(breaking) => return self.failed(name, breaking),",
     "            zfs::Taken::NotWhole(breaking) => {\n                if self.memory.get(name).and_then(|m| m.before.as_ref()).is_none() {\n                    let _ = breaking;\n                    return self.retake(name, None);\n                }\n                return self.failed(name, breaking);\n            }"),
    ("D1m", "the report of a take not as before names nothing", ENG,
     "                let differ = differing(&program.members, before);",
     "                let differ: Vec<String> = Vec::new();"),
    ("D1n", "differing(): the datasets read before and missing from the take not named", ENG,
     "    names.extend(\n",
     "    names.truncate(names.len());\n    let _ = (\n"),
    ("D1o", "the count read from the stalls, not attempts", ENG,
     "            .map_or(0, |p| p.attempts)",
     "            .map_or(0, |p| p.stalls)"),
    ("D1p", "the give-up's report made after the abandon (lost on a stop between)", ENG,
     "        self.reports\n            .push((name.to_string(), Report::NotWhole(names)));\n" + GIVE_UP_DESTROY
     + "\n            Why::NotWhole\n        } else {\n            Why::ManifestLost\n        };\n        self.abandon(name, why)?;",
     GIVE_UP_DESTROY
     + "\n            Why::NotWhole\n        } else {\n            Why::ManifestLost\n        };\n        self.abandon(name, why)?;"
     + "\n        self.reports\n            .push((name.to_string(), Report::NotWhole(names)));"),
    ("D1q", "taken(): only the first dataset that breaks the take named", ZFS,
     "    if !breaking.is_empty() {\n",
     "    breaking.truncate(1);\n    if !breaking.is_empty() {\n"),
    ("D1r", "the pause applied to every take, not only after a failed check", ENG,
     "        if failed {\n            memory.retake_after",
     "        if true {\n            memory.retake_after"),
    ("D1s", "a retake for no reading keeps the failed checks' pause off (pause reset)", ENG,
     "        let mut memory = self.memory.remove(name).unwrap_or_default();\n        memory.before = None;",
     "        let mut memory = self.memory.remove(name).unwrap_or_default();\n        memory.before = None;\n        memory.retake_after = None;"),
    # The record.
    ("R1", "Why::NotWhole inserted before Why::Stopped (a variant's index moved)", REC,
     "    Stopped,\n    /// [`crate::plan::CHECKS_FAILED_MAX`] takes in a row were not one\n    /// whole take of the pool, or their members not as read before them\n    /// (a dataset that `zfs snapshot -r` skips, for one); the last was\n    /// destroyed (the lead's ruling on D1 of #153's review).\n    NotWhole,\n",
     "    /// Not whole (plant).\n    NotWhole,\n    /// Stopped (plant).\n    Stopped,\n"),
    # The limits.
    ("L1", "a failed count keeps the last limits", CLI,
     "            Err(_) => Scale::Base,",
     "            Err(_) => self.scale,"),
    ("L2", "a failed count gives the cap (the first ruling)", CLI,
     "            Err(_) => Scale::Base,",
     "            Err(_) => Scale::Datasets(u64::MAX),"),
    ("L3", "a read scaled as a change", CLI,
     "        self.scaled(self.limits.read, self.limits.read_per_dataset)",
     "        self.scaled(self.limits.read, self.limits.per_dataset)"),
    ("L4", "a change scaled as a read", CLI,
     "        self.scaled(self.limits.change, self.limits.per_dataset)",
     "        self.scaled(self.limits.change, self.limits.read_per_dataset)"),
    ("L5", "the cap at 20 minutes", CLI,
     "            most: Duration::from_secs(35 * 60),",
     "            most: Duration::from_secs(20 * 60),"),
    ("L6", "a read at 100 ms a dataset", CLI,
     "            read_per_dataset: Duration::from_millis(180),",
     "            read_per_dataset: Duration::from_millis(100),"),
    ("L7", "a read at 179 ms a dataset (still above twice the measured read)", CLI,
     "            read_per_dataset: Duration::from_millis(180),",
     "            read_per_dataset: Duration::from_millis(179),"),
]


def run(plant):
    pid, what, rel, old, new = plant
    path = ROOT / rel
    text = path.read_text()
    if text.count(old) != 1:
        return {"id": pid, "what": what, "result": f"ERROR: site found {text.count(old)} times"}
    try:
        path.write_text(text.replace(old, new))
        try:
            out = subprocess.run(
                ["cargo", "test", "-p", "skyzfs-agent", "--lib", "--", "--test-threads=4"],
                cwd=ROOT, capture_output=True, text=True, timeout=600)
        except subprocess.TimeoutExpired:
            return {"id": pid, "what": what, "result": "KILLED (timeout 600 s)"}
        lines = (out.stdout + out.stderr).splitlines()
        failed = [l.split()[1] for l in lines if l.startswith("test ") and l.endswith("FAILED")]
        if "could not compile" in out.stderr:
            errs = [l for l in out.stderr.splitlines() if l.startswith("error")][:3]
            return {"id": pid, "what": what, "result": "KILLED (build)", "by": errs}
        if out.returncode != 0:
            return {"id": pid, "what": what, "result": "KILLED", "by": failed}
        passed = [l for l in lines if l.startswith("test result")]
        return {"id": pid, "what": what, "result": "SURVIVED", "tests": passed}
    finally:
        path.write_text(text)


def main():
    only = set(sys.argv[2].split(",")) if len(sys.argv) > 2 else None
    for plant in PLANTS:
        if only and plant[0] not in only:
            continue
        print(json.dumps(run(plant)), flush=True)
    st = subprocess.run(["git", "status", "--short"], cwd=ROOT, capture_output=True, text=True)
    print(json.dumps({"worktree after": st.stdout.strip() or "clean"}))


main()
