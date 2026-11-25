"""Textual plants for the review of #142 (M2.6b1) at 05d79f4, one at a
time, each file restored from its saved text in `finally`. Run from a
worktree of the head:
WORKTREE=<path> OUT=<json> python3 plants142.py [name prefixes...]
Each suite skips the review's own scratch tests (`--skip review`)."""
import json, os, subprocess, sys, time

W = os.environ.get("WORKTREE", ".")
OUT = os.environ.get("OUT", "plants142.json")
ENV = dict(os.environ, CARGO_INCREMENTAL="0")
FEAT = "skyzfsd/test-hooks,skyzfs-local/test-hooks"
SUITES = {
    "a": ["cargo", "test", "-p", "skyzfs-agent", "--", "--skip", "review"],
    "l": ["cargo", "test", "-p", "skyzfs-local", "--lib", "--", "--skip", "review", "cuts"],
    "d": ["cargo", "test", "-p", "skyzfsd", "--features", FEAT, "--lib", "--", "--skip", "review", "control::tests::uploader"],
}
RC = "crates/skyzfs-agent/src/record.rs"
MF = "crates/skyzfs-agent/src/manifest.rs"
PL = "crates/skyzfs-agent/src/plan.rs"
CU = "crates/skyzfs-local/src/inplace/cuts.rs"
HD = "bins/skyzfsd/src/control/handler.rs"
P = [
 # The record's format: no golden bytes.
 ("R1 two fields of a publish swapped (a format change)", RC,
  "    /// Whether the agent believes its hold placed on `pool@<name>`.\n    pub held: bool,\n    /// The requests made of the daemon for it.\n    pub attempts: u32,\n",
  "    /// The requests made of the daemon for it.\n    pub attempts: u32,\n    /// Whether the agent believes its hold placed on `pool@<name>`.\n    pub held: bool,\n", "a"),
 ("R2 a reason inserted first in Why (a format change)", RC,
  "pub enum Why {\n",
  "pub enum Why {\n    /// Planted.\n    Planted,\n", "a"),
 ("R3 bytes after the body read", RC,
  "        if !left.is_empty() {\n",
  "        if false && !left.is_empty() {\n", "a"),
 # The write path's syncs: no fault test.
 ("R4 the file not synced before the rename", RC,
  "        file.sync_all()?;\n", "", "a"),
 ("R5 the directory not synced after the rename", RC,
  "    fs::rename(&tmp, dir.join(name))?;\n    File::open(dir)?.sync_all()\n",
  "    fs::rename(&tmp, dir.join(name))\n", "a"),
 ("R6 written in place, no temporary file", RC,
  "    let tmp = dir.join(format!(\".{name}.tmp\"));\n",
  "    let tmp = dir.join(name);\n", "a"),
 ("R7 the new directory's parent not synced", RC,
  "            Some(parent) if !parent.as_os_str().is_empty() => File::open(parent)?.sync_all(),\n",
  "            Some(parent) if !parent.as_os_str().is_empty() => Ok(()),\n", "a"),
 ("R8 a removed manifest's directory not synced", RC,
  "            Ok(()) => File::open(&dir)?.sync_all(),\n",
  "            Ok(()) => Ok(()),\n", "a"),
 # Confirmations in the manifest and the planner.
 ("M1 the canonical re-encode not compared", MF,
  "        if manifest.encode()? != bytes {\n",
  "        if manifest.encode()?.is_empty() {\n", "a"),
 ("M2 a GUID's leading zero taken", MF,
  "        Ok(n) if n.to_string() == text => Ok(n),\n",
  "        Ok(n) => Ok(n),\n", "a"),
 ("P1 ours without the group", PL,
  "                && row.group >= taken.group\n", "", "a"),
 ("P2 ours without the hash", PL,
  "                && row.manifest_hash == taken.manifest_hash\n", "", "a"),
 ("P3 pending kept after load_guid moved", PL,
  "            if !pool.held || broken.is_some() || moved {\n",
  "            if !pool.held || broken.is_some() {\n", "a"),
 ("P4 the hold released after held_until, not at it", PL,
  "        Some((_, until)) if seen.now_ms >= until && pool.held => Action::Release,\n",
  "        Some((_, until)) if seen.now_ms > until && pool.held => Action::Release,\n", "a"),
 ("P5 done at any commit", PL,
  "            Some(SnapSeen::Ours(at)) if at == *commit => Action::Done(at),\n",
  "            Some(SnapSeen::Ours(at)) => Action::Done(at),\n", "a"),
 ("P6 unknown and rewound looked up when the snapshot is broken (F1's fix)", PL,
  "    let anew = |rewound: bool| {\n        if let Some(why) = broken {\n",
  "    let anew = |rewound: bool| {\n        if let (Some(_), None) = (broken, seen.snap) {\n            return Action::Look;\n        }\n        if let Some(SnapSeen::Ours(commit)) = seen.snap {\n            return Action::Done(commit);\n        }\n        if let Some(why) = broken {\n", "a"),
 ("P7 Held recorded before the hold is seen", PL,
  "            if !pool.held {\n                return Action::Hold;\n            }\n            if pool.load_guid == publish.count.load_guid {\n",
  "            if pool.load_guid == publish.count.load_guid {\n", "a"),
 # The #140 follow-ups (Wb, Q5, Q7): the new tests must fail.
 ("Q5 a second state of the top's group not remembered", CU,
  "                if top.state != Some(state) {\n                    top.state = None;\n                }\n",
  "", "l"),
 ("Q7 unverified uberblocks in the rings raise the top", CU,
  "                if uberblock.verified() && uberblock.counts() {\n                    self.saw(uberblock.state);\n",
  "                if uberblock.counts() {\n                    self.saw(uberblock.state);\n", "l"),
 ("Wb the limit counted from the handler's making", HD,
  "    Ok(Box::new(move |request: &Request, manifest: Vec<u8>| {\n        let waiter = Waiter {\n            runtime: &runtime,\n            limit,\n            began: std::time::Instant::now(),\n        };\n",
  "    let began = std::time::Instant::now();\n    Ok(Box::new(move |request: &Request, manifest: Vec<u8>| {\n        let waiter = Waiter {\n            runtime: &runtime,\n            limit,\n            began,\n        };\n", "d"),
]
only = sys.argv[1:]
res = []
for name, f, old, new, suites in P:
    if only and not any(name.startswith(o + " ") or name == o for o in only):
        continue
    path = os.path.join(W, f)
    if not os.path.exists(path):
        res.append((name, "NOT APPLIED (no file)"))
        print(f"{name}: NOT APPLIED (no file)", flush=True)
        continue
    with open(path) as h:
        text = h.read()
    n = text.count(old)
    if n != 1:
        res.append((name, f"NOT APPLIED (found {n})"))
        print(f"{name}: NOT APPLIED ({n})", flush=True)
        continue
    t = time.time()
    verdict = "SURVIVED"
    try:
        with open(path, "w") as h:
            h.write(text.replace(old, new))
        for s in suites:
            try:
                r = subprocess.run(SUITES[s], cwd=W, env=ENV, capture_output=True, text=True, timeout=600)
            except subprocess.TimeoutExpired:
                verdict = f"KILLED ({s}: timeout, a hang)"
                break
            out = r.stdout + r.stderr
            if "error[E" in out or "could not compile" in out:
                verdict = "BUILD ERROR"
                print(out[-3000:])
                break
            if r.returncode != 0:
                failed = [l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.endswith("FAILED")]
                verdict = f"KILLED ({s}): " + ", ".join(failed[:4]) + (f" and {len(failed) - 4} more" if len(failed) > 4 else "")
                break
    finally:
        with open(path, "w") as h:
            h.write(text)
    res.append((name, verdict))
    print(f"{name}: {verdict} ({time.time() - t:.0f}s)", flush=True)
    with open(OUT, "w") as h:
        json.dump(res, h, indent=1)
print("done")
