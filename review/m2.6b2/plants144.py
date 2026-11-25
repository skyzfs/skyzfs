"""Textual plants for the review of #144 (M2.6b2) at b5063b2, one at a
time, each file restored from its saved text in `finally`, 600 s each.
A plant is a list of edits (file, old, new); each old text must occur
once, or the plant is NOT APPLIED. Run from a worktree of the head:
WORKTREE=<path> OUT=<json> python3 plants144.py [name prefixes...]
The suite skips the review's own scratch tests (`--skip review`).

P*, R*, X*, L*, M*: the author's 35, rebuilt from their names in the
description (the edits themselves are not in the pull request). V*: the
review's own."""
import json, os, subprocess, sys, time

W = os.environ.get("WORKTREE", ".")
OUT = os.environ.get("OUT", "plants144.json")
ENV = dict(os.environ, CARGO_INCREMENTAL="0")
SUITE = ["cargo", "test", "-p", "skyzfs-agent", "--lib", "--", "--skip", "review"]
PL = "crates/skyzfs-agent/src/plan.rs"
EN = "crates/skyzfs-agent/src/engine.rs"
RC = "crates/skyzfs-agent/src/record.rs"
FS = "crates/skyzfs-agent/src/fs.rs"
MO = "crates/skyzfs-agent/src/fs/model.rs"

REPLACE = ("    fs.write(&tmp, bytes)?;\n    fs.sync_file(&tmp)?;\n"
           "    fs.rename(&tmp, &dir.join(name))?;\n    fs.sync_dir(dir)\n")
MAKE_DIR_SYNC = ("    match parent_of(dir) {\n        Some(parent) => fs.sync_dir(parent),\n"
                 "        None => Ok(()),\n    }\n")
REMOVE_SYNC = ("        match self.fs.sync_dir(&dir) {\n            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),\n"
               "            other => other,\n        }\n")
WRITE_MANIFEST = ("        let dir = self.manifests(name)?;\n        make_dir(&self.fs, &dir)?;\n"
                  "        replace(&self.fs, &dir, name, bytes)\n")
ASK_ANEW_CHECKS = ("        let before = self.zfs.props(&self.pool)?.load_guid;\n"
                   "        let members = self.zfs.members(&self.pool, &name)?;\n"
                   "        let held = self.zfs.held(&self.pool, &name, HOLD_TAG)?;\n"
                   "        let whole = members.is_some_and(|members| plan::same_snapshot(&members, &taken));\n"
                   "        if !whole || !held {\n"
                   "            // Seen again at the next round, which plans from it.\n"
                   "            return Ok(Moved::No);\n        }\n"
                   "        let Ok(count) = self.count(before) else {\n            return Ok(Moved::No);\n        };\n")
ASK_ANEW_COUNT_FIRST = ("        let before = self.zfs.props(&self.pool)?.load_guid;\n"
                        "        let Ok(count) = self.count(before) else {\n            return Ok(Moved::No);\n        };\n"
                        "        let members = self.zfs.members(&self.pool, &name)?;\n"
                        "        let held = self.zfs.held(&self.pool, &name, HOLD_TAG)?;\n"
                        "        let whole = members.is_some_and(|members| plan::same_snapshot(&members, &taken));\n"
                        "        if !whole || !held {\n"
                        "            // Seen again at the next round, which plans from it.\n"
                        "            return Ok(Moved::No);\n        }\n")

P = [
    # The planner and the engine (the author's P1 to P22).
    ("P1 unknown broken: no look", [(PL, "            _ if broken.is_some() => look(&|| anew(false)),\n", "")]),
    ("P2 rewound broken: no look", [(PL, "            Code::Rewound if broken.is_some() => look(&|| anew(true)),\n", "")]),
    ("P3 replaced not reported", [(PL, "        Some(Why::Destroyed | Why::Replaced) => Some(Report::SnapshotGone),\n",
                                    "        Some(Why::Destroyed) => Some(Report::SnapshotGone),\n")]),
    ("P4 no report", [(PL, "        Some(Why::Destroyed | Why::Replaced) => Some(Report::SnapshotGone),\n",
                       "        Some(Why::Destroyed | Why::Replaced) => None,\n")]),
    ("P5 S1 premise not at intent", [(PL, "            (None, DaemonSeen::Here { .. }) if !pool.premise => Action::Abandon(Why::Premise),\n", "")]),
    ("P6 S3 by name", [(PL, "        let mut guids: Vec<u64> = members.iter().map(|m| m.snapshot_guid).collect();\n",
                        "        let mut guids: Vec<(String, u64)> = members.iter().map(|m| (m.name.clone(), m.snapshot_guid)).collect();\n")]),
    ("P7 S11 guard", [(PL, "    let assisted_ms = assisted_ms.filter(|&last| last <= now_ms);\n", "")]),
    ("P8 stalls with load_guid", [(PL, "    a.attachment == b.attachment && a.state == b.state && a.after == b.after\n",
                                   "    a.attachment == b.attachment && a.state == b.state && a.after == b.after && a.load_guid == b.load_guid\n")]),
    ("P9 stalls at held", [(PL, "        Phase::Asked { .. } if same_place(count, &publish.count) => {\n",
                            "        Phase::Asked { .. } | Phase::Held(_) if same_place(count, &publish.count) => {\n")]),
    ("P10 report from 4", [(PL, "    (stalls >= STALLS_REPORTED).then_some", "    (stalls > STALLS_REPORTED).then_some")]),
    ("P11 abandon early", [(PL, "        if stalled && publish.stalls.saturating_add(1) >= STALLS_ABANDONED {\n",
                            "        if stalled && publish.stalls.saturating_add(2) >= STALLS_ABANDONED {\n")]),
    ("P12 no Stalled record", [(PL, "            return Action::Stalled;\n", "            return give_up(pool, Why::NoProgress);\n")]),
    ("P13 pending not withdrawn", [(PL, "            Reply::Pending => Action::Withdraw,\n            _ => look(",
                                    "            _ => look(")]),
    ("P14 no look at 8", [(PL, "            _ => look(&|| give_up(pool, broken.unwrap_or(Why::NoProgress))),\n",
                           "            _ => give_up(pool, broken.unwrap_or(Why::NoProgress)),\n")]),
    ("P15 assist value fixed", [(EN, "        let value = self.clock.now_ms().max(self.assists.saturating_add(1));\n",
                                 "        let value = 1;\n")]),
    ("P16 now_for wall", [(EN, "        let Phase::Asked { asked_ms, .. } = publish.phase else {\n            return wall;\n        };\n",
                           "        let Phase::Asked { asked_ms, .. } = publish.phase else {\n            return wall;\n        };\n        if asked_ms > 0 {\n            return wall;\n        }\n")]),
    ("P17 ask_anew stays Asked", [(EN, "                p.stalls = stalls;\n                p.phase = Phase::Held(recorded);\n",
                                   "                p.stalls = stalls;\n                let _ = &recorded;\n")]),
    ("P18 D5 after check", [(EN, "        if after != before {\n            return Ok(Moved::No);\n        }\n", "        let _ = after;\n")]),
    ("P19 report each time", [(EN, "                if self.reported.insert(name.clone()) {\n",
                               "                if self.reported.insert(name.clone()) || true {\n")]),
    ("P20 engine stalls from planner ignored", [(EN, "                p.stalls = stalls;\n                p.phase = Phase::Held(recorded);\n",
                                                 "                p.phase = Phase::Held(recorded);\n")]),
    ("P21 report at 3 only", [(PL, "    (stalls >= STALLS_REPORTED).then_some", "    (stalls == STALLS_REPORTED).then_some")]),
    ("P22 reported never forgotten", [(EN, "                self.reported.remove(&name);\n", "")]),
    # The review's R4 to R8 (#142), on the store's file-system trait.
    ("R4 no file sync before rename", [(RC, "    fs.sync_file(&tmp)?;\n", "")]),
    ("R5 no dir sync after rename", [(RC, "    fs.rename(&tmp, &dir.join(name))?;\n    fs.sync_dir(dir)\n",
                                      "    fs.rename(&tmp, &dir.join(name))\n")]),
    ("R6 in place", [(RC, REPLACE, "    let _ = &tmp;\n    fs.write(&dir.join(name), bytes)?;\n    fs.sync_file(&dir.join(name))?;\n    fs.sync_dir(dir)\n")]),
    ("R7 new dir parent not synced", [(RC, MAKE_DIR_SYNC, "    let _ = parent_of(dir);\n    Ok(())\n")]),
    ("R8 removed manifest dir not synced", [(RC, REMOVE_SYNC, "        Ok(())\n")]),
    # The store's retry paths.
    ("X1 remove: NotFound returns without sync", [(RC, "            Err(e) if e.kind() == io::ErrorKind::NotFound => {}\n            Err(e) => return Err(e),\n        }\n        match self.fs.sync_dir",
                                                    "            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),\n            Err(e) => return Err(e),\n        }\n        match self.fs.sync_dir")]),
    ("X2 make_dir: exists returns without sync", [(RC, "        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}\n",
                                                    "        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Ok(()),\n")]),
    ("X3 save: no corrupt check", [(RC, "        if let Loaded::Corrupt(why) = self.load()? {\n            return Err(SaveError::Corrupt(why));\n        }\n", "")]),
    ("X4 manifest dir made without its parent's sync", [(RC, WRITE_MANIFEST,
        "        let dir = self.manifests(name)?;\n        match self.fs.create_dir(&dir) {\n            Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e),\n            _ => {}\n        }\n        replace(&self.fs, &dir, name, bytes)\n")]),
    # The lock.
    ("L1 no lock", [(MO, "        if !self.locks.borrow_mut().insert(path.to_path_buf()) {\n",
                     "        if !self.locks.borrow_mut().insert(path.to_path_buf()) && false {\n"),
                    (FS, "        match file.try_lock() {\n", "        match Ok::<(), TryLockError>(()) {\n")]),
    ("L2 model lock not released", [(MO, "        self.locks.borrow_mut().remove(&self.path);\n", "        let _ = &self.path;\n")]),
    # The model itself.
    ("M1 model: rename keeps bytes", [(MO, "        if let Some(node) = state.entries(from_dir).remove(&from_name) {\n            state.entries(to_dir).insert(to_name, node);\n        }\n",
                                       "        if let Some(node) = state.entries(from_dir).remove(&from_name) {\n            state.entries(to_dir).insert(to_name, node);\n"
                                       "            if let Node::File { bytes, synced, .. } = &mut state.nodes[node] {\n                *synced = bytes.clone();\n            }\n        }\n")]),
    ("M2 model: failed sync syncs", [(MO, "                if failed.is_err() {\n                    *clean_but_lost = true;\n",
                                      "                if failed.is_err() {\n                    *synced = bytes.clone();\n")]),

    # The review's own: the record's write path (the brief's list).
    ("V1 rename before the file's sync", [(RC, REPLACE, "    fs.write(&tmp, bytes)?;\n    fs.rename(&tmp, &dir.join(name))?;\n    fs.sync_file(&dir.join(name))?;\n    fs.sync_dir(dir)\n")]),
    ("V2 a file sync's error ignored", [(RC, "    fs.sync_file(&tmp)?;\n", "    let _ = fs.sync_file(&tmp);\n")]),
    ("V3 a file sync that failed asked again once (Linux's fsync retry)", [(RC, "    fs.sync_file(&tmp)?;\n", "    fs.sync_file(&tmp).or_else(|_| fs.sync_file(&tmp))?;\n")]),
    ("V4 the record dir sync's error ignored", [(RC, "    fs.rename(&tmp, &dir.join(name))?;\n    fs.sync_dir(dir)\n",
                                                  "    fs.rename(&tmp, &dir.join(name))?;\n    let _ = fs.sync_dir(dir);\n    Ok(())\n")]),
    ("V5 remove_manifest's sync error ignored", [(RC, REMOVE_SYNC, "        let _ = self.fs.sync_dir(&dir);\n        Ok(())\n")]),
    ("V6 make_dir's parent sync error ignored", [(RC, MAKE_DIR_SYNC, "    if let Some(parent) = parent_of(dir) {\n        let _ = fs.sync_dir(parent);\n    }\n    Ok(())\n")]),
    ("V7 write_manifest without its directory's sync", [(RC, WRITE_MANIFEST,
        "        let dir = self.manifests(name)?;\n        make_dir(&self.fs, &dir)?;\n        let tmp = dir.join(format!(\".{name}.tmp\"));\n"
        "        self.fs.write(&tmp, bytes)?;\n        self.fs.sync_file(&tmp)?;\n        self.fs.rename(&tmp, &dir.join(name))\n")]),
    ("V8 write_manifest without its file's sync", [(RC, WRITE_MANIFEST,
        "        let dir = self.manifests(name)?;\n        make_dir(&self.fs, &dir)?;\n        let tmp = dir.join(format!(\".{name}.tmp\"));\n"
        "        self.fs.write(&tmp, bytes)?;\n        self.fs.rename(&tmp, &dir.join(name))?;\n        self.fs.sync_dir(&dir)\n")]),
    ("V9 Disk: sync_dir does nothing", [(FS, "    fn sync_dir(&self, path: &Path) -> io::Result<()> {\n        File::open(path)?.sync_all()\n",
                                         "    fn sync_dir(&self, path: &Path) -> io::Result<()> {\n        File::open(path).map(|_| ())\n")]),
    ("V10 Disk: sync_file does nothing", [(FS, "        File::open(path)?.sync_all()\n    }\n\n    fn rename",
                                           "        File::open(path).map(|_| ())\n    }\n\n    fn rename")]),
    ("V11 Disk: write does not truncate", [(FS, "            .truncate(true)\n            .open(path)?\n            .write_all(bytes)\n",
                                            "            .truncate(false)\n            .open(path)?\n            .write_all(bytes)\n")]),
    # The review's own: the engine and the planner.
    ("V12 Stalled leaves the phase at Asked", [(EN, "                        p.stalls = stalls;\n                        p.phase = Phase::Held(taken);\n",
                                                 "                        p.stalls = stalls;\n                        let _ = &taken;\n")]),
    ("V13 Stalled does not count the eighth", [(EN, "                let stalls = publish.stalls.saturating_add(1);\n", "                let stalls = publish.stalls;\n")]),
    ("V14 a stall by the count alone (attachment and state not compared)", [(PL, "    let stalled = asked.is_some()\n        && cuts.attachment == publish.count.attachment\n        && cuts.state == Some(publish.count.state)\n        && cuts.count == publish.count.after;\n",
                                                                              "    let stalled = asked.is_some() && cuts.count == publish.count.after;\n")]),
    ("V15 AskAnew reads the count before it verifies the hold and members", [(EN, ASK_ANEW_CHECKS, ASK_ANEW_COUNT_FIRST)]),
    ("V16 AskAnew without the members' check", [(EN, "        if !whole || !held {\n            // Seen again", "        if !held {\n            let _ = whole;\n            // Seen again")]),
    ("V17 AskAnew without the hold's check", [(EN, "        if !whole || !held {\n            // Seen again", "        if !whole {\n            let _ = held;\n            // Seen again")]),
    ("V18 at the eighth, a broken snapshot given up as NoProgress", [(PL, "            _ => look(&|| give_up(pool, broken.unwrap_or(Why::NoProgress))),\n",
                                                                       "            _ => look(&|| give_up(pool, Why::NoProgress)),\n")]),
    ("V19 the bound on stalls never applied", [(PL, "        if stalled && publish.stalls.saturating_add(1) >= STALLS_ABANDONED {\n",
                                                 "        if stalled && false {\n")]),
    ("V20 SnapshotGone where the premise alone is refused", [(PL, "        Some(Why::Destroyed | Why::Replaced) => Some(Report::SnapshotGone),\n",
                                                               "        Some(_) => Some(Report::SnapshotGone),\n")]),
    ("V21 the assist's group not counted as a stall's end (stalled ignores cuts.count)", [(PL, "        && cuts.count == publish.count.after;\n", "        && cuts.count >= publish.count.after;\n")]),
    ("V22 the report of Done dropped by the engine", [(EN, "                if let Some(report) = report {\n                    self.reports.push((name, report));\n                }\n",
                                                        "                let _ = report;\n")]),
]

want = sys.argv[1:]
res = []
for name, edits in P:
    if want and not any(name.startswith(w + " ") or name == w for w in want):
        continue
    files = sorted({os.path.join(W, f) for f, _, _ in edits})
    saved = {}
    for path in files:
        with open(path) as h:
            saved[path] = h.read()
    bad = None
    texts = dict(saved)
    for f, old, new in edits:
        path = os.path.join(W, f)
        n = texts[path].count(old)
        if n != 1:
            bad = f"NOT APPLIED ({f}: found {n})"
            break
        texts[path] = texts[path].replace(old, new)
    if bad:
        res.append((name, bad))
        print(f"{name}: {bad}", flush=True)
        continue
    t = time.time()
    verdict = "SURVIVED"
    try:
        for path, text in texts.items():
            with open(path, "w") as h:
                h.write(text)
        try:
            r = subprocess.run(SUITE, cwd=W, env=ENV, capture_output=True, text=True, timeout=600)
        except subprocess.TimeoutExpired:
            r = None
            verdict = "KILLED (timeout, a hang)"
        if r is not None:
            out = r.stdout + r.stderr
            if "error[E" in out or "could not compile" in out:
                verdict = "BUILD ERROR"
                print(out[-3000:])
            elif r.returncode != 0:
                failed = [l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.endswith("FAILED")]
                verdict = "KILLED: " + ", ".join(failed[:4]) + (f" and {len(failed) - 4} more" if len(failed) > 4 else "")
    finally:
        for path, text in saved.items():
            with open(path, "w") as h:
                h.write(text)
    res.append((name, verdict))
    print(f"{name}: {verdict} ({time.time() - t:.0f}s)", flush=True)
    with open(OUT, "w") as h:
        json.dump(res, h, indent=1)
print("done")
