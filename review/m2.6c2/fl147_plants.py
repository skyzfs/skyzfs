#!/usr/bin/env python3
"""Fourth look at #147: plants on the head (66b9044), each applied alone
to a scratch worktree, then `cargo test -p skyzfs-agent --lib` (600 s
limit), then restored in a finally. One JSON line per plant: KILLED (a
test failed, or the build failed: named), SURVIVED, or an error.

A1..A12 are the author's twelve, as described in the answer; M* are
this look's.

usage: python3 -I fl147_plants.py WORKTREE [ID,ID...]
"""
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(sys.argv[1])
AG = "crates/skyzfs-agent/src"
PLAN = f"{AG}/plan.rs"
ENG = f"{AG}/engine.rs"
CLI = f"{AG}/zfs/cli.rs"
FAKE = f"{AG}/engine/fake.rs"

PLANTS = [
    ("A1", "give_up releases only a whole hold", PLAN,
     "fn give_up(pool: &PoolSeen, why: Why) -> Action {\n    if pool.hold != Held::No {",
     "fn give_up(pool: &PoolSeen, why: Why) -> Action {\n    if pool.hold == Held::All {"),
    ("A2", "Done releases only a whole hold", PLAN,
     "Phase::Done { .. } if pool.hold != Held::No => Action::Release,",
     "Phase::Done { .. } if pool.hold == Held::All => Action::Release,"),
    ("A3", "away past held_until releases only a whole hold", PLAN,
     "seen.now_ms >= until && pool.hold != Held::No => Action::Release,",
     "seen.now_ms >= until && pool.hold == Held::All => Action::Release,"),
    ("A4", "taken: a part read as held", PLAN,
     "            if pool.hold != Held::All {\n                return Action::Hold;\n            }\n            if pool.load_guid",
     "            if pool.hold == Held::No {\n                return Action::Hold;\n            }\n            if pool.load_guid"),
    ("A5", "pending: a part read as held", PLAN,
     "if pool.hold != Held::All || broken.is_some() || moved {",
     "if pool.hold == Held::No || broken.is_some() || moved {"),
    ("A6", "asked anew (onward): a part read as held", PLAN,
     "        if pool.hold != Held::All {\n            return Action::Hold;\n        }\n        Action::AskAnew",
     "        if pool.hold == Held::No {\n            return Action::Hold;\n        }\n        Action::AskAnew"),
    ("A7", "F1e: the fake's held on the root's tag alone (first planted with a type error, run again)", FAKE,
     "            let members = || p.snaps.iter().filter(|(name, _)| name.ends_with(&suffix));\n            if members().all(",
     "            let members = || p.snaps.iter().filter(|(name, _)| **name == format!(\"{POOL}@{t}\"));\n            if members().all("),
    ("A8", "the fake's hold adds the tag where missing (as before)", FAKE,
     "        if !tagged.is_empty() {\n            return if lacking.is_empty() {",
     "        if !tagged.is_empty() && lacking.is_empty() {\n            return if lacking.is_empty() {"),
    ("A9", "adapter: root untagged read as none", CLI,
     "        if !tagged.contains(&snapshot) {\n            return Ok(Held::Part);",
     "        if !tagged.contains(&snapshot) {\n            return Ok(Held::No);"),
    ("A10", "adapter: pool@t gone between the reads read as none", CLI,
     "        let Some(members) = self.members(pool, t)? else {\n            return Ok(Held::Part);\n        };\n        if members",
     "        let Some(members) = self.members(pool, t)? else {\n            return Ok(Held::No);\n        };\n        if members"),
    ("A11", "adapter: a part read as all", CLI,
     "            Ok(Held::All)\n        } else {\n            Ok(Held::Part)\n        }",
     "            Ok(Held::All)\n        } else {\n            Ok(Held::All)\n        }"),
    ("A12", "ask_anew's own read: a part read as held", ENG,
     "let held = self.zfs.held(&self.pool, &name, HOLD_TAG)? == Held::All;",
     "let held = self.zfs.held(&self.pool, &name, HOLD_TAG)? != Held::No;"),
    # This look's.
    ("M1", "held, not asked, unknown: a part read as held (Ask)", PLAN,
     "None if pool.hold == Held::All && broken.is_none() && !moved => Action::Ask,",
     "None if pool.hold != Held::No && broken.is_none() && !moved => Action::Ask,"),
    ("M2", "adapter: nothing tagged read as a part (no early No)", CLI,
     "        if tagged.is_empty() {\n            return Ok(Held::No);\n        }\n        if !tagged.contains",
     "        if !tagged.contains"),
    ("M3", "the fake's held: a part read as none (TL-T1 undone in held)", FAKE,
     "            } else if members().any(|(_, s)| s.holds.contains(tag)) {\n                Held::Part",
     "            } else if members().any(|(_, s)| s.holds.contains(tag)) {\n                Held::No"),
    ("M4", "taken: Part is held (Held recorded on a part)", PLAN,
     "            if pool.hold != Held::All {\n                return Action::Hold;\n            }\n            if pool.load_guid == publish.count.load_guid {",
     "            if pool.hold == Held::No {\n                return Action::Hold;\n            }\n            if pool.load_guid == publish.count.load_guid {"),
    ("M5", "pending with a part: release (the hold let go while it pends)", PLAN,
     "        Reply::Pending => {\n            if pool.hold != Held::All || broken.is_some() || moved {",
     "        Reply::Pending => {\n            if pool.hold == Held::Part {\n                return Action::Release;\n            }\n            if pool.hold != Held::All || broken.is_some() || moved {"),
    ("M6", "give_up on a part: abandon with no release where pool@t is gone", PLAN,
     "fn give_up(pool: &PoolSeen, why: Why) -> Action {\n    if pool.hold != Held::No {",
     "fn give_up(pool: &PoolSeen, why: Why) -> Action {\n    if pool.hold == Held::All || (pool.hold == Held::Part && pool.snapshot.is_some()) {"),
    ("M7", "the fake's hold: refuses a whole hold too", FAKE,
     "            return if lacking.is_empty() {\n                Ok(())",
     "            return if lacking.is_empty() {\n                Err(ZfsError::Other(\"held\".into()))"),
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
