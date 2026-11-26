#!/usr/bin/env python3
"""Look at #165 (M2.6c-5e), head 6752363: plants on a scratch worktree of
the head, each applied alone as text (exact replacements, each site found
exactly once), then the agent's tests, then restored in a finally. One
JSON line per plant: KILLED (the tests that failed, named; or the 600 s
limit), SURVIVED, BUILD (did not compile: not a kill), or an error.

G*: my versions of the author's plants G1 to G15.
M*: mine.

usage: python3 -I plants165.py WORKTREE [ID,ID...]
"""
import json
import os
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(sys.argv[1])
CLI = "crates/skyzfs-agent/src/zfs/cli.rs"

P = []


def plant(pid, what, *files):
    P.append((pid, what, list(files)))


SWEEP_CMP = "outnumbered(&holds, |name| refs.get(name).copied().unwrap_or(0)).map(|()| holds)"
R_CMP = "if let Err(Odd(why)) = outnumbered(&holds, |name| refs.get(name).copied().unwrap_or(0)) {"
RETRY = "Ok(Err(_)) | Err(ZfsError::Vanished(_)) if tries < VANISHED_READS => tries += 1,"
LIST_FAIL = ("            if listed.code != Some(0) {\n"
             "                return Err(match self.failed(\"zfs\", &list, &listed, true, pool) {\n")
UNDER_T = "        let t = snapshot.split_once('@').map_or(\"\", |(_, t)| t);\n"
SWEEP_EXIT = ("            // Each line a snapshot this command asked for, its tag and\n")

# --- the author's, as I read them -----------------------------------------
plant("G1", "a line that cannot be read skipped", (CLI, [
    ("        let Some(hold) = hold else {\n",
     "        let Some(hold) = hold else {\n            continue;\n"),
]))
plant("G2", "more than three fields taken", (CLI, [
    ("            [snapshot, tag, _] => std::str::from_utf8(snapshot)",
     "            [snapshot, tag, _, ..] => std::str::from_utf8(snapshot)"),
]))
plant("G3", "any snapshot taken, asked or not", (CLI, [
    ("                .filter(|snapshot| asked(snapshot))\n",
     "                .filter(|snapshot| true || asked(snapshot))\n"),
]))
plant("G4", "the sweep's count not compared", (CLI, [
    (SWEEP_CMP, "Ok::<_, Odd>(holds)"),
]))
plant("G5", "the -r count not compared", (CLI, [
    (R_CMP, "if let Err(Odd(why)) = outnumbered(&holds, |name| refs.get(name).copied().unwrap_or(0)).or(Ok::<(), Odd>(())) {"),
]))
plant("G6", "one line more than the holds allowed", (CLI, [
    ("        if lines > refs {\n", "        if lines > refs + 1 {\n"),
]))
plant("G7", "holds that do not add up not read again", (CLI, [
    (RETRY, "Err(ZfsError::Vanished(_)) if tries < VANISHED_READS => tries += 1,"),
]))
plant("G8", "a pool that the pool's name begins taken", (CLI, [
    ("                            .is_some_and(|rest| rest.starts_with('/')))\n",
     "                            .is_some_and(|_rest| true))\n"),
]))
plant("G9", "another @t taken", (CLI, [
    ("                at == t\n", "                (at == t || true)\n"),
]))
plant("G10", "an output with no newline at its end taken", (CLI, [
    ("    let Some(body) = out.strip_suffix(b\"\\n\") else {\n",
     "    let Some(body) = out.strip_suffix(b\"\\n\").or(Some(out)) else {\n"),
]))
plant("G11", "the -r holds read as UTF-8", (CLI, [
    (UNDER_T,
     UNDER_T + "        if std::str::from_utf8(&output.stdout).is_err() {\n"
     "            return Err(ZfsError::Other(\"its output is not UTF-8\".to_string()));\n"
     "        }\n"),
]))
plant("G12", "a snapshot gone before its count taken as none held", (CLI, [
    (LIST_FAIL,
     "            if listed.code != Some(0) {\n"
     "                if true { return Ok(Ok(None)); }\n"
     "                return Err(match self.failed(\"zfs\", &list, &listed, true, pool) {\n"),
]))
plant("G13", "a snapshot left out of the count taken as held", (CLI, [
    (R_CMP, "if let Err(Odd(why)) = outnumbered(&holds, |name| refs.get(name).copied().unwrap_or(u64::MAX)) {"),
]))
plant("G14", "the lines before and after swapped in the error", (CLI, [
    ("                .filter(|pair| pair[1].0 == snapshot)\n                .map(|pair| pair[0].0)\n",
     "                .filter(|pair| pair[0].0 == snapshot)\n                .map(|pair| pair[1].0)\n"),
]))
plant("G15", "the sweep's tags read as text", (CLI, [
    (SWEEP_EXIT,
     "            if std::str::from_utf8(&output.stdout).is_err() {\n"
     "                return Err(ZfsError::Other(\"its output is not UTF-8\".to_string()));\n"
     "            }\n" + SWEEP_EXIT),
]))

# --- mine ------------------------------------------------------------------
plant("M1", "an empty holds output an error (no early return)", (CLI, [
    ("    if out.is_empty() {\n        return Ok(Vec::new());\n    }\n", ""),
]))
plant("M2", "Odd read twice only, Vanished three times", (CLI, [
    (RETRY,
     "Ok(Err(_)) if tries < 2 => tries += 1,\n"
     "                Err(ZfsError::Vanished(_)) if tries < VANISHED_READS => tries += 1,"),
]))
plant("M3", "the -r count on a walk's limit", (CLI, [
    ("            let listed = self.exec(\"zfs\", &list, &[], self.read_limit())?;\n",
     "            let listed = self.exec(\"zfs\", &list, &[], self.walk_limit(self.limits.holds_per_snapshot))?;\n"),
]))
plant("M4", "the -r count not batched (all names in one command)", (CLI, [
    ("        for batch in holds_batches(&names) {\n            let mut list = vec![\"list\"",
     "        for batch in [&names[..]] {\n            let mut list = vec![\"list\""),
]))
plant("M5", "the -r reads take a tag the agent's begins (starts_with)", (CLI, [
    ("                .filter(|(_, held_by)| *held_by == tag.as_bytes())\n",
     "                .filter(|(_, held_by)| held_by.starts_with(tag.as_bytes()))\n"),
]))
plant("M6", "hold's check: Vanished not read again", (CLI, [
    ("        let tagged = self\n            .again_checked(|zfs| zfs.tagged_snapshots(&snapshot, tag))?\n",
     "        let tagged = self\n            .again_checked(|zfs| zfs.tagged_snapshots(&snapshot, tag).map_err(|e| match e { ZfsError::Vanished(w) => ZfsError::Other(w), e => e }))?\n"),
]))
plant("M7", "release's check: Vanished not read again", (CLI, [
    ("        let Some(tagged) = self.again_checked(|zfs| zfs.tagged_snapshots(&snapshot, tag))? else {\n            return Ok(());\n",
     "        let Some(tagged) = self.again_checked(|zfs| zfs.tagged_snapshots(&snapshot, tag).map_err(|e| match e { ZfsError::Vanished(w) => ZfsError::Other(w), e => e }))? else {\n            return Ok(());\n"),
]))
plant("M8", "the -r count's exit code not checked", (CLI, [
    (LIST_FAIL,
     "            if false && listed.code != Some(0) {\n"
     "                return Err(match self.failed(\"zfs\", &list, &listed, true, pool) {\n"),
]))
plant("M9", "the sweep's asked = any held snapshot of the pool, not of this command", (CLI, [
    ("hold_lines(&output.stdout, |name| asked.contains(name))",
     "hold_lines(&output.stdout, |name| refs.get(name).is_some_and(|&r| r > 0) || asked.contains(name))"),
]))
plant("M10", "fewer lines than holds an error too (lines != refs)", (CLI, [
    ("        if lines > refs {\n", "        if lines != refs {\n"),
]))
plant("M11", "the -r count lists only the snapshots with the agent's tag", (CLI, [
    ("        let names: BTreeSet<&str> = holds.iter().map(|(name, _)| *name).collect();\n",
     "        let names: BTreeSet<&str> = holds.iter().filter(|(_, b)| *b == tag.as_bytes()).map(|(name, _)| *name).collect();\n"),
]))
plant("M12", "a snapshot with userrefs 0 counted as one hold", (CLI, [
    ("        let refs = refs(snapshot);\n", "        let refs = refs(snapshot).max(1);\n"),
]))
plant("M13", "the -r count's Vanished taken as an error at once (Other)", (CLI, [
    ("                    ZfsError::NoSnapshot => ZfsError::Vanished(format!(\n"
     "                        \"{}: a snapshot read with a hold is gone: {}\",",
     "                    ZfsError::NoSnapshot => ZfsError::Other(format!(\n"
     "                        \"{}: a snapshot read with a hold is gone: {}\","),
]))
plant("M14", "the -r count's NoPool/other failure taken as Vanished", (CLI, [
    ("                    e => e,\n                });\n            }\n            for (name, count) in userrefs(",
     "                    e => ZfsError::Vanished(e.to_string()),\n                });\n            }\n            for (name, count) in userrefs("),
]))
plant("M15", "the sweep's Odd after the last read returned as Ok(empty) (skip)", (CLI, [
    ("    fn tagged(&mut self, pool: &str, tag: &str) -> Result<BTreeSet<String>, ZfsError> {\n"
     "        self.again_checked(|zfs| zfs.tagged_once(pool, tag))\n",
     "    fn tagged(&mut self, pool: &str, tag: &str) -> Result<BTreeSet<String>, ZfsError> {\n"
     "        self.again_checked(|zfs| zfs.tagged_once(pool, tag).map(|r| Ok(r.unwrap_or_default())))\n"),
]))
plant("M16", "the -r tag compared after a lossy UTF-8 read", (CLI, [
    ("                .filter(|(_, held_by)| *held_by == tag.as_bytes())\n",
     "                .filter(|(_, held_by)| String::from_utf8_lossy(held_by) == tag)\n"),
]))

plant("M4b", "the -r count not batched (all names in one command, none where none)", (CLI, [
    ("        for batch in holds_batches(&names) {\n            let mut list = vec![\"list\"",
     "        for batch in [&names[..]].into_iter().filter(|b| !b.is_empty()) {\n            let mut list = vec![\"list\""),
]))
plant("M17", "the sweep takes a tag the agent's begins (starts_with)", (CLI, [
    ("                if held_by == tag.as_bytes() {\n",
     "                if held_by.starts_with(tag.as_bytes()) {\n"),
]))
plant("M18", "the -r count's limit a walk's, at a counted scale (as M3; see TG)", (CLI, [
    ("            let listed = self.exec(\"zfs\", &list, &[], self.read_limit())?;\n",
     "            let listed = self.exec(\"zfs\", &list, &[], self.list_limit())?;\n"),
]))

AGENT = ["cargo", "test", "-p", "skyzfs-agent", "-p", "skyzfs-agent-cli",
         "--features", "skyzfs-agent-cli/test-hooks", "--no-fail-fast",
         "--", "--test-threads=4"]


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
            return {"id": pid, "what": what, "result": "BUILD", "by": errs}
        if out.returncode != 0:
            return {"id": pid, "what": what, "result": "KILLED", "failed": len(failed), "by": failed}
        passed = [l for l in lines if l.startswith("test result")]
        return {"id": pid, "what": what, "result": "SURVIVED", "tests": passed}
    finally:
        for rel, text in texts.items():
            (ROOT / rel).write_text(text)


def main():
    only = sys.argv[2].split(",") if len(sys.argv) > 2 else None
    order = only if only else [p[0] for p in P]
    by = {p[0]: p for p in P}
    for pid in order:
        print(json.dumps(run(by[pid])), flush=True)
    st = subprocess.run(["git", "status", "--short"], cwd=ROOT, capture_output=True, text=True)
    print(json.dumps({"worktree after": st.stdout.strip() or "clean"}), flush=True)


main()
