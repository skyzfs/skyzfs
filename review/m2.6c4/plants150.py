#!/usr/bin/env python3
"""Plants for the review of skyzfs/skyzfs#150 (M2.6c-4).

  WORKTREE=<clean worktree of the head> CARGO_TARGET_DIR=<scratch> python3 -I plants150.py [ids...]

Each plant is one textual change, applied alone to a clean worktree of
the head, then `cargo test -p skyzfsd --features test-hooks` is run on the
carry tests and the parser's test (600 s limit), and the file is restored
whatever happens. KILLED: a test failed. SURVIVED: all passed. NOT
APPLIED: the text was not found exactly once, or the plant does not
compile. P* are a sample of the author's eleven, M* the reviewer's.
"""
import os
import subprocess
import sys

WT = os.environ["WORKTREE"]
KILL = "bins/skyzfsd/src/kill.rs"
UPLOAD = "bins/skyzfsd/src/upload.rs"
CLI = "bins/skyzfsd/src/cli.rs"

PLANTS = [
    # The author's (a sample).
    ("P1", KILL, "counted when asked, refusals too",
     "if answer == Answer::Accepted {", "if true {"),
    ("P3", KILL, "the cut under way at the acceptance chosen",
     "seen.wanted = Some(asked.wanted);",
     "seen.wanted = Some(asked.wanted);\n                            seen.chosen = seen.reading.filter(|c| asked.wanted.chooses(c));"),
    ("P4", KILL, "opens as itself not checked",
     "chosen.itself && cut(chosen)", "cut(chosen)"),
    ("P5", KILL, "no new choice once a cut is chosen",
     "seen.chosen.is_some_and(|chosen| same(&chosen, &cut))", "seen.chosen.is_some()"),
    ("P6", KILL, "the commit of another cut fires carry-swap",
     "|chosen| chosen.row() == cut", "|_chosen| true"),
    ("P8", KILL, "an answer not passed on to the asker",
     "let _ = asked.reply.send(answer);", "let _ = answer;"),
    ("P9", KILL, "an ask whose asker went still waited on",
     "if !reply.is_closed() {\n            self.seen().asked", "if true {\n            self.seen().asked"),
    ("P10", KILL, "a segment PUT fires under carry-swap",
     "if self.at != CarryAt::Put || !key.contains(\"/seg/\") {", "if !key.contains(\"/seg/\") {"),
    ("P11", KILL, "any PUT fires carry-put",
     "if self.at != CarryAt::Put || !key.contains(\"/seg/\") {", "if self.at != CarryAt::Put {"),
    # The reviewer's.
    ("M1", KILL, "publish N armed one acceptance early",
     "if seen.accepted == self.n {", "if seen.accepted + 1 == self.n {"),
    ("M2", KILL, "publish N armed one acceptance late",
     "if seen.accepted == self.n {", "if seen.accepted == self.n + 1 {"),
    ("M3", KILL, "C's bound: a cut numbered C chosen",
     "cut.number > self.after", "cut.number >= self.after"),
    ("M4", KILL, "G's bound: a cut of group G not chosen",
     "cut.group >= self.group", "cut.group > self.group"),
    ("M5", KILL, "G not checked",
     "&& cut.group >= self.group", ""),
    ("M6", KILL, "C not checked",
     "&& cut.number > self.after", ""),
    ("M7", KILL, "a read from a later block is a new reading",
     "if from != 0 {\n            return;\n        }", ""),
    ("M8", KILL, "fires more than once",
     "seen.fired = true;", ""),
    ("M9", KILL, "carry-put does not compare the reading with the chosen cut",
     "reading.is_some_and(|reading| same(chosen, &reading))", "{ let _ = reading; true }"),
    ("M10", KILL, "the post does not read the answers",
     "fn posted(&self) {\n        self.answered();\n    }", "fn posted(&self) {}"),
    ("M11", KILL, "carry-swap fires before the commit landed",
     "        let commit = self.inner.commit(cut, expected).await?;",
     "        if let Some(carry) = &self.carry {\n            carry.committed(cut);\n        }\n        let commit = self.inner.commit(cut, expected).await?;"),
    ("M12", KILL, "carry-put fires before the PUT landed",
     "        self.inner.put(key, data).await?;\n        if let Some((counted, at)) = self.at {",
     "        if let Some(carry) = &self.carry {\n            carry.put_landed(key);\n        }\n        self.inner.put(key, data).await?;\n        if let Some((counted, at)) = self.at {"),
    ("M13", KILL, "maker does not give the hook to the objects (carry-put never fires in the daemon)",
     "KillStore::new(objects, kill, Arc::clone(&die)).carrying(carry.clone())",
     "KillStore::new(objects, kill, Arc::clone(&die))"),
    ("M14", KILL, "maker does not give the hook to the remote",
     "KillRemote::new(make(epoch), kill, Arc::clone(&die)).carrying(carry.clone())",
     "KillRemote::new(make(epoch), kill, Arc::clone(&die))"),
    ("M15", UPLOAD, "Uploads::begin puts Plain around the uploader even with a carry hook",
     "Uploading::start(runtime, keeper, make, desk, publishes, publish, carry)",
     "{ let _ = carry; Uploading::start(runtime, keeper, make, desk, publishes, publish, Plain) }"),
    ("M16", KILL, "an ask still waited on is kept after its asker went",
     "Err(oneshot::error::TryRecvError::Empty) if !asked.reply.is_closed() => {",
     "Err(oneshot::error::TryRecvError::Empty) => {"),
    ("M17", KILL, "carry-put and carry-swap swapped in Carry::new",
     "Some(KillAt::CarryPut(n)) => (CarryAt::Put, n),", "Some(KillAt::CarryPut(n)) => (CarryAt::Swap, n),"),
    ("M18", CLI, "the parser takes carry-put for carry-swap",
     "\"carry-put\" => Ok(KillAt::CarryPut(n)),", "\"carry-put\" => Ok(KillAt::CarrySwap(n)),"),
    ("M19", KILL, "the offer of no cut forgets the cut offered",
     "if cut.is_some() {\n            self.seen().offered = cut;\n        }", "self.seen().offered = cut;"),
    ("M20", KILL, "the cut read is not checked to be the one offered",
     "seen.offered.filter(|cut| cut.number == number)", "seen.offered"),
]

FILTERS = os.environ.get("FILTERS", "carry the_test_hooks_are_parsed_in_a_test_build").split()
# SCRATCH=<file>: appended to kill/tests.rs for the whole run, then removed.
SCRATCH = os.environ.get("SCRATCH")


def run(cmd, timeout):
    try:
        p = subprocess.run(cmd, cwd=WT, capture_output=True, text=True, timeout=timeout)
        return p.returncode, p.stdout + p.stderr
    except subprocess.TimeoutExpired:
        return None, "timeout"


def main():
    want = set(sys.argv[1:])
    env_ok = "CARGO_TARGET_DIR" in os.environ
    if not env_ok:
        print("CARGO_TARGET_DIR not set", file=sys.stderr)
        return 2
    os.environ.setdefault("CARGO_PROFILE_DEV_DEBUG", "0")
    os.environ.setdefault("CARGO_INCREMENTAL", "0")
    results = []
    kt = os.path.join(WT, "bins/skyzfsd/src/kill/tests.rs")
    with open(kt) as f:
        kt_text = f.read()
    try:
        if SCRATCH:
            with open(SCRATCH) as f, open(kt, "a") as g:
                g.write(f.read())
        plant_all(want, results)
    finally:
        with open(kt, "w") as f:
            f.write(kt_text)
    print()
    for pid, verdict, what, why in results:
        print(f"{pid:4} {verdict:11} {what}" + (f"  [{why}]" if why else ""))
    return 0


def plant_all(want, results):
    for pid, path, what, old, new in PLANTS:
        if want and pid not in want:
            continue
        full = os.path.join(WT, path)
        with open(full) as f:
            text = f.read()
        if text.count(old) != 1:
            results.append((pid, "NOT APPLIED", what, f"found {text.count(old)} times"))
            print(results[-1], flush=True)
            continue
        try:
            with open(full, "w") as f:
                f.write(text.replace(old, new))
            code, out = run(
                ["cargo", "test", "-p", "skyzfsd", "--features", "test-hooks", "--"] + FILTERS,
                600,
            )
            if code is None:
                verdict, why = "KILLED", "timeout (600 s)"
            elif "error[" in out or "could not compile" in out:
                verdict, why = "NOT APPLIED", "does not compile"
            elif code == 0:
                verdict, why = "SURVIVED", ""
            else:
                failed = sorted({l.split()[1] for l in out.splitlines()
                                 if l.startswith("test ") and l.endswith("FAILED")})
                verdict, why = "KILLED", " ".join(failed)
            results.append((pid, verdict, what, why))
            print(results[-1], flush=True)
        finally:
            with open(full, "w") as f:
                f.write(text)


if __name__ == "__main__":
    sys.exit(main())
