"""Plants of the third look at #149 (M2.6c-3), head 6d1841a: those of the
next look (head 0ad7944) re-run (B*, L1*, ST*, D2*, L4*, S*), N5's fix
removed (N5*), the merge of #147's head (H*: the planner's `Held`, the
matrices' oracle on both sides), and fix probes (F*, where SURVIVED
means the fix breaks no test). Each is
applied alone, as text, to a clean worktree of the head; its suites run
with a 600 s limit each; then the files are restored (try/finally).

  KILLED       a suite failed (a test caught the plant)
  SURVIVED     every suite passed
  NOT APPLIED  the text to replace was not found exactly once
  TIMEOUT      a suite ran past 600 s (counted as caught, and said)

Suites: LIB = `cargo test -p skyzfs-agent --lib`; BIN = `cargo test -p
skyzfs-agent-cli`; BINH = the same with `--features test-hooks`.

WORKTREE=<path> CARGO_TARGET_DIR=<path> python3 -I plants149t.py [ids...]
"""
import os, subprocess, sys, time

W = os.environ["WORKTREE"]
EN = "crates/skyzfs-agent/src/engine.rs"
ET = "crates/skyzfs-agent/src/engine/tests.rs"
PL = "crates/skyzfs-agent/src/plan.rs"
PR = "crates/skyzfs-agent/src/program.rs"
RE = "crates/skyzfs-agent/src/record.rs"
MAIN = "bins/skyzfs-agent/src/main.rs"

LIB = ["cargo", "test", "-p", "skyzfs-agent", "--lib"]
BIN = ["cargo", "test", "-p", "skyzfs-agent-cli"]
BINH = ["cargo", "test", "-p", "skyzfs-agent-cli", "--features", "test-hooks"]

SILENT = "Reply::Refused(Code::Ended | Code::NoAnswer | Code::NoUpload, _)"
HERE_ARM = """        DaemonSeen::Here { answer, .. } => {
            !seen.withdrawn
                && matches!(
                    answer,
                    Reply::Refused(Code::Ended | Code::NoAnswer | Code::NoUpload, _)
                )
        }
"""
ASKING = """        self.commit(Effect::Asking, |r| {
            set(r, &name, |p| {
                p.count = count;
                p.phase = Phase::Asked {
                    taken: taken.clone(),
                    asked_ms: sent,
                    held_until_ms: asking_until,
                };
            })
        })?;
"""
ASKING_UNTIL = """        let asking_until = plan::held_until(
            sent.saturating_add(ms(skyzfs_control::ASK_LIMIT)),
            self.expiry,
        );
"""
MAY_PEND = "Reply::Refused(code, _) => matches!(code, Code::Asked | Code::Ended | Code::NoAnswer),"
NO_ANSWER = """            // No answer: the request may have landed, and the publish
            // stays recorded asked.
            Err(_) => Ok(Moved::No),
"""
SWEEP_CHECK = "        if !stale.is_empty() && !self.keep.marked().map_err(Error::Io)? {\n"
BEGIN_SWEEP = "        // Before anything is written in the directory ([`Agent::sweep`]).\n        self.sweep()?;\n"
SAVE_MARK = """        if !self.marked()? {
            replace(&self.fs, &self.dir, MARKER, &marker_bytes())?;
        }
        replace(&self.fs, &self.dir, RECORD, &bytes)?;
"""
ACCEPTED_AT_ASKING = "        self.memory.entry(name.clone()).or_default().accepted = Some((sent, sent_mono));\n"
COUNT_HELD = """                p.stalls = stalls;
                p.phase = Phase::Held(recorded);
            })
        })?;
"""

P = [
    # B1's fix (plan.rs, given_up).
    ("B1a", "silent: only the daemon away (the fix reverted)",
     [(PL, HERE_ARM, "        DaemonSeen::Here { .. } => false,\n")], [LIB]),
    ("B1b", "silent: no-answer not counted", [(PL, SILENT, "Reply::Refused(Code::Ended | Code::NoUpload, _)")], [LIB]),
    ("B1c", "silent: ended not counted", [(PL, SILENT, "Reply::Refused(Code::NoAnswer | Code::NoUpload, _)")], [LIB]),
    ("B1d", "silent: no-upload not counted", [(PL, SILENT, "Reply::Refused(Code::Ended | Code::NoAnswer, _)")], [LIB]),
    ("B1e", "silent: busy counted too",
     [(PL, SILENT, "Reply::Refused(Code::Ended | Code::NoAnswer | Code::NoUpload | Code::Busy, _)")], [LIB]),
    ("B1f", "silent: the withdraw's guard dropped",
     [(PL, "            !seen.withdrawn\n                && matches!(", "            matches!(")], [LIB]),
    ("B1g", "silent: waits past held_until",
     [(PL, "if silent && seen.now_ms < *held_until_ms =>", "if silent && seen.now_ms < held_until_ms.saturating_add(3_600_000) =>")], [LIB]),
    ("B1h", "silent: a publish held waits too (its hold kept while away)",
     [(PL, "        (Phase::Intent | Phase::Taken(_), _) | (_, DaemonSeen::Away) => {\n            return give_up(pool, Why::Person)\n        }\n",
       "        (Phase::Held(_), _) if silent => return Action::Wait,\n        (Phase::Intent | Phase::Taken(_), _) | (_, DaemonSeen::Away) => {\n            return give_up(pool, Why::Person)\n        }\n")], [LIB]),
    # L1 (engine.rs, ask).
    ("L1a", "the ask recorded only after the send", [(EN, ASKING, "")], [LIB]),
    ("L1b", "asking held_until without the channel's wait",
     [(EN, "            sent.saturating_add(ms(skyzfs_control::ASK_LIMIT)),\n", "            sent,\n")], [LIB]),
    ("L1c", "asking held_until = the send (no expiry)",
     [(EN, ASKING_UNTIL, "        let asking_until = sent;\n")], [LIB]),
    ("L1d", "never recorded held again", [(EN, "                if !may_pend(&answer) {\n", "                if false && !may_pend(&answer) {\n")], [LIB]),
    ("L1e", "held again on every refusal", [(EN, "                if !may_pend(&answer) {\n", "                if true {\n")], [LIB]),
    ("L1f", "may_pend: no-upload read as pending",
     [(EN, MAY_PEND, "Reply::Refused(code, _) => matches!(code, Code::Asked | Code::Ended | Code::NoAnswer | Code::NoUpload),")], [LIB]),
    ("L1g", "may_pend: no-answer read as nothing pending",
     [(EN, MAY_PEND, "Reply::Refused(code, _) => matches!(code, Code::Asked | Code::Ended),")], [LIB]),
    ("L1h", "may_pend: ended read as nothing pending",
     [(EN, MAY_PEND, "Reply::Refused(code, _) => matches!(code, Code::Asked | Code::NoAnswer),")], [LIB]),
    ("L1i", "may_pend: asked read as nothing pending",
     [(EN, MAY_PEND, "Reply::Refused(code, _) => matches!(code, Code::Ended | Code::NoAnswer),")], [LIB]),
    ("L1j", "no answer at all recorded held again",
     [(EN, NO_ANSWER, "            Err(_) => {\n                self.commit(Effect::Unasked, |r| set(r, &name, |p| p.phase = Phase::Held(taken)))?;\n                Ok(Moved::No)\n            }\n")], [LIB]),
    ("ST1", "the author's stall mutant: the count's stalls committed with the publish left asked",
     [(EN, COUNT_HELD, "                p.stalls = stalls;\n            })\n        })?;\n")], [LIB]),
    ("ST2", "an ask anew from a record of `asking` (never accepted) counts two stalls",
     [(EN, "        let stalls = plan::stalls(publish, &count);\n",
       "        let stalls = plan::stalls(publish, &count) + u32::from(matches!(publish.phase, Phase::Asked { asked_ms, held_until_ms, .. } if held_until_ms.saturating_sub(asked_ms) > 650_000));\n")], [LIB]),
    # D2 (engine.rs, program.rs).
    ("D2a", "execute: the exit looked up by the name begun", [(PR, "            let mine = agent.name_now(&mine);\n", "")], [LIB]),
    ("D2b", "name_now: one rename followed", [(EN, "        while let Some(next) = self.renamed.get(now) {\n", "        if let Some(next) = self.renamed.get(now) {\n")], [LIB]),
    ("D2c", "fresh_name: a name left chosen again", [(EN, "            if recorded || self.renamed.contains_key(&name) {\n", "            if recorded {\n")], [LIB]),
    ("D2d", "the rename not remembered", [(EN, "                self.renamed.insert(name.to_string(), next);\n", "                drop(next);\n")], [LIB]),
    # L4 (engine.rs, record.rs, program.rs).
    ("L4a", "sweep: the marker not checked", [(EN, SWEEP_CHECK, "        if false {\n")], [LIB, BINH]),
    ("L4b", "sweep: refused with nothing to release", [(EN, SWEEP_CHECK, "        if !self.keep.marked().map_err(Error::Io)? {\n")], [LIB]),
    ("L4c", "begin: no sweep first", [(EN, BEGIN_SWEEP, "")], [LIB]),
    ("L4d", "save: the marker not written", [(RE, SAVE_MARK, "        replace(&self.fs, &self.dir, RECORD, &bytes)?;\n")], [LIB, BIN, BINH]),
    ("L4e", "save: the marker after the record",
     [(RE, SAVE_MARK, "        replace(&self.fs, &self.dir, RECORD, &bytes)?;\n        if !self.marked()? {\n            replace(&self.fs, &self.dir, MARKER, &marker_bytes())?;\n        }\n")], [LIB]),
    ("L4f", "marked: any file of the name", [(RE, "            Ok(bytes) => Ok(bytes == marker_bytes()),\n", "            Ok(_) => Ok(true),\n")], [LIB]),
    ("L4g", "Unmarked exits 1", [(PR, "        Error::Corrupt(_) | Error::Volume(_) | Error::Unmarked(_) => exit::RECORD,\n",
                                   "        Error::Corrupt(_) | Error::Volume(_) => exit::RECORD,\n        Error::Unmarked(_) => exit::ERROR,\n")], [LIB, BINH]),
    # The command line (1cf1428).
    ("S2a", "record of a missing directory prints absent",
     [(MAIN, "        Err(e) => {\n            eprintln!(\"skyzfs-agent: {}: {e}\", dir.display());\n            return exit::ERROR;\n        }\n", "        Err(_) => {}\n")], [BIN]),
    ("S3a", "a value that begins with -- taken as the value",
     [(PR, "                Some(value) if value.starts_with(\"--\") => {\n", "                Some(value) if value.starts_with(\"--\") && false => {\n")], [LIB]),
    ("S4a", "print! on a closed pipe",
     [(MAIN, "    let mut out = io::stdout().lock();\n    let _ = out.write_all(text.as_bytes());\n    let _ = out.flush();\n", "    print!(\"{text}\");\n")], [BIN]),
    ("S6a", "--help after a command not taken",
     [(PR, "        if matches!(word.as_str(), \"--help\" | \"-h\") {\n", "        if false {\n")], [LIB]),
    # Fix probes.
    # N5's fix (9d1e80a).
    ("N5a", "the reading of the clock that does not step not taken at the ask (the fix removed)",
     [(EN, ACCEPTED_AT_ASKING, "")], [LIB]),
    ("N5b", "taken at the ask, from the wall clock's reading after the send (asked_ms no longer matches)",
     [(EN, ACCEPTED_AT_ASKING, "        self.memory.entry(name.clone()).or_default().accepted = Some((sent + 1, sent_mono));\n")], [LIB]),
    # The merge of #147's head (03793ac): the planner's `Held`.
    ("H1", "report: HeldAway only where every member is held",
     [(PL, "                && seen.pool.hold != Held::No\n", "                && seen.pool.hold == Held::All\n")], [LIB]),
    ("H2", "give_up: a hold on some members abandoned unreleased",
     [(PL, "fn give_up(pool: &PoolSeen, why: Why) -> Action {\n    if pool.hold != Held::No {", "fn give_up(pool: &PoolSeen, why: Why) -> Action {\n    if pool.hold == Held::All {")], [LIB]),
    ("H3", "a publish done: a hold on some members not released",
     [(PL, "        Phase::Done { .. } if pool.hold != Held::No => Action::Release,", "        Phase::Done { .. } if pool.hold == Held::All => Action::Release,")], [LIB]),
    ("H4", "away: a hold on some members never released when the daemon is away",
     [(PL, "        Some((_, until)) if seen.now_ms >= until && pool.hold != Held::No => Action::Release,", "        Some((_, until)) if seen.now_ms >= until && pool.hold == Held::All => Action::Release,")], [LIB]),
    # The matrices' oracle, each side's exception removed (test code: KILLED means it is needed).
    ("O1", "oracle: #147's partly-released refusal report not accepted",
     [(ET, "            Report::Refused(_) if world.log.contains(&format!(\"partly released {t}\")) => {}\n", "")], [LIB]),
    ("O2", "oracle: #147's too-old give-up never confirmed",
     [(ET, "            Why::TooOld => world.log.contains(&format!(\"partly released {t}\")),\n", "            Why::TooOld => false,\n")], [LIB]),
    # Fix probes.
    ("F2", "fix probe: memory.withdrawn cleared at the asking commit (N6)",
     [(EN, ACCEPTED_AT_ASKING, "        {\n            let memory = self.memory.entry(name.clone()).or_default();\n            memory.accepted = Some((sent, sent_mono));\n            memory.withdrawn = false;\n        }\n")], [LIB]),
]


def run(cmd):
    t = time.time()
    try:
        r = subprocess.run(cmd, cwd=W, capture_output=True, text=True, timeout=600)
    except subprocess.TimeoutExpired:
        return "TIMEOUT", time.time() - t, ""
    out = r.stdout + r.stderr
    failed = [l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.rstrip().endswith("FAILED")]
    if r.returncode == 0:
        return "pass", time.time() - t, ""
    if "error[E" in out or "error: could not compile" in out:
        return "fail (does not build)", time.time() - t, ""
    return "fail", time.time() - t, " ".join(failed)


want = sys.argv[1:]
for pid, what, edits, suites in P:
    if want and pid not in want:
        continue
    files = sorted({f for f, _, _ in edits})
    saved = {f: open(os.path.join(W, f)).read() for f in files}
    texts = dict(saved)
    bad = None
    for f, old, new in edits:
        n = texts[f].count(old)
        if n != 1:
            bad = f"NOT APPLIED ({f}: found {n})"
            break
        texts[f] = texts[f].replace(old, new)
    if bad:
        print(f"{pid} {what}: {bad}", flush=True)
        continue
    try:
        for f in files:
            open(os.path.join(W, f), "w").write(texts[f])
        results = []
        for cmd in suites:
            name = {tuple(LIB): "LIB", tuple(BIN): "BIN", tuple(BINH): "BINH"}[tuple(cmd)]
            verdict, secs, failed = run(cmd)
            results.append((name, verdict, secs, failed))
    finally:
        for f in files:
            open(os.path.join(W, f), "w").write(saved[f])
    caught = any(v != "pass" for _, v, _, _ in results)
    timeout = any(v == "TIMEOUT" for _, v, _, _ in results)
    verdict = "TIMEOUT (caught)" if timeout else ("KILLED" if caught else "SURVIVED")
    detail = "; ".join(f"{n}: {v} {s:.0f}s {fl}".rstrip() for n, v, s, fl in results)
    print(f"{pid} {what}: {verdict} [{detail}]", flush=True)
