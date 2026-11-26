#!/usr/bin/env python3
"""Review of #162 (M2.6c-5d), head 6e420dc: plants on a scratch copy of
the head, each applied alone as text (one or more exact replacements,
each site found exactly once), then the agent's tests, then restored in
a finally. One JSON line per plant: KILLED (the tests that failed,
named; or the build failed; or the 600 s limit), SURVIVED, or an error.

S*, T*, L*: my versions of the author's 21 plants (the description's
table), as I read each row.
R*: mine.

usage: python3 -I plants162.py WORKTREE [ID,ID...]
"""
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(sys.argv[1])
CLI = "crates/skyzfs-agent/src/zfs/cli.rs"

P = []


def plant(pid, what, *files):
    P.append((pid, what, list(files)))


REFS = "                    if refs > 0 {\n"
TAG_EQ = "                if held_by == tag {\n"
INSERT = "                        tagged.insert(t.to_string());\n"
LIST_LIMIT = "        let limit = self.walk_limit(self.limits.list_per_snapshot);\n"
HOLDS_CALL = "            let holds = match self.zfs(&args, self.read_limit(), true, pool) {\n"
CHUNKS = "        for names in held.chunks(HOLDS_AT_ONCE) {\n"
GONE_ARM = ("                Err(ZfsError::NoSnapshot) => {\n"
            "                    return Err(ZfsError::Vanished(format!(\n"
            "                        \"{}: a snapshot listed with a hold is gone\",\n"
            "                        command(\"zfs\", &args)\n"
            "                    )))\n"
            "                }\n")
OTHER_ARM = ("                Err(e) => return Err(e),\n"
             "            };\n"
             "            for line in holds.lines() {\n")
LISTED = "        let listed = self.zfs_bytes(&list, limit, pool, true)?;\n"
AT_GUARD = "                Some((name, refs)) if name.contains('@') => {\n"
AGAIN = "        self.again(|zfs| zfs.tagged_once(pool, tag))\n"
STATUS = "        let out = self.zpool(&args, self.read_limit(), pool)?;\n"
FALLBACK = "                    self.zpool(&[\"status\", \"-P\", pool], self.read_limit(), pool)\n"
SYNC = "        self.zpool(&[\"sync\", pool], self.change_limit(), pool)?;\n"
LIST_OUTER = ("                grown(l.read, l.full_per_dataset, l.full_most, datasets),\n"
              "                l.list_per_snapshot,\n"
              "                l.full_most,\n"
              "                snapshots,\n")
D_LIST = "            list_per_snapshot: Duration::from_millis(10),\n"
D_HOLDS = "            holds_per_snapshot: Duration::from_millis(10),\n"
SKIP = ("                let (Some(snapshot), Some(held_by)) = (fields.next(), fields.next()) else {\n"
        "                    continue;\n"
        "                };\n")
SPLIT_AT = "                    if let Some((_, t)) = snapshot.split_once('@') {\n"
SNAP_COUNT = "            .run(\"zfs\", &args, &[], self.read_limit(), false, pool)\n            .ok()\n"

# --- the author's plants, my versions ---------------------------------------
plant("S1", "a snapshot with one hold not read (refs > 1)", (CLI, [(REFS, REFS.replace("> 0", "> 1"))]))
plant("S2", "the holds of every snapshot read", (CLI, [(REFS, "                    if true || refs > 0 {\n")]))
plant("S3", "a tag that only begins with the agent's kept", (CLI, [(TAG_EQ, "                if held_by.starts_with(tag) {\n")]))
plant("S4", "the dataset's part kept in the name", (CLI, [(INSERT, "                        let _ = t;\n                        tagged.insert(snapshot.to_string());\n")]))
plant("S5", "the sweep's list on a read's limit", (CLI, [(LIST_LIMIT, "        let limit = self.read_limit();\n")]))
plant("S6", "each zfs holds on the list's limit", (CLI, [(HOLDS_CALL, HOLDS_CALL.replace("self.read_limit()", "limit"))]))
plant("S7", "1001 names a command", (CLI, [(CHUNKS, CHUNKS.replace("HOLDS_AT_ONCE", "HOLDS_AT_ONCE + 1"))]))
plant("S8", "a snapshot gone before its holds taken as no hold", (CLI, [(GONE_ARM, "                Err(ZfsError::NoSnapshot) => continue,\n")]))
plant("S9", "a snapshot gone before its holds an error at once", (CLI, [(GONE_ARM, "                Err(ZfsError::NoSnapshot) => return Err(ZfsError::Other(command(\"zfs\", &args))),\n")]))
plant("S10", "the list's `dataset does not exist` of the pool not confirmed", (CLI, [(LISTED, LISTED.replace("pool, true", "pool, false"))]))
plant("S11", "a line with no @ taken for a snapshot", (CLI, [(AT_GUARD, "                Some((name, refs)) => {\n")]))
plant("S12", "no reading again", (CLI, [(AGAIN, "        self.tagged_once(pool, tag)\n")]))
plant("T1", "zpool status (both) confirmed on a pool named `status`", (CLI, [(STATUS, STATUS.replace("), pool)", "), \"status\")"))]))
plant("T2", "the props fallback's zpool status confirmed on `status`", (CLI, [(FALLBACK, FALLBACK.replace("), pool)", "), \"status\")"))]))
plant("T3", "the assist's zpool sync confirmed on `sync`", (CLI, [(SYNC, SYNC.replace("), pool)", "), \"sync\")"))]))
plant("L1", "the manifest's list's snapshots at a dataset's 200 ms", (CLI, [(LIST_OUTER, LIST_OUTER.replace("l.list_per_snapshot", "l.full_per_dataset"))]))
plant("L2", "the sweep's list on holds -r's allowance", (CLI, [(LIST_LIMIT, LIST_LIMIT.replace("list_per_snapshot", "holds_per_snapshot"))]))
plant("L3", "list_per_snapshot 5 ms", (CLI, [(D_LIST, D_LIST.replace("10", "5"))]))
plant("L4", "holds_per_snapshot 4 ms", (CLI, [(D_HOLDS, D_HOLDS.replace("10", "4"))]))
plant("L5a", "the manifest's list's datasets part capped at most", (CLI, [(LIST_OUTER, LIST_OUTER.replace("l.full_per_dataset, l.full_most", "l.full_per_dataset, l.most"))]))
plant("L5b", "the manifest's list capped at most", (CLI, [(LIST_OUTER, LIST_OUTER.replace("                l.full_most,\n", "                l.most,\n"))]))
plant("L6", "the manifest's list's snapshots not added", (CLI, [(LIST_OUTER, LIST_OUTER.replace("l.list_per_snapshot", "Duration::ZERO"))]))

# --- mine -------------------------------------------------------------------
plant("R1", "the sweep's holds -H confirmed on its first snapshot's name, not the pool", (CLI, [(HOLDS_CALL, HOLDS_CALL.replace("true, pool)", "true, names[0])"))]))
plant("R2", "holds -H's `no such pool or dataset` (Vanished) taken as no hold", (CLI, [(OTHER_ARM, "                Err(ZfsError::Vanished(_)) => continue,\n" + OTHER_ARM)]))
plant("R3", "holds -H's `no such pool or dataset` (Vanished) an error at once", (CLI, [(OTHER_ARM, "                Err(ZfsError::Vanished(w)) => return Err(ZfsError::Other(w)),\n" + OTHER_ARM)]))
plant("R4", "a tag that the agent's only begins with kept", (CLI, [(TAG_EQ, "                if tag.starts_with(held_by) {\n")]))
plant("R5", "a holds line with no tag an error, not skipped", (CLI, [(SKIP, SKIP.replace("                    continue;\n", "                    return Err(ZfsError::Other(line.to_string()));\n"))]))
plant("R6", "the sweep's list on the manifest's list's limit", (CLI, [(LIST_LIMIT, "        let limit = self.list_limit();\n")]))
plant("R7", "zpool status -g alone confirmed on `status`", (CLI, [(STATUS, "        let out = self.zpool(&args, self.read_limit(), if guids { \"status\" } else { pool })?;\n")]))
plant("R8", "the snapshot count on full_most (the R-COUNT option): does a test see it", (CLI, [(SNAP_COUNT, SNAP_COUNT.replace("self.read_limit()", "self.limits.full_most"))]))
plant("R9", "the t taken after the last @, not the first (equivalent, names hold one @)", (CLI, [(SPLIT_AT, SPLIT_AT.replace("split_once", "rsplit_once"))]))

AGENT = ["cargo", "test", "-p", "skyzfs-agent", "-p", "skyzfs-agent-cli",
         "--features", "skyzfs-agent-cli/test-hooks", "--", "--test-threads=4"]


def run(p):
    pid, what, files = p
    texts = {}
    try:
        for rel, subs in files:
            path = ROOT / rel
            if rel not in texts:
                texts[rel] = path.read_text()
            new = path.read_text()
            for old, rep in subs:
                if new.count(old) != 1:
                    return {"id": pid, "what": what, "result": f"ERROR: site in {rel} found {new.count(old)} times"}
                new = new.replace(old, rep)
            path.write_text(new)
        try:
            out = subprocess.run(AGENT, cwd=ROOT, capture_output=True, text=True, timeout=600)
        except subprocess.TimeoutExpired:
            return {"id": pid, "what": what, "result": "KILLED (timeout 600 s)"}
        lines = (out.stdout + out.stderr).splitlines()
        failed = [l.split()[1] for l in lines if l.startswith("test ") and l.endswith("FAILED")]
        if "could not compile" in out.stderr:
            errs = [l for l in out.stderr.splitlines() if l.startswith("error")][:3]
            return {"id": pid, "what": what, "result": "KILLED (build)", "by": errs}
        if out.returncode != 0:
            return {"id": pid, "what": what, "result": "KILLED", "failed": len(failed), "by": failed[:12]}
        passed = [l for l in lines if l.startswith("test result")]
        return {"id": pid, "what": what, "result": "SURVIVED", "tests": passed}
    finally:
        for rel, text in texts.items():
            (ROOT / rel).write_text(text)


def main():
    only = set(sys.argv[2].split(",")) if len(sys.argv) > 2 else None
    for p in P:
        if only and p[0] not in only:
            continue
        print(json.dumps(run(p)), flush=True)
    st = subprocess.run(["git", "status", "--short"], cwd=ROOT, capture_output=True, text=True)
    print(json.dumps({"worktree after": st.stdout.strip() or "clean"}))


main()
