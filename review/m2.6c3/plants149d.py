"""Plants of the fourth look at #149 (M2.6c-3), head 5259bf5: N6's fix
(the clearing of `memory.withdrawn` at the asking commit) taken out or
moved (N6*), equivalents (E*, where SURVIVED is expected: the change
does not alter behaviour), N5's fix re-planted on the new text (N5a),
and the plants behind the answers on Codex's two threads (B1a, B1f for
P1; D2a for P2). Each is applied alone, as text, to a clean worktree of
the head; its suites run with a 600 s limit each; then the files are
restored (try/finally).

  KILLED       a suite failed (a test caught the plant)
  SURVIVED     every suite passed
  NOT APPLIED  the text to replace was not found exactly once
  TIMEOUT      a suite ran past 600 s (counted as caught, and said)

Suites: LIB = `cargo test -p skyzfs-agent --lib`.

WORKTREE=<path> CARGO_TARGET_DIR=<path> python3 -I plants149d.py [ids...]
"""
import os, subprocess, sys, time

W = os.environ["WORKTREE"]
EN = "crates/skyzfs-agent/src/engine.rs"
PL = "crates/skyzfs-agent/src/plan.rs"
PR = "crates/skyzfs-agent/src/program.rs"

LIB = ["cargo", "test", "-p", "skyzfs-agent", "--lib"]

AT_ASKING = """        let memory = self.memory.entry(name.clone()).or_default();
        memory.accepted = Some((sent, sent_mono));
        memory.withdrawn = false;
        self.point(Effect::Ask, false)?;
"""
AT_ACCEPTED = """                memory.assists = 0;
                memory.withdrawn = false;
                Ok(Moved::Yes)
"""
OK_ARM = """            Ok(answer) => {
                if !may_pend(&answer) {
"""
ERR_ARM = """            Err(_) => Ok(Moved::No),
        }
    }
"""
HERE_ARM = """        DaemonSeen::Here { answer, .. } => {
            !seen.withdrawn
                && matches!(
                    answer,
                    Reply::Refused(Code::Ended | Code::NoAnswer | Code::NoUpload, _)
                )
        }
"""
NO_CLEAR = """        let memory = self.memory.entry(name.clone()).or_default();
        memory.accepted = Some((sent, sent_mono));
        self.point(Effect::Ask, false)?;
"""

P = [
    ("N6a", "the clearing at the asking commit removed (the author's mutant)",
     [(EN, AT_ASKING, NO_CLEAR)], [LIB]),
    ("N6b", "cleared only where the ask has no answer at all (Err)",
     [(EN, AT_ASKING, NO_CLEAR),
      (EN, ERR_ARM, """            Err(_) => {
                self.memory.entry(name).or_default().withdrawn = false;
                Ok(Moved::No)
            }
        }
    }
""")], [LIB]),
    ("N6c", "cleared only where the ask has an answer (Ok, any)",
     [(EN, AT_ASKING, NO_CLEAR),
      (EN, OK_ARM, """            Ok(answer) => {
                self.memory.entry(name.clone()).or_default().withdrawn = false;
                if !may_pend(&answer) {
""")], [LIB]),
    ("N6d", "the clearing at the asking commit removed, and the one at acceptance too",
     [(EN, AT_ASKING, NO_CLEAR),
      (EN, AT_ACCEPTED, "                memory.assists = 0;\n                Ok(Moved::Yes)\n")], [LIB]),
    ("E1", "equivalent: the clearing at acceptance removed (the asking commit clears it already)",
     [(EN, AT_ACCEPTED, "                memory.assists = 0;\n                Ok(Moved::Yes)\n")], [LIB]),
    ("N5a", "N5's fix removed: no reading of the clock that does not step at the asking commit",
     [(EN, AT_ASKING, """        let memory = self.memory.entry(name.clone()).or_default();
        memory.withdrawn = false;
        self.point(Effect::Ask, false)?;
""")], [LIB]),
    ("B1a", "P1: silent only for the daemon away (B1's fix reverted)",
     [(PL, HERE_ARM, "        DaemonSeen::Here { .. } => false,\n")], [LIB]),
    ("B1f", "P1: silent: the withdraw's guard dropped",
     [(PL, "            !seen.withdrawn\n                && matches!(", "            matches!(")], [LIB]),
    ("D2a", "P2: execute looks the exit up by the name begun",
     [(PR, "            let mine = agent.name_now(&mine);\n", "")], [LIB]),
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
    rows = ""
    if "a_withdraw_answered_does_not_count_for_the_request_asked_anew" in failed:
        rows = " rows: " + ",".join(
            l.strip().strip('"').split(":")[0]
            for l in out.splitlines()
            if l.strip().startswith('"Idle') or l.strip().startswith('"Busy'))
    return "fail", time.time() - t, " ".join(failed) + rows


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
            verdict, secs, failed = run(cmd)
            results.append(("LIB", verdict, secs, failed))
    finally:
        for f in files:
            open(os.path.join(W, f), "w").write(saved[f])
    caught = any(v != "pass" for _, v, _, _ in results)
    timeout = any(v == "TIMEOUT" for _, v, _, _ in results)
    verdict = "TIMEOUT (caught)" if timeout else ("KILLED" if caught else "SURVIVED")
    detail = "; ".join(f"{n}: {v} {s:.0f}s {fl}".rstrip() for n, v, s, fl in results)
    print(f"{pid} {what}: {verdict} [{detail}]", flush=True)
