#!/usr/bin/env python3
"""Next look at #168 (M2.6c-5f), head b30e452: my four surviving plants of
the first look (A2, A9, S2, N3m), re-run against the tests the commit adds,
and fresh plants on those tests and on the commit's one code change (the
"more lines than holds" message). Each plant is applied alone as exact
text (each site found exactly once) on a scratch worktree of the head, then
the agent's tests run (both agent crates, every test target, with the
program's test hooks) with a 600 s limit, then the file is restored in a
finally. One JSON line per plant: KILLED (the tests that failed, named,
and the first lines of each failure, to see that it fails for the right
reason; or the 600 s limit), SURVIVED, BUILD (did not compile: not a
kill), or an error. BASE is the head unchanged.

usage: python3 -I plants168n.py WORKTREE [ID,ID...]
"""
import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(sys.argv[1])
CLI = "crates/skyzfs-agent/src/zfs/cli.rs"
FAKE = "crates/skyzfs-agent/src/engine/fake.rs"

P = []


def plant(pid, what, *files):
    P.append((pid, what, list(files)))


AGENTS = "        .is_some_and(|(_, t)| t.starts_with(crate::names::PREFIX))\n"
FILTER = "            .filter(|(name, refs)| *refs > 0 && agents_name(name))\n"
FAKE_FILTER = "                .filter(|t| t.starts_with(crate::names::PREFIX))\n"
NO_NL = ("            why: format!(\"its output does not end with a newline; {ODD_HOLDS}\"),\n"
         "            again: false,\n")
N3_MSG = ("                        \"{}: a snapshot read with a hold is gone, or a line forged by a \\\n"
          "                         tag with a newline names a snapshot that never was: {}; {ODD_HOLDS}\",\n")
N4_MSG = ("                     holds were placed or released between the two reads, again and \\\n"
          "                     again, or a line is forged by a tag with a newline, on the hold just \\\n"
          "                     before one of them (of {}); read `zfs holds` of those snapshots and, where a tag \\\n")

plant("BASE", "the head unchanged")

# --- my four survivors of the first look (TG2) --------------------------------
plant("A2", "the prefix taken in any case (SKZ-1 read)", (CLI, [
    (AGENTS, "        .is_some_and(|(_, t)| t.to_ascii_lowercase().starts_with(crate::names::PREFIX))\n"),
]))
plant("A9", "the fake's tagged: the filter dropped", (FAKE, [
    (FAKE_FILTER, ""),
]))
plant("S2", "an output with no newline at its end read again", (CLI, [
    (NO_NL, NO_NL.replace("again: false", "again: true")),
]))
plant("N3m", "the count's missing-snapshot message without the forged cause (N3 undone)", (CLI, [
    (N3_MSG, "                        \"{}: a snapshot read with a hold is gone: {}\",\n"),
]))

# --- fresh ----------------------------------------------------------------------
plant("N4pm", "the message's 'placed or released' back to 'added' (N4' undone)", (CLI, [
    (N4_MSG, N4_MSG.replace("placed or released", "added")),
]))
plant("A2u", "upper case alone taken too (SKZ- or skz-)", (CLI, [
    (AGENTS, "        .is_some_and(|(_, t)| t.starts_with(crate::names::PREFIX) || t.starts_with(\"SKZ-\"))\n"),
]))
plant("A9c", "the fake's filter in any case", (FAKE, [
    (FAKE_FILTER, "                .filter(|t| t.to_ascii_lowercase().starts_with(crate::names::PREFIX))\n"),
]))
plant("A9d", "the fake's filter without the dash (skz)", (FAKE, [
    (FAKE_FILTER, "                .filter(|t| t.starts_with(\"skz\"))\n"),
]))
plant("A9e", "the fake's filter: the prefix anywhere in t", (FAKE, [
    (FAKE_FILTER, "                .filter(|t| t.contains(crate::names::PREFIX))\n"),
]))
plant("A9f", "the fake's filter inverted for a name of another dataset (pool's own snapshots only filtered)", (FAKE, [
    ("                .filter(|(_, s)| s.holds.contains(tag))\n"
     "                .filter_map(|(name, _)| name.split_once('@').map(|(_, t)| t.to_string()))\n"
     + FAKE_FILTER,
     "                .filter(|(_, s)| s.holds.contains(tag))\n"
     "                .filter(|(name, _)| name.contains('/') || name.split_once('@').is_some_and(|(_, t)| t.starts_with(crate::names::PREFIX)))\n"
     "                .filter_map(|(name, _)| name.split_once('@').map(|(_, t)| t.to_string()))\n"),
]))
plant("A7", "the CLI sweep's filter dropped (the base): which tests kill it", (CLI, [
    (FILTER, "            .filter(|(name, refs)| *refs > 0 || (false && agents_name(name)))\n"),
]))
plant("N3a", "N3's message without its advice (ODD_HOLDS dropped)", (CLI, [
    (N3_MSG, N3_MSG.replace("; {ODD_HOLDS}\"", "\"")),
]))
plant("N3v", "the count's missing snapshot an error at once (Other), not read again", (CLI, [
    ("                    ZfsError::NoSnapshot => ZfsError::Vanished(format!(\n"
     "                        \"{}: a snapshot read with a hold is gone, or a line forged by a \\\n",
     "                    ZfsError::NoSnapshot => ZfsError::Other(format!(\n"
     "                        \"{}: a snapshot read with a hold is gone, or a line forged by a \\\n"),
]))
plant("S2b", "the no-newline output taken as no holds (Ok empty) rather than an error", (CLI, [
    ("    let Some(body) = out.strip_suffix(b\"\\n\") else {\n        return Err(Odd {\n",
     "    let Some(body) = out.strip_suffix(b\"\\n\") else {\n        if true { return Ok(Vec::new()); }\n        return Err(Odd {\n"),
]))

AGENT = ["cargo", "test", "-p", "skyzfs-agent", "-p", "skyzfs-agent-cli",
         "--features", "skyzfs-agent-cli/test-hooks", "--no-fail-fast",
         "--", "--test-threads=4"]


def why(text, name):
    """The first lines of a failed test's panic."""
    m = re.search(r"---- " + re.escape(name) + r" stdout ----\n(.*?)(?:\n\n|\nnote:|\n---- )", text, re.S)
    if not m:
        return ""
    body = [l for l in m.group(1).splitlines() if l.strip()]
    return " | ".join(body[:4])[:900]


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
        text = out.stdout + out.stderr
        lines = text.splitlines()
        failed = [l.split()[1] for l in lines if l.startswith("test ") and l.endswith("FAILED")]
        if "could not compile" in out.stderr:
            errs = [l for l in out.stderr.splitlines() if l.startswith("error")][:3]
            return {"id": pid, "what": what, "result": "BUILD", "by": errs}
        if out.returncode != 0:
            return {"id": pid, "what": what, "result": "KILLED", "failed": len(failed), "by": failed,
                    "why": {f: why(text, f) for f in failed[:6]}}
        passed = [l for l in lines if l.startswith("test result")]
        total = sum(int(l.split()[3]) for l in passed)
        return {"id": pid, "what": what, "result": "SURVIVED", "tests passed": total, "targets": len(passed)}
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
