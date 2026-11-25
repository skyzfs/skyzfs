#!/usr/bin/env python3
"""Review of #153 (M2.6c-2b) at 65d72ee: plants on a scratch copy of the
head, each applied alone as text, then `cargo test -p skyzfs-agent --lib`
(600 s limit), then restored in a finally. One JSON line per plant:
KILLED (a test failed, named, or the build failed), SURVIVED, or an
error.

B*, L*, R*, M5, M6, A12 are the author's (a sample, as the description
names them); P* are this review's.

usage: python3 -I plants153.py WORKTREE [ID,ID...]
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
LUA = f"{AG}/zfs/cli/members.lua"

PLANTS = [
    # The author's, re-run.
    ("B1", "the members not compared with the reading before the take", ENG,
     "        if !as_before(&program.members, before) {",
     "        if false && !as_before(&program.members, before) {"),
    ("B2", "a take with no reading from before it adopted", ENG,
     "    let Some(before) = before else {\n        return false;\n    };",
     "    let Some(before) = before else {\n        return true;\n    };"),
    ("B3", "EEXIST at the take moves to the next name (the old rule)", ENG,
     "            Err(ZfsError::Exists) => self.adopt(name, Origin::Exists),",
     "            Err(ZfsError::Exists) => self.rename(name),"),
    ("B4", "a take that is not whole moves to the next name without a destroy", ENG,
     "            zfs::Taken::NotWhole(_) => return self.retake(name),",
     "            zfs::Taken::NotWhole(_) => return self.rename(name),"),
    ("B7", "Intent with pool@t gives up (the old rule)", PLAN,
     "            (Some(_), _) => Action::Adopt,",
     "            (Some(_), _) => give_up(pool, Why::ManifestLost),"),
    ("B8", "a publish too old at Intent with pool@t adopted", PLAN,
     "            (Some(_), _) if seen.too_old => give_up(pool, Why::TooOld),\n",
     ""),
    ("B9", "a destroy that failed moves to the next name", ENG,
     "        if destroyed.is_err() {\n            self.abandon(name, Why::ManifestLost)?;",
     "        if destroyed.is_err() && false {\n            self.abandon(name, Why::ManifestLost)?;"),
    ("B9b", "a destroy that failed is tried again at the next round", ENG,
     "        if destroyed.is_err() {\n            self.abandon(name, Why::ManifestLost)?;\n            return Ok(Moved::Yes);",
     "        if destroyed.is_err() {\n            return Ok(Moved::No);"),
    ("B11", "<= to <: a dataset of the take's own group without @t read as whole", ZFS,
     "            None if dataset.createtxg <= group => {",
     "            None if dataset.createtxg < group => {"),
    ("B13", "the round gives held no members", ENG,
     "        let hold = self.zfs.held(pool, name, HOLD_TAG, snapshot.as_deref())?;",
     "        let hold = self.zfs.held(pool, name, HOLD_TAG, None)?;"),
    ("L1", "the limits not scaled", CLI,
     "            Ok(n) => Scale::Datasets(n),",
     "            Ok(_) => Scale::Base,"),
    ("L3", "a failed count keeps the last", CLI,
     "            Err(_) => Scale::Most,",
     "            Err(_) => self.scale,"),
    ("L7", "one count for all the rounds", ENG,
     "        loop {\n            // Where the count fails, the limits are the longest, and the\n            // round's own calls say what is wrong (no pool, or another\n            // error): the count is not an error of the round.\n            let _counted = self.zfs.count(&self.pool);\n            self.sweep()?;",
     "        let _counted = self.zfs.count(&self.pool);\n        loop {\n            self.sweep()?;"),
    ("R1", "EEXIST at the take read as another error", CLI,
     "            return Err(ZfsError::Exists);\n        }\n        Err(failure(\"zfs\", &args, &output, false))",
     "            return Err(failure(\"zfs\", &args, &output, false));\n        }\n        Err(failure(\"zfs\", &args, &output, false))"),
    ("M5", "pending with a part plans Release (FL-T2)", PLAN,
     "        Reply::Pending => {\n            if pool.hold != Held::All || broken.is_some() || moved {",
     "        Reply::Pending => {\n            if pool.hold == Held::Part {\n                return Action::Release;\n            }\n            if pool.hold != Held::All || broken.is_some() || moved {"),
    ("M6", "a part with pool@t gone gives up without a release (FL-F1)", PLAN,
     "fn give_up(pool: &PoolSeen, why: Why) -> Action {\n    if pool.hold != Held::No {",
     "fn give_up(pool: &PoolSeen, why: Why) -> Action {\n    if pool.hold == Held::All || (pool.hold == Held::Part && pool.snapshot.is_some()) {"),
    ("A12", "asking anew takes a part as held (FL-T1)", ENG,
     ".held(&self.pool, &name, HOLD_TAG, members.as_deref())?\n            == Held::All;",
     ".held(&self.pool, &name, HOLD_TAG, members.as_deref())?\n            != Held::No;"),
    # This review's.
    ("P1", "taken(): the group not compared (a member's @t of another group accepted)", ZFS,
     "            Some(at) if at.createtxg != group => {",
     "            Some(at) if false && at.createtxg != group => {"),
    ("P2", "as_before: the GUID compare dropped", ENG,
     "                dataset.guid == member.guid\n                    && dataset.name == member.name",
     "                dataset.name == member.name"),
    ("P3", "as_before: the name compare dropped", ENG,
     "                    && dataset.name == member.name\n",
     "\n"),
    ("P4", "as_before: the local properties not compared", ENG,
     "                    && dataset.local == member.local\n",
     "\n"),
    ("P5", "as_before: the count of datasets not compared (a dataset gone since the reading accepted)", ENG,
     "    members.len() == before.len()\n        && members",
     "    members"),
    ("P6", "as_before: the type not compared", ENG,
     "                    && dataset.kind == member.kind\n",
     "\n"),
    ("P7", "take(): a reading with pool@t there kept as the reading before (no early return)", ENG,
     "        if zfs::taken(&before, &self.pool) != zfs::Taken::Absent {",
     "        if false && zfs::taken(&before, &self.pool) != zfs::Taken::Absent {"),
    ("P8", "rename(): the reading before the old name's take kept for the new name", ENG,
     "            memory.before = None;\n            self.memory.insert(next, memory);",
     "            self.memory.insert(next, memory);"),
    ("P9", "EEXIST with no pool@t: destroyed (someone else's member @t)", ENG,
     "                    Origin::Exists => self.rename(name),",
     "                    Origin::Exists => self.retake(name),"),
    ("P10", "EEXIST with no pool@t: the same name kept", ENG,
     "                    Origin::Exists => self.rename(name),",
     "                    Origin::Exists => Ok(Moved::No),"),
    ("P12", "the limits off by one dataset", CLI,
     "                    .and_then(|n| self.limits.per_dataset.checked_mul(n))",
     "                    .and_then(|n| self.limits.per_dataset.checked_mul(n.saturating_add(1)))"),
    ("P13", "the limits' units: 100 us a dataset, not 100 ms", CLI,
     "            per_dataset: Duration::from_millis(100),",
     "            per_dataset: Duration::from_micros(100),"),
    ("P14", "the cap dropped", CLI,
     "        limit.min(self.limits.most)",
     "        limit"),
    ("P15", "a failed count gives 0 datasets, not the cap", CLI,
     "            Err(_) => Scale::Most,",
     "            Err(_) => Scale::Datasets(0),"),
    ("P16", "a failed count gives the bases", CLI,
     "            Err(_) => Scale::Most,",
     "            Err(_) => Scale::Base,"),
    ("P17", "a count past u32 gives no more time, not the cap", CLI,
     "                    .unwrap_or(self.limits.most);",
     "                    .unwrap_or(Duration::ZERO);"),
    ("P18", "the count's own limit scaled", CLI,
     "            .run(\"zfs\", &args, &[], self.limits.count, false)",
     "            .run(\"zfs\", &args, &[], self.read_limit(), false)"),
    ("P19", "a failed count: the base of the call, not the cap", CLI,
     "            Scale::Most => self.limits.most,",
     "            Scale::Most => base,"),
    ("P20", "members.lua full: the snapshot's createtxg read from the dataset", LUA,
     "        entry[\"snapshot_createtxg\"] = tostring(zfs.get_prop(at, \"createtxg\"))",
     "        entry[\"snapshot_createtxg\"] = tostring(zfs.get_prop(ds, \"createtxg\"))"),
    ("P21", "members.lua full: inherited system properties read as local", LUA,
     "        if source == ds and value ~= nil then",
     "        if value ~= nil then"),
    ("P22", "datasets(): the full read run without -n (may change the pool)", CLI,
     "            &[pool, t, \"full\", ASSIST_PROPERTY],\n            true,",
     "            &[pool, t, \"full\", ASSIST_PROPERTY],\n            false,"),
    ("P23", "the count without -t filesystem,volume", CLI,
     "            \"-t\",\n            \"filesystem,volume\",\n",
     ""),
    ("P24", "destroy_r: dataset does not exist read as an error", CLI,
     "        match failure(\"zfs\", &args, &output, true) {\n            ZfsError::NoSnapshot => Ok(()),",
     "        match failure(\"zfs\", &args, &output, true) {\n            ZfsError::NoSnapshot => Err(ZfsError::NoSnapshot),"),
    ("P25", "held(): no members given read as all", CLI,
     "        let Some(members) = members else {\n            return Ok(Held::Part);",
     "        let Some(members) = members else {\n            return Ok(Held::All);"),
    ("P27", "adopt: a take gone after zfs snapshot -r exited 0 moves to the next name", ENG,
     "                    Origin::Took | Origin::Seen => Ok(Moved::No),",
     "                    Origin::Took | Origin::Seen => self.rename(name),"),
    ("P28", "rename(): the old name's manifest file not removed", ENG,
     "        // take destroyed in the next process): no publish names it now.\n        self.unlink(name)?;",
     "        // take destroyed in the next process): no publish names it now."),
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
