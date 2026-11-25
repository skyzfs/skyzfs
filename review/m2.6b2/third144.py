"""Textual plants for the third look at #144 (M2.6b2), at 1b5e5f9, one at
a time, each file restored from its saved text in `finally`, 600 s each.
A plant is a list of edits (file, old, new); each old text must occur
once, or the plant is NOT APPLIED. Run from a clean worktree of the
head: WORKTREE=<path> OUT=<json> python3 third144.py [name prefixes...]

Earlier plants that still apply, re-anchored where the code moved:
F* (F1, TG2), T2* and T3* (D5 and Disk), A* (the age bound; A4', A6' and
A14' re-worded for 5faaabf's anchor), Q* (the pace), G* (the fake's
assist, TG5).
New: R* the "ended" arm of `onward` (N1 as rebuilt, L5 and L6); E* the
engine's `withdrawn` flag (E2 and E3 are the author's survivors); N2*
the rename; S2* the anchor after a restart; TG4a the take-over."""
import json, os, subprocess, sys, time

W = os.environ.get("WORKTREE", ".")
OUT = os.environ.get("OUT", "third144.json")
ENV = dict(os.environ, CARGO_INCREMENTAL="0")
SUITE = ["cargo", "test", "-p", "skyzfs-agent", "--lib", "--", "--skip", "review"]
PL = "crates/skyzfs-agent/src/plan.rs"
EN = "crates/skyzfs-agent/src/engine.rs"
FS = "crates/skyzfs-agent/src/fs.rs"
FK = "crates/skyzfs-agent/src/engine/fake.rs"
VALUE_MAX = (EN, "                let value = self.clock.now_ms().max(self.assists.saturating_add(1));\n",
             "                let value = self.clock.now_ms().max(self.assists);\n")

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
ARM_DONE = "            Reply::Done(commit) => said_done(commit),\n            Reply::Refused(Code::Taken, _) => look(&away),\n"
ARM_WITHDRAW = "            Reply::Pending | Reply::Absent | Reply::Refused(Code::Asked, _) => Action::Withdraw,\n"
ARM_FLAG = ("            Reply::Withdrawn | Reply::Unknown | Reply::Expired if seen.withdrawn => {\n"
            "                look(&|| give_up(pool, broken.unwrap_or(why)))\n"
            "            }\n")
ARM_FIRST = "            Reply::Withdrawn | Reply::Unknown | Reply::Expired => Action::Withdraw,\n            _ => away(),\n        };\n"
GIVE_UP = "look(&|| give_up(pool, broken.unwrap_or(why)))"
FLAG_SET = ("                        if matches!(answer, Reply::Withdrawn | Reply::Unknown | Reply::Expired) {\n"
            "                            memory.withdrawn = true;\n"
            "                        }\n")
AGE_ARM = ("            Some((begun, at, then)) if begun == publish.begun_ms => (at, then),\n"
           "            _ => {\n"
           "                let at = self.clock.now_ms();\n"
           "                memory.begun = Some((publish.begun_ms, at, mono));\n"
           "                (at, mono)\n"
           "            }\n")

P = [
    # F1 (28d1d64) and TG2.
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
    # The age bound (45648ab, 5faaabf).
    ("A1 never too old", [(PL, "    now_ms.saturating_sub(begun_ms) >= age_max\n", "    let _ = (now_ms, begun_ms, age_max);\n    false\n")]),
    ("A2 too old one ms late (> for >=)", [(PL, "    now_ms.saturating_sub(begun_ms) >= age_max\n", "    now_ms.saturating_sub(begun_ms) > age_max\n")]),
    ("A3 a clock stepped back read as old (TG3)", [(PL, "    now_ms.saturating_sub(begun_ms) >= age_max\n", "    now_ms.abs_diff(begun_ms) >= age_max\n")]),
    ("A4' the age on the wall clock in the process that began it", [(EN,
        "            Some((begun, at, then)) if begun == publish.begun_ms => (at, then),\n",
        "            Some((begun, _, _)) if begun == publish.begun_ms => (self.clock.now_ms(), mono),\n")]),
    ("A5 the age measured from the planner's time (now_for)", [(EN,
        "        let too_old = plan::too_old(publish.begun_ms, self.age_now(publish), self.age_max);\n",
        "        let too_old = plan::too_old(publish.begun_ms, now_ms, self.age_max);\n")]),
    ("A6' begun not remembered in begin (anchored at the first round instead)", [(EN,
        "        self.memory.entry(name.clone()).or_default().begun = Some((begun_ms, begun_ms, mono));\n",
        "        let _ = mono;\n")]),
    ("A7 the age bound at Intent only with the daemon here", [(PL,
        "            (None, _) if seen.too_old => Action::Abandon(Why::TooOld),\n",
        "            (None, DaemonSeen::Here { .. }) if seen.too_old => Action::Abandon(Why::TooOld),\n")]),
    ("A8 the age bound not applied at Taken", [(PL, "            if seen.too_old {\n                return give_up(pool, Why::TooOld);\n            }\n", "")]),
    ("A9 at Taken, too old before broken", [(PL,
        "            if let Some(why) = broken(pool, taken) {\n                return give_up(pool, why);\n            }\n            if seen.too_old {\n                return give_up(pool, Why::TooOld);\n            }\n",
        "            if seen.too_old {\n                return give_up(pool, Why::TooOld);\n            }\n            if let Some(why) = broken(pool, taken) {\n                return give_up(pool, why);\n            }\n")]),
    ("A12 too old over eight stalls", [(PL, ENDED,
        "    let ended = if seen.too_old {\n        Some(Why::TooOld)\n    } else if publish.stalls >= STALLS_ABANDONED {\n        Some(Why::NoProgress)\n    } else {\n        None\n    };\n")]),
    ("A13 too old given up while the daemon is away (no answer at all)", [(PL,
        "    let (cuts, answer) = match &seen.daemon {\n        DaemonSeen::Away => return away(),\n",
        "    let (cuts, answer) = match &seen.daemon {\n        DaemonSeen::Away if seen.too_old => return give_up(pool, Why::TooOld),\n        DaemonSeen::Away => return away(),\n")]),
    ("A14' too old: its own reason over the snapshot's", [(PL, ARM_FLAG,
        ARM_FLAG.replace("broken.unwrap_or(why)", "broken.map_or(why, |_| why)"))]),
    ("A15 begun_ms 0 (an age read from the epoch)", [(EN,
        "        let begun_ms = self.clock.now_ms();\n", "        let begun_ms = 0;\n")]),
    # R: the ended arm (N1 rebuilt to L5, N3 to L6).
    ("R1 any other answer: looked up, then given up (the arm at 6b05991)", [(PL, ARM_FIRST,
        "            Reply::Withdrawn | Reply::Unknown | Reply::Expired => Action::Withdraw,\n            _ => " + GIVE_UP + ",\n        };\n")]),
    ("R2 no-answer alone: looked up, then given up", [(PL, ARM_FIRST,
        "            Reply::Withdrawn | Reply::Unknown | Reply::Expired => Action::Withdraw,\n"
        "            Reply::Refused(Code::NoAnswer, _) => " + GIVE_UP + ",\n            _ => away(),\n        };\n")]),
    ("R3 stopped alone: looked up, then given up", [(PL, ARM_FIRST,
        "            Reply::Withdrawn | Reply::Unknown | Reply::Expired => Action::Withdraw,\n"
        "            Reply::Refused(Code::Stopped, _) => " + GIVE_UP + ",\n            _ => away(),\n        };\n")]),
    ("R4 any other answer: the hold of a publish held released at once", [(PL, ARM_FIRST,
        "            Reply::Withdrawn | Reply::Unknown | Reply::Expired => Action::Withdraw,\n"
        "            _ if asked.is_none() && pool.held => Action::Release,\n            _ => away(),\n        };\n")]),
    ("R5 refused asked kept as a daemon away (not withdrawn)", [(PL, ARM_WITHDRAW,
        "            Reply::Pending | Reply::Absent => Action::Withdraw,\n")]),
    ("R6 the withdraw's answer not required (given up on withdrawn/unknown/expired)", [(PL, ARM_FLAG,
        ARM_FLAG.replace(" if seen.withdrawn", ""))]),
    ("R7 after the withdraw, given up without a look", [(PL, ARM_FLAG,
        ARM_FLAG.replace(GIVE_UP, "give_up(pool, broken.unwrap_or(why))"))]),
    ("R8 done not confirmed: given up after a look (L6 off)", [(PL, ARM_DONE,
        "            Reply::Done(_) => " + GIVE_UP + ",\n            Reply::Refused(Code::Taken, _) => look(&away),\n")]),
    ("R9 refused taken: withdrawn rather than looked up", [(PL, ARM_DONE,
        "            Reply::Done(commit) => said_done(commit),\n            Reply::Refused(Code::Taken, _) => Action::Withdraw,\n")]),
    ("R10 eight stalls not in the arm (only too old)", [(PL, ENDED,
        "    let ended = if seen.too_old {\n        Some(Why::TooOld)\n    } else {\n        None\n    };\n")]),
    ("R11 pending kept, not withdrawn", [(PL, ARM_WITHDRAW,
        "            Reply::Absent | Reply::Refused(Code::Asked, _) => Action::Withdraw,\n")]),
    # E: the engine's withdrawn flag.
    ("E1 the withdraw's answer never remembered", [(EN, FLAG_SET, "")]),
    ("E2 a withdraw's answer remembered whatever it is (the author's E2)", [(EN, FLAG_SET,
        "                        memory.withdrawn = true;\n")]),
    ("E3 the flag not forgotten when a request is accepted (the author's E3)", [(EN,
        "                memory.assists = 0;\n                memory.withdrawn = false;\n",
        "                memory.assists = 0;\n")]),
    ("E4 the flag set on withdrawn alone (not unknown, expired)", [(EN, FLAG_SET,
        FLAG_SET.replace("Reply::Withdrawn | Reply::Unknown | Reply::Expired", "Reply::Withdrawn"))]),
    ("E5 the flag set on withdrawn and expired (not unknown)", [(EN, FLAG_SET,
        FLAG_SET.replace("Reply::Withdrawn | Reply::Unknown | Reply::Expired", "Reply::Withdrawn | Reply::Expired"))]),
    # N2: the rename.
    ("N2a the memory left under the old name at the rename", [(EN,
        "                if let Some(memory) = self.memory.remove(name) {\n                    self.memory.insert(next, memory);\n                }\n",
        "                let _ = next;\n")]),
    # S2: the anchor after a restart.
    ("S2a no anchor after a restart: the wall clock at each round", [(EN, AGE_ARM,
        "            Some((begun, at, then)) if begun == publish.begun_ms => (at, then),\n"
        "            _ => (self.clock.now_ms(), mono),\n")]),
    ("S2b after a restart, anchored at begun_ms on the clock that does not step (the age restarts)", [(EN, AGE_ARM,
        AGE_ARM.replace("                let at = self.clock.now_ms();\n", "                let at = publish.begun_ms;\n"))]),
    # TG4: the take-over.
    ("TG4a unknown after an acceptance asked again with the recorded count (no stall counted)", [(PL,
        "            _ if broken.is_some() => look(&|| anew(false)),\n            _ => anew(false),\n",
        "            _ if broken.is_some() => look(&|| anew(false)),\n            _ if pool.held && !moved => Action::Ask,\n            _ => anew(false),\n")]),
    # The pace (f6ef194).
    ("Q1 four quick syncs (<= for <)", [(PL, "        let pause = if assists < ASSIST_QUICK {\n", "        let pause = if assists <= ASSIST_QUICK {\n")]),
    ("Q2 the syncs' count not started again at an acceptance", [(EN,
        "                memory.accepted = Some((now, mono));\n                memory.assists = 0;\n",
        "                memory.accepted = Some((now, mono));\n")]),
    ("Q3 the syncs not counted", [(EN, "                memory.assists = memory.assists.saturating_add(1);\n", "")]),
    ("Q4 every sync a pause apart (no spacing)", [(PL,
        "        let pause = if assists < ASSIST_QUICK {\n            ASSIST_PAUSE\n        } else {\n            ASSIST_AFTER\n        };\n",
        "        let _ = assists;\n        let pause = ASSIST_PAUSE;\n")]),
    # The fake's assist (6b05991) and TG5.
    ("G1 the fake's assist writes one group (the history record's removed)", [(FK,
        "            world.sync(\"zfs set skyzfs:assist\");\n            world.sync(\"zpool sync: the set's history record\");\n",
        "            world.sync(\"zfs set skyzfs:assist\");\n")]),
    ("G2 the agent's value set again, the fake writing groups for a value it holds", [(FK,
        "                .push(format!(\"zpool sync alone (value {value} again): no group\"));\n",
        "                .push(format!(\"zpool sync alone (value {value} again): no group\"));\n            world.sync(\"zfs set skyzfs:assist (the value it holds)\");\n            world.sync(\"zpool sync: the set's history record\");\n"),
    (EN, "                let value = self.clock.now_ms().max(self.assists.saturating_add(1));\n",
         "                let value = self.assists.max(1);\n")]),
    ("G3 the agent's value set again (the control of G2)", [(EN, "                let value = self.clock.now_ms().max(self.assists.saturating_add(1));\n",
         "                let value = self.assists.max(1);\n")]),
    ("G4 the agent's value not above the last one (max without + 1)", [VALUE_MAX]),
    ("G5 G4, the fake writing groups for a value it holds", [(FK,
        "                .push(format!(\"zpool sync alone (value {value} again): no group\"));\n",
        "                .push(format!(\"zpool sync alone (value {value} again): no group\"));\n            world.sync(\"zfs set skyzfs:assist (the value it holds)\");\n            world.sync(\"zpool sync: the set's history record\");\n"),
        VALUE_MAX]),
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
