"""Textual plants for the next look at #144 (M2.6b2) at 45648ab, one at a
time, each file restored from its saved text in `finally`, 600 s each.
A plant is a list of edits (file, old, new); each old text must occur
once, or the plant is NOT APPLIED. Run from a clean worktree of the
head: WORKTREE=<path> OUT=<json> python3 next144.py [name prefixes...]

F*: F1's fix (28d1d64) and T1's check. T2*: D5's re-checks (the first
look's V15 to V17). T3*: `Disk` (the first look's V9 to V11, and the
author's T3b to T3d as their names read). A*: the age bound (45648ab).
Q*: the pace (f6ef194)."""
import json, os, subprocess, sys, time

W = os.environ.get("WORKTREE", ".")
OUT = os.environ.get("OUT", "next144.json")
ENV = dict(os.environ, CARGO_INCREMENTAL="0")
SUITE = ["cargo", "test", "-p", "skyzfs-agent", "--lib", "--", "--skip", "review"]
PL = "crates/skyzfs-agent/src/plan.rs"
EN = "crates/skyzfs-agent/src/engine.rs"
FS = "crates/skyzfs-agent/src/fs.rs"

REPORT = ("                if let Some(report) = report {\n"
          "                    let made = (name.clone(), report);\n"
          "                    if !self.reports.contains(&made) {\n"
          "                        self.reports.push(made);\n"
          "                    }\n"
          "                }\n")
SAVE_DONE = ("                self.commit(Effect::Done, |r| {\n"
             "                    set(r, &name, |p| p.phase = Phase::Done { taken, commit })\n"
             "                })?;\n")
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
ENDED = ("    let ended = if publish.stalls >= STALLS_ABANDONED {\n"
         "        Some(Why::NoProgress)\n"
         "    } else if seen.too_old {\n"
         "        Some(Why::TooOld)\n"
         "    } else {\n"
         "        None\n"
         "    };\n")

P = [
    # F1 (28d1d64) and T1.
    ("F1a the report after the save of Done (the order at b5063b2)", [(EN, REPORT + SAVE_DONE, SAVE_DONE + REPORT)]),
    ("F1b the report of Done dropped (the first look's V22)", [(EN,
        "                    if !self.reports.contains(&made) {\n                        self.reports.push(made);\n                    }\n",
        "                    let _ = made;\n")]),
    ("F1c the report of Done made each time it is planned (no de-duplication)", [(EN,
        "                    if !self.reports.contains(&made) {\n", "                    if !made.0.is_empty() {\n")]),
    ("F1d the report of Done de-duplicated by name alone", [(EN,
        "                    if !self.reports.contains(&made) {\n",
        "                    if !self.reports.iter().any(|(n, _)| *n == made.0) {\n")]),
    # T2: D5's re-checks in ask_anew.
    ("T2a count before the members and hold checks (V15)", [(EN, ASK_ANEW_CHECKS, ASK_ANEW_COUNT_FIRST)]),
    ("T2b no members check (V16)", [(EN, "        if !whole || !held {\n            // Seen again", "        if !held {\n            let _ = whole;\n            // Seen again")]),
    ("T2c no hold check (V17)", [(EN, "        if !whole || !held {\n            // Seen again", "        if !whole {\n            let _ = held;\n            // Seen again")]),
    # T3: Disk.
    ("T3a Disk::write does not truncate (V11)", [(FS, "            .truncate(true)\n            .open(path)?\n            .write_all(bytes)\n",
                                                  "            .truncate(false)\n            .open(path)?\n            .write_all(bytes)\n")]),
    ("T3b Disk::unlink does nothing", [(FS, "        fs::remove_file(path)\n", "        fs::metadata(path).map(|_| ())\n")]),
    ("T3c Disk::rename does nothing", [(FS, "        fs::rename(from, to)\n", "        fs::metadata(from).map(|_| ()).and(fs::metadata(to).map(|_| ()).or(Ok(())))\n")]),
    ("T3d Disk::create_dir does not refuse a directory that is there", [(FS, "        fs::create_dir(path)\n", "        fs::create_dir_all(path)\n")]),
    ("T3e Disk::list leaves the last name out", [(FS, "                names.push(name.to_string());\n            }\n        }\n        Ok(names)\n",
                                                  "                names.push(name.to_string());\n            }\n        }\n        names.pop();\n        Ok(names)\n")]),
    ("T3f Disk::sync_dir does nothing (V9)", [(FS, "    fn sync_dir(&self, path: &Path) -> io::Result<()> {\n        File::open(path)?.sync_all()\n",
                                               "    fn sync_dir(&self, path: &Path) -> io::Result<()> {\n        File::open(path).map(|_| ())\n")]),
    ("T3g Disk::sync_file does nothing (V10)", [(FS, "        File::open(path)?.sync_all()\n    }\n\n    fn rename",
                                                 "        File::open(path).map(|_| ())\n    }\n\n    fn rename")]),
    # The age bound (45648ab).
    ("A1 never too old", [(PL, "    now_ms.saturating_sub(begun_ms) >= age_max\n", "    let _ = (now_ms, begun_ms, age_max);\n    false\n")]),
    ("A2 too old one ms late (> for >=)", [(PL, "    now_ms.saturating_sub(begun_ms) >= age_max\n", "    now_ms.saturating_sub(begun_ms) > age_max\n")]),
    ("A3 a clock stepped back read as old", [(PL, "    now_ms.saturating_sub(begun_ms) >= age_max\n", "    now_ms.abs_diff(begun_ms) >= age_max\n")]),
    ("A4 the age on the wall clock in the process that began it", [(EN,
        "            Some((at, mono)) if at == publish.begun_ms => {\n                at.saturating_add(self.clock.mono_ms().saturating_sub(mono))\n            }\n            _ => self.clock.now_ms(),\n",
        "            Some((at, mono)) if at == publish.begun_ms && false => {\n                at.saturating_add(self.clock.mono_ms().saturating_sub(mono))\n            }\n            _ => self.clock.now_ms(),\n")]),
    ("A5 the age measured from the planner's time (now_for)", [(EN,
        "        let too_old = plan::too_old(publish.begun_ms, self.age_now(publish), self.age_max);\n",
        "        let too_old = plan::too_old(publish.begun_ms, now_ms, self.age_max);\n")]),
    ("A6 begun not remembered in begin", [(EN,
        "        self.memory.entry(name.clone()).or_default().begun = Some((begun_ms, mono));\n",
        "        let _ = mono;\n")]),
    ("A7 the age bound at Intent only with the daemon here", [(PL,
        "            (None, _) if seen.too_old => Action::Abandon(Why::TooOld),\n",
        "            (None, DaemonSeen::Here { .. }) if seen.too_old => Action::Abandon(Why::TooOld),\n")]),
    ("A8 the age bound not applied at Taken", [(PL, "            if seen.too_old {\n                return give_up(pool, Why::TooOld);\n            }\n", "")]),
    ("A9 at Taken, too old before broken", [(PL,
        "            if let Some(why) = broken(pool, taken) {\n                return give_up(pool, why);\n            }\n            if seen.too_old {\n                return give_up(pool, Why::TooOld);\n            }\n",
        "            if seen.too_old {\n                return give_up(pool, Why::TooOld);\n            }\n            if let Some(why) = broken(pool, taken) {\n                return give_up(pool, why);\n            }\n")]),
    ("A10 too old held or asked: no look", [(PL, "            _ => look(&|| give_up(pool, broken.unwrap_or(why))),\n",
                                             "            _ => give_up(pool, broken.unwrap_or(why)),\n")]),
    ("A11 too old: a pending request not withdrawn", [(PL,
        "        return match answer {\n            Reply::Pending => Action::Withdraw,\n            _ => look(&|| give_up(pool, broken.unwrap_or(why))),\n",
        "        return match answer {\n            _ => look(&|| give_up(pool, broken.unwrap_or(why))),\n")]),
    ("A12 too old over eight stalls", [(PL, ENDED,
        "    let ended = if seen.too_old {\n        Some(Why::TooOld)\n    } else if publish.stalls >= STALLS_ABANDONED {\n        Some(Why::NoProgress)\n    } else {\n        None\n    };\n")]),
    ("A13 too old given up while the daemon is away (no answer at all)", [(PL,
        "    let (cuts, answer) = match &seen.daemon {\n        DaemonSeen::Away => return away(),\n",
        "    let (cuts, answer) = match &seen.daemon {\n        DaemonSeen::Away if seen.too_old => return give_up(pool, Why::TooOld),\n        DaemonSeen::Away => return away(),\n")]),
    ("A14 too old: its own reason over the snapshot's", [(PL, "            _ => look(&|| give_up(pool, broken.unwrap_or(why))),\n",
                                                          "            _ => look(&|| give_up(pool, broken.map_or(why, |_| why))),\n")]),
    ("A15 begun_ms not kept (the record's begun_ms is the publish's first save)", [(EN,
        "        let begun_ms = self.clock.now_ms();\n", "        let begun_ms = 0;\n")]),
    # The pace (f6ef194).
    ("Q1 four quick syncs (<= for <)", [(PL, "        let pause = if assists < ASSIST_QUICK {\n", "        let pause = if assists <= ASSIST_QUICK {\n")]),
    ("Q2 the syncs' count not started again at an acceptance", [(EN,
        "                memory.accepted = Some((now, mono));\n                memory.assists = 0;\n",
        "                memory.accepted = Some((now, mono));\n")]),
    ("Q3 the syncs not counted", [(EN, "                memory.assists = memory.assists.saturating_add(1);\n", "")]),
    ("Q4 every sync a pause apart (no spacing)", [(PL,
        "        let pause = if assists < ASSIST_QUICK {\n            ASSIST_PAUSE\n        } else {\n            ASSIST_AFTER\n        };\n",
        "        let _ = assists;\n        let pause = ASSIST_PAUSE;\n")]),
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
