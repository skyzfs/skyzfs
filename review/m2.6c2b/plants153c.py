#!/usr/bin/env python3
"""Next look at #153 (M2.6c-2b), head 0a19e1c: plants on a scratch copy of
the head, each applied alone, then `cargo test -p skyzfs-agent --lib`
(600 s limit), then restored in a finally. One JSON line per plant:
KILLED (a test failed, named; or the build failed), SURVIVED, or an error.

C*: the limits of each call (which kind of limit a call site passes).
F*: the parsers the new recordings tests read the fixtures through.
X*: the fixtures themselves (full-ubuntu-<image>.json), a record's
output changed, the file rewritten in its own layout.

usage: python3 -I plants153c.py WORKTREE [ID,ID...]
"""
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(sys.argv[1])
CLI = "crates/skyzfs-agent/src/zfs/cli.rs"
REC = "crates/skyzfs-agent/src/zfs/cli/recorded"

TEXT = [
    # Which limit each call passes.
    ("C1", "the full read given a read's limit", CLI,
     "            self.full_read_limit(),\n        )?;\n        parse_datasets",
     "            self.read_limit(),\n        )?;\n        parse_datasets"),
    ("C2", "the members program (plain) given the full read's limit", CLI,
     "MEMBERS_PROGRAM, &[pool, t], true, self.read_limit())",
     "MEMBERS_PROGRAM, &[pool, t], true, self.full_read_limit())"),
    ("C3", "the tagged program given the full read's limit", CLI,
     "TAGGED_PROGRAM, &[pool, tag], true, self.read_limit())",
     "TAGGED_PROGRAM, &[pool, tag], true, self.full_read_limit())"),
    ("C4", "`zfs holds -r` given a change's limit", CLI,
     "&[\"holds\", \"-H\", \"-r\", snapshot], self.read_limit(), true)",
     "&[\"holds\", \"-H\", \"-r\", snapshot], self.change_limit(), true)"),
    ("C5", "the full read scaled on a change's base", CLI,
     "self.scaled(l.read, l.full_per_dataset, l.full_most)",
     "self.scaled(l.change, l.full_per_dataset, l.full_most)"),
    ("C6", "the full read capped as a read", CLI,
     "self.scaled(l.read, l.full_per_dataset, l.full_most)",
     "self.scaled(l.read, l.full_per_dataset, l.most)"),
    ("C7", "scaled() caps every call at the 20-minute cap", CLI,
     "        limit.min(most)\n",
     "        let _ = most;\n        limit.min(self.limits.most)\n"),
    ("C8", "the full read's cap at 36 minutes", CLI,
     "            full_most: Duration::from_secs(35 * 60),",
     "            full_most: Duration::from_secs(36 * 60),"),
    ("C9", "`zpool status` given the full read's limit", CLI,
     "        let out = self.zpool(&args, self.read_limit())?;",
     "        let out = self.zpool(&args, self.full_read_limit())?;"),
    ("C10", "the destroy given a read's limit", CLI,
     "            .exec(\"zfs\", &args, &[], self.change_limit())\n            .map_err(",
     "            .exec(\"zfs\", &args, &[], self.read_limit())\n            .map_err("),
    ("C11", "`zpool sync` (the assist) given a read's limit", CLI,
     "self.zpool(&[\"sync\", pool], self.change_limit())?;",
     "self.zpool(&[\"sync\", pool], self.read_limit())?;"),
    # The parsers.
    ("F1", "a GUID written negative read as its absolute value", CLI,
     ".or_else(|| text.parse::<i64>().ok().map(|n| n as u64))",
     ".or_else(|| text.parse::<i64>().ok().map(|n| n.unsigned_abs()))"),
    ("F2", "an @t's group read from the dataset's", CLI,
     "                createtxg: number(\"snapshot_createtxg\", false)?,",
     "                createtxg: number(\"createtxg\", false)?,"),
    ("F3", "a value in hex taken as its text", CLI,
     "                Some(bytes) if manifest::hex(&bytes) == hex && !manifest::plain(&bytes) => {\n                    Ok(bytes)\n                }",
     "                Some(_) => Ok(hex.as_bytes().to_vec()),"),
    ("F4", "local properties dropped", CLI,
     "                local.insert(property.clone(), value);",
     "                let _ = (property, value);"),
    ("F5", "the count leaves out the first line (the root)", CLI,
     ".map(|out| out.lines().filter(|line| !line.is_empty()).count() as u64);",
     ".map(|out| out.lines().filter(|line| !line.is_empty()).skip(1).count() as u64);"),
    ("F6", "a failed count read as none", CLI,
     "            Err(_) => Scale::Base,\n        };\n        counted\n",
     "            Err(_) => Scale::Base,\n        };\n        counted.or(Ok(0))\n"),
    ("F7", "a volume read as a file system", CLI,
     "            \"volume\" => Kind::Volume,\n            kind => return Err(format!(\"`{name}` is a {kind}\")),\n        };\n        let mut local",
     "            \"volume\" => Kind::Filesystem,\n            kind => return Err(format!(\"`{name}` is a {kind}\")),\n        };\n        let mut local"),
    ("F8", "an @t's GUID read from the dataset's", CLI,
     "                guid: number(\"snapshot_guid\", true)?,",
     "                guid: number(\"guid\", true)?,"),
    ("F9", "a program's `no such pool` not read as no pool", CLI,
     "    if says(&output.stderr, \"no such pool\") {",
     "    if says(&output.stderr, \"no such pool (plant)\") {"),
    ("F10", "the agent's own property not removed from a dataset's", CLI,
     "        local.remove(ASSIST_PROPERTY);\n",
     "\n"),
    ("F11", "a snapshot read in part taken as none", CLI,
     "            _ => return Err(format!(\"`{name}`: its snapshot is read in part\")),",
     "            _ => None,"),
]


def fx_guid(stdout):
    return stdout.replace("-5563034144446978196", "-5563034144446978197")


def fx_child_group(stdout):
    value = json.loads(stdout)
    child = value["return"]["datasets"]["skzq/child"]
    child["snapshot_createtxg"] = str(int(child["snapshot_createtxg"]) + 1)
    return json.dumps(value, separators=(",", ":"))


def fx_raw(stdout):
    return stdout.replace("61fffe62c3", "61fffe62c4")


def fx_count(stdout):
    return stdout.replace("skzq/vol\n", "")


def fx_root_at(stdout):
    value = json.loads(stdout)
    ds = value["return"]["datasets"]
    child = ds["skzqm/child"]
    ds["skzqm"]["snapshot_guid"] = "17"
    ds["skzqm"]["snapshot_createtxg"] = child["snapshot_createtxg"]
    return json.dumps(value, separators=(",", ":"))


FIXTURES = [
    ("X1", "26.04: a snapshot GUID of the rig's full read changed by one", "ubuntu-26.04",
     "rig-members-full", fx_guid),
    ("X2", "24.04: the child's @t of another group in the rig's full read", "ubuntu-24.04",
     "rig-members-full", fx_child_group),
    ("X3", "24.04: the raw value's last byte changed in the mirror's odd read", "ubuntu-24.04",
     "mirror-members-full-odd", fx_raw),
    ("X4", "26.04: the rig's count without `skzq/vol`", "ubuntu-26.04", "rig-count", fx_count),
    ("X5", "24.04: the mirror's root given an @p in the partial read", "ubuntu-24.04",
     "mirror-members-full-partial", fx_root_at),
]


def test():
    try:
        out = subprocess.run(
            ["cargo", "test", "-p", "skyzfs-agent", "--lib", "--", "--test-threads=4"],
            cwd=ROOT, capture_output=True, text=True, timeout=600)
    except subprocess.TimeoutExpired:
        return {"result": "KILLED (timeout 600 s)"}
    lines = (out.stdout + out.stderr).splitlines()
    failed = [l.split()[1] for l in lines if l.startswith("test ") and l.endswith("FAILED")]
    if "could not compile" in out.stderr:
        errs = [l for l in out.stderr.splitlines() if l.startswith("error")][:3]
        return {"result": "KILLED (build)", "by": errs}
    if out.returncode != 0:
        return {"result": "KILLED", "by": failed}
    return {"result": "SURVIVED", "tests": [l for l in lines if l.startswith("test result")]}


def run_text(plant):
    pid, what, rel, old, new = plant
    path = ROOT / rel
    text = path.read_text()
    if text.count(old) != 1:
        return {"id": pid, "what": what, "result": f"ERROR: site found {text.count(old)} times"}
    try:
        path.write_text(text.replace(old, new))
        return {"id": pid, "what": what, **test()}
    finally:
        path.write_text(text)


def run_fixture(plant):
    pid, what, image, record, change = plant
    path = ROOT / REC / f"full-{image}.json"
    text = path.read_text()
    value = json.loads(text)
    before = value["records"][record]["stdout"]
    after = change(before)
    if after == before:
        return {"id": pid, "what": what, "result": "ERROR: the change changed nothing"}
    value["records"][record]["stdout"] = after
    try:
        path.write_text(json.dumps(value, indent=1, ensure_ascii=False, sort_keys=True) + "\n")
        return {"id": pid, "what": what, **test()}
    finally:
        path.write_text(text)


def main():
    only = set(sys.argv[2].split(",")) if len(sys.argv) > 2 else None
    for plant in TEXT:
        if not only or plant[0] in only:
            print(json.dumps(run_text(plant)), flush=True)
    for plant in FIXTURES:
        if not only or plant[0] in only:
            print(json.dumps(run_fixture(plant)), flush=True)
    st = subprocess.run(["git", "status", "--short"], cwd=ROOT, capture_output=True, text=True)
    print(json.dumps({"worktree after": st.stdout.strip() or "clean"}))


main()
