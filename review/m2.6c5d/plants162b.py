#!/usr/bin/env python3
"""Review of #162 (M2.6c-5d), second look, head b83ac36 (the lead's
ruling on R-COUNT): plants on a scratch copy of the head, each applied
alone as text (one or more exact replacements, each site found exactly
once), then the agent's tests, then restored in a finally. One JSON
line per plant: KILLED (the tests that failed, named; or the build
failed; or the 600 s limit), SURVIVED, or an error.

C*: my versions of the author's seven plants (the description's table).
Q*: mine.

usage: python3 -I plants162b.py WORKTREE [ID,ID...]
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


SNAPC = '        let snapshots = match self.run("zfs", &args, &[], self.limits.full_most, false, pool) {\n'
HOLDS_R_ERR = "            Err(e) => return Err(self.walked(e)),\n"
SWEEP_LIST = ("            .zfs_bytes(&list, limit, pool, true)\n"
              "            .map_err(|e| self.walked(e))?;\n")
MANIFEST_LIST = ("            .zfs_bytes(&list, self.list_limit(), pool, true)\n"
                 "            .map_err(|e| self.walked(e))?;\n")
CLEAR = "        self.uncounted = None;\n"
KEEP = "                self.uncounted = Some(e.to_string());\n"
SNAPC_OK = "            Ok(out) => Some(out.lines().filter(|line| !line.is_empty()).count() as u64),\n"
WALKED_ARM = "            (ZfsError::Other(why), Some(count)) => ZfsError::Other(format!(\n"
WALKED_FMT = '                "{why} (on a limit that did not grow with the snapshots, since \\\n'

# --- the author's plants, my versions ---------------------------------------
plant("C1", "the snapshot count on a read's limit (as before)", (CLI, [(SNAPC, SNAPC.replace("self.limits.full_most", "self.read_limit()"))]))
plant("C2", "the snapshot count on the count's own 60 s", (CLI, [(SNAPC, SNAPC.replace("self.limits.full_most", "self.limits.count"))]))
plant("C3", "holds -r's error not naming the failed count", (CLI, [(HOLDS_R_ERR, "            Err(e) => return Err(e),\n")]))
plant("C4", "the sweep's list's error not naming it", (CLI, [(SWEEP_LIST, "            .zfs_bytes(&list, limit, pool, true)?;\n")]))
plant("C5", "the manifest's list's error not naming it", (CLI, [(MANIFEST_LIST, "            .zfs_bytes(&list, self.list_limit(), pool, true)?;\n")]))
plant("C6", "the failure not forgotten by a later count", (CLI, [(CLEAR, "")]))
plant("C7", "the failure kept without its reason", (CLI, [(KEEP, KEEP.replace("e.to_string()", "String::new()"))]))

# --- mine -------------------------------------------------------------------
plant("Q1", "the snapshot count on the other reads' cap (20 min)", (CLI, [(SNAPC, SNAPC.replace("self.limits.full_most", "self.limits.most"))]))
plant("Q2", "the snapshot count on the gets' limit (datasets counted, 80.2 s)", (CLI, [(SNAPC, SNAPC.replace("self.limits.full_most", "self.full_read_limit()"))]))
plant("Q3", "a walk's missing pool or vanished snapshot turned into Other by a failed count", (CLI, [(WALKED_ARM, "            (why, Some(count)) => ZfsError::Other(format!(\n")]))
plant("Q4", "the failure forgotten only by a count that counts the snapshots (kept when the datasets' count fails)",
      (CLI, [(CLEAR, ""), (SNAPC_OK, "            Ok(out) => {\n                self.uncounted = None;\n                Some(out.lines().filter(|line| !line.is_empty()).count() as u64)\n            }\n")]))
plant("Q5", "the walk's own error dropped, the count's failure alone said", (CLI, [(WALKED_FMT, WALKED_FMT.replace("{why} (on", "(on"))]))
# C1 again, with the new test's two checks of the count's limit taken out
# of a scratch copy: does the test still fail by what the host does (the
# count cut off, the walks not grown), and not only by the limit named?
TESTS = "crates/skyzfs-agent/src/zfs/cli/tests.rs"
LIMIT_1 = "    assert_eq!(zfs.host().fake.ran[1].limit, base.full_most);\n    let counted = walks(&mut zfs);\n"
LIMIT_2 = "    assert_eq!(zfs.host().fake.ran[1].limit, base.full_most);\n    let uncounted = walks(&mut zfs);\n"
plant("Q6", "C1, with the test's checks of the count's limit taken out (scratch only)",
      (CLI, [(SNAPC, SNAPC.replace("self.limits.full_most", "self.read_limit()"))]),
      (TESTS, [(LIMIT_1, "    let counted = walks(&mut zfs);\n"), (LIMIT_2, "    let uncounted = walks(&mut zfs);\n")]))

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
