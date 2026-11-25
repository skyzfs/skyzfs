"""Plants of the review of #149 (M2.6c-3) at af50814: a sample of the
author's 32 (A*), re-made from the PR's table, and the reviewer's own
(M*, on the binary and on the readings the review questions). Each is
applied alone, as text, to a clean worktree of the head; its suites run
with a 600 s limit each; then the files are restored (try/finally).

  KILLED       a suite failed (a test caught the plant)
  SURVIVED     every suite passed
  NOT APPLIED  the text to replace was not found exactly once
  TIMEOUT      a suite ran past 600 s (counted as caught, and said)

Suites: LIB = `cargo test -p skyzfs-agent --lib`; BIN = `cargo test -p
skyzfs-agent-cli`; BINH = the same with `--features test-hooks` (which
CI does not run). A plant on the library runs LIB; one on the binary
runs BIN and BINH.

WORKTREE=<path> CARGO_TARGET_DIR=<path> python3 -I plants149.py [ids...]
"""
import os, subprocess, sys, time

W = os.environ["WORKTREE"]
EN = "crates/skyzfs-agent/src/engine.rs"
PL = "crates/skyzfs-agent/src/plan.rs"
PR = "crates/skyzfs-agent/src/program.rs"
RE = "crates/skyzfs-agent/src/record.rs"
MAIN = "bins/skyzfs-agent/src/main.rs"

LIB = ["cargo", "test", "-p", "skyzfs-agent", "--lib"]
BIN = ["cargo", "test", "-p", "skyzfs-agent-cli"]
BINH = ["cargo", "test", "-p", "skyzfs-agent-cli", "--features", "test-hooks"]

STOP_COMMIT = """                        if publish.person != Some(Person::Stopped) {
                            self.commit(Effect::Stop, |r| {
                                set(r, &name, |p| p.person = Some(Person::Stopped))
                            })?;
                        }
"""
STEP_STOP = """        if publish.person == Some(Person::Stopped) {
            // Stopped for a person in an earlier round or process: nothing
            // is asked of the pool or the daemon for it.
            self.stop(name.to_string(), STOPPED_BEFORE.to_string());
            return Ok(Moved::No);
        }
"""
MAY_PEND = """    let may_pend = matches!(
        answer,
        Reply::Pending | Reply::Accepted | Reply::Refused(Code::Asked, _)
    );
"""
GIVEN_AWAY = """        (Phase::Asked { held_until_ms, .. }, DaemonSeen::Away) if seen.now_ms < *held_until_ms => {
            return Action::Wait
        }
"""

P = [
    # The author's, re-made.
    ("A1", "stop not recorded", [(EN, STOP_COMMIT, "")], [LIB]),
    ("A2", "recorded stop not checked in the round", [(EN, STEP_STOP, "")], [LIB]),
    ("A3", "plan: a recorded stop planned as the agent's own",
     [(PL, "        (Some(Person::Stopped), _) => return Action::Stop(Stop::DoneNotSeen),\n",
       "        (Some(Person::Stopped), _) => {}\n")], [LIB]),
    ("A4", "give-up: no wait for held_until while away", [(PL, GIVEN_AWAY, "")], [LIB]),
    ("A5", "give-up: no withdraw",
     [(PL, "    if may_pend && !seen.withdrawn {\n        return Action::Withdraw;\n    }\n", "")], [LIB]),
    ("A6", "give-up: a publish made given up",
     [(PL, """        Some(SnapSeen::Ours(commit)) => Action::Done {
            commit,
            report: None,
        },
        Some(SnapSeen::Absent | SnapSeen::NotOurs | SnapSeen::Unreadable) => {
            give_up(pool, Why::Person)
        }""", """        Some(SnapSeen::Ours(_) | SnapSeen::Absent | SnapSeen::NotOurs | SnapSeen::Unreadable) => {
            give_up(pool, Why::Person)
        }""")], [LIB]),
    ("A7", "worst: abandoned over stopped",
     [(PR, """    } else if any(exit::STOPPED) {
        exit::STOPPED
    } else if any(exit::ABANDONED) {
        exit::ABANDONED
    }""", """    } else if any(exit::ABANDONED) {
        exit::ABANDONED
    } else if any(exit::STOPPED) {
        exit::STOPPED
    }""")], [LIB]),
    ("A8", "publish exits by the worst, not its own",
     [(PR, "        Some(mine) => exit_of(\n", "        Some(_) if true => worst(agent.outcomes(), !unsettled.is_empty()),\n        Some(mine) => exit_of(\n")], [LIB]),
    ("A9", "hooks allowed without the feature",
     [(PR, "        if !self.hooks && self.options.iter().any(|(k, _)| k == key) {\n",
       "        if false && !self.hooks && self.options.iter().any(|(k, _)| k == key) {\n")], [LIB]),
    ("A10", "trace written after the kill",
     [(PR, """        if let Some(out) = &mut trace {
            let line = trace_line(now_ns(), point, name);
            if let Err(e) = out.write_all(line.as_bytes()).and_then(|()| out.flush()) {
                let _ = writeln!(err, "skyzfs-agent: --trace-points: {e}; stopped");
                return true;
            }
        }
        if let Some(at) = kill_at {
            if at.point == point {
                reached += 1;
                if reached == at.nth {
                    die(&kill_line(&at));
                }
            }
        }
""", """        if let Some(at) = kill_at {
            if at.point == point {
                reached += 1;
                if reached == at.nth {
                    die(&kill_line(&at));
                }
            }
        }
        if let Some(out) = &mut trace {
            let line = trace_line(now_ns(), point, name);
            if let Err(e) = out.write_all(line.as_bytes()).and_then(|()| out.flush()) {
                let _ = writeln!(err, "skyzfs-agent: --trace-points: {e}; stopped");
                return true;
            }
        }
""")], [LIB]),
    # The reviewer's: the binary.
    ("M1", "bin: another agent's lock exits 1, not 8",
     [(MAIN, "                OpenError::Held(_) => exit::HELD,\n", "                OpenError::Held(_) => exit::ERROR,\n")], [BIN, BINH]),
    ("M2", "bin: the run's expiry and hooks not applied (configured skipped)",
     [(MAIN, "    let mut agent = program::configured(agent, run).with_sink(", "    let mut agent = agent.with_sink(")], [BIN, BINH]),
    ("M3", "bin: the points never attached (--kill-at and --trace-points ignored)",
     [(MAIN, "        Ok(Some(points)) => agent = agent.with_points(points),\n", "        Ok(Some(_points)) => {}\n")], [BIN, BINH]),
    ("M4", "bin: reports written to standard output",
     [(MAIN, "        let mut err = io::stderr().lock();\n        let _ = err.write_all(program::report_line(name, report).as_bytes());",
       "        let mut err = io::stdout().lock();\n        let _ = err.write_all(program::report_line(name, report).as_bytes());")], [BIN, BINH]),
    ("M5", "bin: reports not written at all",
     [(MAIN, "        let _ = err.write_all(program::report_line(name, report).as_bytes());",
       "        let _ = (name, report, &mut err);")], [BIN, BINH]),
    ("M6", "bin: an unreadable record (I/O) exits 6, not 1",
     [(MAIN, """        Err(e) => {
            eprintln!("skyzfs-agent: {}: {e}", dir.display());
            exit::ERROR
        }""", """        Err(e) => {
            eprintln!("skyzfs-agent: {}: {e}", dir.display());
            exit::RECORD
        }""")], [BIN, BINH]),
    ("M7", "bin: the hooks always taken (cfg! read as true)",
     [(MAIN, "program::parse(&args, cfg!(feature = \"test-hooks\"))", "program::parse(&args, true)")], [BIN, BINH]),
    ("M8", "bin: the volume's id not read (zeros)",
     [(MAIN, "            Ok(volume) => volume,\n", "            Ok(_volume) => [0u8; 16],\n")], [BIN, BINH]),
    # The reviewer's: the library.
    ("M9", "settle: stopped ranked above unsettled",
     [(PR, """    if unsettled {
        exit::UNSETTLED
    } else if any(exit::STOPPED) {
        exit::STOPPED
    }""", """    if any(exit::STOPPED) {
        exit::STOPPED
    } else if unsettled {
        exit::UNSETTLED
    }""")], [LIB]),
    ("M10", "another volume's record exits 1, not 6",
     [(PR, "        Error::Corrupt(_) | Error::Volume(_) => exit::RECORD,\n", "        Error::Corrupt(_) => exit::RECORD,\n        Error::Volume(_) => exit::ERROR,\n")], [LIB]),
    ("M11", "dump: the person's words swapped",
     [(PR, "            Some(Person::Stopped) => \"stopped\",\n            Some(Person::GiveUp) => \"give-up\",\n",
       "            Some(Person::Stopped) => \"give-up\",\n            Some(Person::GiveUp) => \"stopped\",\n")], [LIB]),
    ("M12", "unsettled names not printed",
     [(PR, "        Err(Error::Unsettled(names)) => names.clone(),\n", "        Err(Error::Unsettled(_)) => vec![String::new()],\n")], [LIB]),
    ("M13", "a repeated option taken (the last wins)",
     [(PR, "        if options.iter().any(|(k, _)| *k == key) {\n            return usage(format!(\"--{key} is given twice\"));\n        }\n",
       "        options.retain(|(k, _)| *k != key);\n")], [LIB]),
    ("M14", "give-up: the hold released at once for a publish asked, daemon away (no held_until)",
     [(PL, GIVEN_AWAY, "")], [LIB]),
    ("M15", "give-up: held and not asked, daemon away, waits (the conservative reading)",
     [(PL, "        (Phase::Intent | Phase::Taken(_), _) | (_, DaemonSeen::Away) => {\n",
       "        (Phase::Held(_), DaemonSeen::Away) => return Action::Wait,\n        (Phase::Intent | Phase::Taken(_), _) | (_, DaemonSeen::Away) => {\n")], [LIB]),
    ("M16", "FIX probe: a silent uploader (no-answer) read as a request that may pend",
     [(PL, MAY_PEND, MAY_PEND.replace("Reply::Refused(Code::Asked, _)", "Reply::Refused(Code::Asked | Code::NoAnswer, _)"))], [LIB]),
    ("M17", "FIX probe: no-answer, ended, no-upload read as a daemon away",
     [(PL, "        (_, DaemonSeen::Here { answer, .. }) => answer,\n",
       "        (Phase::Asked { held_until_ms, .. }, DaemonSeen::Here { answer: Reply::Refused(Code::NoAnswer | Code::Ended | Code::NoUpload, _), .. }) if seen.now_ms < *held_until_ms => return Action::Wait,\n        (_, DaemonSeen::Here { answer, .. }) => answer,\n")], [LIB]),
    ("M18", "FIX probe: publish exits by the worst where its name moved",
     [(PR, "                .map(|(_, outcome)| outcome),\n        ),\n",
       "                .map(|(_, outcome)| outcome),\n        )\n        .min(worst(agent.outcomes(), !unsettled.is_empty())),\n")], [LIB]),
    ("M19", "recover --give-up recorded as a stop",
     [(EN, "            Choice::GiveUp => Some(Person::GiveUp),\n", "            Choice::GiveUp => Some(Person::Stopped),\n")], [LIB]),
    ("M20", "the record not peeked: a missing record read as an error",
     [(RE, "        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Loaded::Absent),\n        Err(e) => return Err(e),\n    };\n    Ok(match Record::decode(&bytes) {",
       "        Err(e) => return Err(e),\n    };\n    Ok(match Record::decode(&bytes) {")], [LIB, BIN]),
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
