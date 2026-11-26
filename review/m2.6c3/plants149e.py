"""Plants of the fifth look at #149 (M2.6c-3), head 6ec5909: C1's fix
(the slack in the asking deadline and the check before the send) taken
out or bent (C1*), T1's course check in the N6 test (T1a), and the
earlier plants re-run on this head (N6a, N5a, B1a, B1f, D2a). Each is
applied alone, as text, to a clean worktree of the head; its suite runs
with a 600 s limit; then the files are restored (try/finally).

  KILLED       a suite failed (a test caught the plant)
  SURVIVED     every suite passed
  NOT APPLIED  the text to replace was not found exactly once
  TIMEOUT      a suite ran past 600 s (counted as caught, and said)

Suite: LIB = `cargo test -p skyzfs-agent --lib`. For the C1 test and
the N6 test the failing rows are summarised.

WORKTREE=<path> CARGO_TARGET_DIR=<path> python3 -I plants149e.py [ids...]
"""
import collections, os, re, subprocess, sys, time

W = os.environ["WORKTREE"]
EN = "crates/skyzfs-agent/src/engine.rs"
PL = "crates/skyzfs-agent/src/plan.rs"
PR = "crates/skyzfs-agent/src/program.rs"

LIB = ["cargo", "test", "-p", "skyzfs-agent", "--lib"]

CHECK = """        if self.clock.mono_ms().saturating_sub(sent_mono) > ms(plan::ASKING_SLACK) {
            return Ok(Moved::No);
        }
"""
DEADLINE = """            sent.saturating_add(ms(plan::ASKING_SLACK))
                .saturating_add(ms(skyzfs_control::ASK_LIMIT)),
"""
STAMP = """        let sent = self.clock.now_ms();
        let sent_mono = self.clock.mono_ms();
        let asking_until = plan::held_until(
"""
AT_ASKING = """        let memory = self.memory.entry(name.clone()).or_default();
        memory.accepted = Some((sent, sent_mono));
        memory.withdrawn = false;
        self.point(Effect::Ask, false)?;
"""
HERE_ARM = """        DaemonSeen::Here { answer, .. } => {
            !seen.withdrawn
                && matches!(
                    answer,
                    Reply::Refused(Code::Ended | Code::NoAnswer | Code::NoUpload, _)
                )
        }
"""
PENDING_ARM = """            if pool.hold != Held::All || broken.is_some() || moved {
                return Action::Withdraw;
            }
"""

P = [
    ("C1a", "no check before the send (the author's mutant)",
     [(EN, CHECK, "")], [LIB]),
    ("C1b", "the slack left out of the asking deadline (the author's mutant)",
     [(EN, DEADLINE, "            sent.saturating_add(ms(skyzfs_control::ASK_LIMIT)),\n")], [LIB]),
    ("C1c", "the check against twice the slack (the author's mutant)",
     [(EN, CHECK, CHECK.replace("> ms(plan::ASKING_SLACK)", "> 2 * ms(plan::ASKING_SLACK)"))], [LIB]),
    ("C1d", "the boundary moved: nothing sent at exactly the slack (>=)",
     [(EN, CHECK, CHECK.replace(") > ms(", ") >= ms("))], [LIB]),
    ("C1e", "the check on the wall clock, from the record's `sent`",
     [(EN, CHECK, CHECK.replace("self.clock.mono_ms().saturating_sub(sent_mono)",
                                "self.clock.now_ms().saturating_sub(sent)"))], [LIB]),
    ("C1f", "the steady clock read after the asking save (the check measures from after it)",
     [(EN, STAMP, STAMP.replace("        let sent_mono = self.clock.mono_ms();\n", "")),
      (EN, AT_ASKING, "        let sent_mono = self.clock.mono_ms();\n" + AT_ASKING)], [LIB]),
    ("C1g", "the check moved before the point `Ask` (a stop at that point not counted)",
     [(EN, CHECK, ""),
      (EN, AT_ASKING, AT_ASKING.replace("        self.point(Effect::Ask, false)?;\n",
                                        CHECK + "        self.point(Effect::Ask, false)?;\n"))], [LIB]),
    ("C1h", "the operands of the check swapped (never past the slack)",
     [(EN, CHECK, CHECK.replace("self.clock.mono_ms().saturating_sub(sent_mono)",
                                "sent_mono.saturating_sub(self.clock.mono_ms())"))], [LIB]),
    ("T1a", "a pending request on a moved pool no longer withdrawn (N6's moved rows become the control)",
     [(PL, PENDING_ARM, PENDING_ARM.replace(" || moved {", " {"))], [LIB]),
    ("N6a", "N6: the clearing at the asking commit removed",
     [(EN, AT_ASKING, AT_ASKING.replace("        memory.withdrawn = false;\n", ""))], [LIB]),
    ("N5a", "N5's fix removed: no reading of the steady clock at the asking commit",
     [(EN, AT_ASKING, AT_ASKING.replace("        memory.accepted = Some((sent, sent_mono));\n", ""))], [LIB]),
    ("B1a", "P1: silent only for the daemon away (B1's fix reverted)",
     [(PL, HERE_ARM, "        DaemonSeen::Here { .. } => false,\n")], [LIB]),
    ("B1f", "P1: silent: the withdraw's guard dropped",
     [(PL, "            !seen.withdrawn\n                && matches!(", "            matches!(")], [LIB]),
    ("D2a", "P2: execute looks the exit up by the name begun",
     [(PR, "            let mine = agent.name_now(&mine);\n", "")], [LIB]),
]

C1_ROW = re.compile(r'"(Idle|Abandoned|Busy) no_answer=(\w+) give_up=(\w+) stuck=(\w+) slow=(\d+) s: (.*)')
N6_ROW = re.compile(r'"(Idle|Busy) no_answer=(\w+) moved=(\w+): (.*)')


def summarise(out):
    c1 = collections.Counter()
    early = collections.Counter()
    for l in out.splitlines():
        m = C1_ROW.search(l)
        if m:
            c1[(m.group(5), m.group(3))] += 1
            if "released while pending true" in m.group(6):
                early[m.group(5)] += 1
    n6 = [f"{m.group(1)}/{m.group(2)}/{m.group(3)}" for m in map(N6_ROW.search, out.splitlines()) if m]
    s = ""
    if c1:
        s += " C1 rows (slow s, give_up: n): " + ", ".join(f"{k[0]},{k[1]}:{v}" for k, v in sorted(c1.items(), key=lambda kv: (int(kv[0][0]), kv[0][1])))
        s += "; released while pending at slow: " + (", ".join(f"{k}:{v}" for k, v in sorted(early.items(), key=lambda kv: int(kv[0]))) or "none")
    if n6:
        s += " N6 rows (mode/no_answer/moved): " + ", ".join(n6)
    return s


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
    return "fail", time.time() - t, " ".join(failed) + summarise(out)


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
