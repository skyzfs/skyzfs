"""Textual plants for the review of #140 (M2.6a), one at a time, each file
restored from its saved text after the run. Run from a worktree:
WORKTREE=<path> python3 plants140.py [name prefixes...]"""
import json, os, subprocess, sys, time

W = os.environ.get("WORKTREE", ".")
OUT = os.environ.get("OUT", "plants140.json")
ENV = dict(os.environ, CARGO_INCREMENTAL="0")
FEAT = "skyzfsd/test-hooks,skyzfs-local/test-hooks"
SUITES = {
    "l": ["cargo", "test", "-p", "skyzfs-local", "--", "--skip", "review"],
    "u": ["cargo", "test", "-p", "skyzfs-upload", "--", "--skip", "review"],
    "d": ["cargo", "test", "-p", "skyzfsd", "--features", FEAT, "--", "--skip", "review"],
    "c": ["cargo", "test", "-p", "skyzfs-control"],
    # The integration tests of publish alone (commit 5's), on the head and the base.
    "p": ["cargo", "test", "-p", "skyzfs-upload", "--test", "publish"],
}
if os.environ.get("SUITE"):
    FORCE = os.environ["SUITE"]
else:
    FORCE = None
CU = "crates/skyzfs-local/src/inplace/cuts.rs"
PB = "crates/skyzfs-upload/src/publish.rs"
DR = "crates/skyzfs-upload/src/driver.rs"
LO = "crates/skyzfs-upload/src/local.rs"
LK = "crates/skyzfs-upload/src/look.rs"
SE = "bins/skyzfsd/src/serve.rs"
UP = "bins/skyzfsd/src/upload.rs"
HD = "bins/skyzfsd/src/control/handler.rs"
CT = "bins/skyzfsd/src/control.rs"
CC = "crates/skyzfs-control/src/lib.rs"
P = [
 # Rule (R): the author's sample of eight, and the two mutants by hand.
 ("R1 the driver gives no mark at the check", DR,
  "        self.publishing.set_rewound(cut.rewound);\n", "", "u"),
 ("R2 no refusal at the ask", PB,
  "        if request.after < self.rewound {\n", "        if false && request.after < self.rewound {\n", "ud"),
 ("R3 the mark goes back", PB,
  "        self.rewound = self.rewound.max(rewound);\n", "        self.rewound = rewound;\n", "u"),
 ("R4 a publish given its cut ended", PB,
  "matches!(p.stage, Stage::Look | Stage::Wait),", "matches!(p.stage, Stage::Look | Stage::Wait | Stage::Ready(_)),", "u"),
 ("R5 publishes ended at the mark itself", PB,
  "|p| p.request.after < mark &&", "|p| p.request.after <= mark &&", "u"),
 ("R6 a request refused at the mark itself", PB,
  "        if request.after < self.rewound {\n", "        if request.after <= self.rewound {\n", "u"),
 ("R7 the store offers no mark", LO,
  "            rewound: InPlace::cuts(self).rewound,\n", "            rewound: 0,\n", "u"),
 ("R8a the daemon's start reads no mark", SE,
  "    let rewound = served.cuts().rewound;\n", "    let rewound = 0;\n", "d"),
 ("R8b the daemon's start gives the uploader no mark", UP,
  "                uploader.publishing().set_rewound(publishes.rewound);\n",
  "                let _ = publishes.rewound;\n", "du"),
 ("R9 REWIND_UNSEEN by hand: no rewind marked", CU,
  "        if rewound && !self.mutants.has(Mutants::REWIND_UNSEEN) {\n",
  "        if false && rewound && !self.mutants.has(Mutants::REWIND_UNSEEN) {\n", "lu"),
 ("R10 REWIND_START_FORGOTTEN by hand", CU,
  "        if !self.mutants.has(Mutants::REWIND_START_FORGOTTEN) {\n            self.cuts.rewound = count;\n",
  "        if false && !self.mutants.has(Mutants::REWIND_START_FORGOTTEN) {\n            self.cuts.rewound = count;\n", "lu"),
 # Rule (R): mine.
 ("M1 an equal group is no rewind (< for <=)", CU,
  ".is_some_and(|before| cut.group <= before.group);", ".is_some_and(|before| cut.group < before.group);", "lu"),
 ("M2 the check gives a cut found committed to no publish", DR,
  "                self.committed(&cut, commit);\n                let step = self.found_committed(&cut).await;\n",
  "                let _ = commit;\n                let step = self.found_committed(&cut).await;\n", "u"),
 ("M3 the mark checked before the same request pending", PB,
  "        if let Some(pending) = self\n            .pending\n            .iter_mut()\n            .find(|p| p.request.name == request.name)\n",
  "        if request.after < self.rewound {\n            return Answer::Refused(Refusal::Rewound);\n        }\n        if let Some(pending) = self\n            .pending\n            .iter_mut()\n            .find(|p| p.request.name == request.name)\n", "ud"),
 ("M4 the mark at a start one below the count", CU,
  "            self.cuts.rewound = count;\n", "            self.cuts.rewound = count.saturating_sub(1);\n", "lu"),
 ("M5 the mark one below the rewind's cut", CU,
  "            self.cuts.rewound = cut.number;\n", "            self.cuts.rewound = cut.number - 1;\n", "lu"),
 # look: the author's sample.
 ("L1 another commit's row read as ours", LK,
  "            Ok(row) if row.commit != commit =>", "            Ok(row) if false && row.commit != commit =>", "u"),
 ("L2 no row read as absent", LK,
  "            return unreadable(Unread::NoRow);\n", "            return Seen::Absent;\n", "u"),
 ("L3 an error answered as absent", HD,
  "        Some(Err(e)) => Err(Reply::Refused(Code::Store, format!(\"object storage: {e}\"))),\n",
  "        Some(Err(_)) => Ok(Reply::Absent),\n", "d"),
 ("L4 another volume looked up (serve)", SE,
  "                    vol: remote.id,\n", "                    vol: skynbd_types::VolumeId(remote.id.0.increment().unwrap_or_default()),\n", "d"),
 ("L5 number and group swapped", CT,
  "            number: row.cut.number,\n            group: row.cut.group,\n",
  "            number: row.cut.group,\n            group: row.cut.number,\n", "d"),
 ("L6 version read as length", CT,
  "SnapRowError::Version(_)) => Unread::Version,", "SnapRowError::Version(_)) => Unread::Length,", "d"),
 ("L7 the name not checked for look", CT,
  "        Request::Progress(name) | Request::Withdraw(name) | Request::Look(name) => Some(name),\n",
  "        Request::Progress(name) | Request::Withdraw(name) => Some(name),\n        Request::Look(_) => None,\n", "d"),
 # look: mine.
 ("L8 the row's read error taken as no row", LK,
  "    let row = refs.meta_get(vol, &snap_row_key(name)).await?;\n",
  "    let row = refs.meta_get(vol, &snap_row_key(name)).await.ok().flatten();\n", "ud"),
 ("L9 no answer in time read as a store error", HD,
  "        None => Err(Reply::Refused(\n            Code::NoAnswer,\n            format!(\n                \"object storage did not answer",
  "        None => Err(Reply::Refused(\n            Code::Store,\n            format!(\n                \"object storage did not answer", "d"),
 ("L10 the ref read but not the row (row = None)", LK,
  "    let row = refs.meta_get(vol, &snap_row_key(name)).await?;\n",
  "    let row = { let _ = snap_row_key; None };\n", "ud"),
 ("W1 the client parses group as number", CC,
  "                    number: number(&p, \"number\")?,\n                    group: number(&p, \"group\")?,\n",
  "                    number: number(&p, \"group\")?,\n                    group: number(&p, \"number\")?,\n", "cd"),
 ("W2 rewound answered as taken", CT,
  "        Refusal::Rewound => Code::Rewound,\n", "        Refusal::Rewound => Code::Taken,\n", "dc"),
 # Commit 5: the M2.5 tests changed; run on the head and on the base.
 ("C1 itself not required", PB,
  "            && cut.itself\n", "            && (cut.itself || true)\n", "u"),
 ("C2 the row's commit not compared", PB,
  "        row.commit == commit\n            && self.carried_by(&cut)\n",
  "        self.carried_by(&cut)\n", "u"),
 ("C3 the manifest not compared", PB,
  "            && row.manifest_len as usize == self.manifest.len()\n            && row.manifest_hash == *blake3::hash(&self.manifest).as_bytes()\n",
  "\n", "u"),
 ("M2b the check offers a cut found committed only to count it not offered", DR,
  "                self.committed(&cut, commit);\n                let step = self.found_committed(&cut).await;\n",
  "                if commit.is_none() {\n                    self.committed(&cut, commit);\n                }\n                let step = self.found_committed(&cut).await;\n", "u"),
 ("M7 a cut found committed offered on head though head moved (F1 of #122)", DR,
  "    (named.commit == Some(seen.head)).then_some(seen.head)\n",
  "    let _ = named;\n    Some(seen.head)\n", "u"),
 ("C4 the C threshold not compared in carried_by", PB,
  "            && cut.cut.number > self.after\n", "            && cut.cut.number >= self.after\n", "u"),
]
only = sys.argv[1:]
res = []
for name, f, old, new, suites in P:
    if only and not any(name.startswith(o + " ") or name == o for o in only):
        continue
    if old == new:
        continue
    path = os.path.join(W, f)
    if not os.path.exists(path):
        res.append((name, "NOT APPLIED (no file)"))
        print(f"{name}: NOT APPLIED (no file)", flush=True)
        continue
    text = open(path).read()
    n = text.count(old)
    if n != 1:
        res.append((name, f"NOT APPLIED (found {n})"))
        print(f"{name}: NOT APPLIED ({n})", flush=True)
        continue
    t = time.time()
    verdict = "SURVIVED"
    try:
        open(path, "w").write(text.replace(old, new))
        for s in (FORCE or suites):
            try:
                r = subprocess.run(SUITES[s], cwd=W, env=ENV, capture_output=True, text=True, timeout=1200)
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
        open(path, "w").write(text)
    res.append((name, verdict))
    print(f"{name}: {verdict} ({time.time() - t:.0f}s)", flush=True)
    json.dump(res, open(OUT, "w"), indent=1)
print("done")
