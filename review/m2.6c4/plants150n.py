#!/usr/bin/env python3
"""Plants for the next look at skyzfs/skyzfs#150 (M2.6c-4) at d629834.

  WORKTREE=<clean worktree of the head> CARGO_TARGET_DIR=<scratch> python3 -I plants150n.py [ids...]

Each plant is one textual change, applied alone to a clean worktree of
the head, then `cargo test -p skyzfsd --features test-hooks` is run on the
carry tests and the parser's test (600 s limit), and the file is restored
whatever happens. KILLED: a test failed. SURVIVED: all passed. NOT
APPLIED: the text was not found exactly once, or the plant does not
compile. Q* are the author's (a sample of the thirteen), P9 and M* the
earlier ones again, N* the reviewer's new ones.
"""
import os
import subprocess
import sys

WT = os.environ["WORKTREE"]
KILL = "bins/skyzfsd/src/kill.rs"
UPLOAD = "bins/skyzfsd/src/upload.rs"
CLI = "bins/skyzfsd/src/cli.rs"

PLANTS = [
    # The author's (a sample of the thirteen of the answer).
    ("Q1", KILL, "KillRemote::publish does not tell the hook (F1)",
     "            carry.published(snapshot.name, snapshot.row.cut);", "            let _ = carry;"),
    ("Q2", KILL, "the ended publishes not read (F1)",
     "        self.answered();\n        self.ended(ended);", "        self.answered();\n        let _ = ended;"),
    ("Q3", KILL, "no fire at the chosen cut's release (F2)",
     "        self.carry.letting_go(number);\n", ""),
    ("Q4", KILL, "every acceptance counted (S2)",
     "&& seen.names.insert(asked.wanted.name.clone())",
     "&& seen.names.insert(format!(\"{}#{}\", asked.wanted.name, seen.names.len()))"),
    # Q4 as written above takes `seen.names` twice and does not compile;
    # Q4b is the same change, written so that it does.
    ("Q4b", KILL, "every acceptance counted (S2), compiling",
     "&& seen.names.insert(asked.wanted.name.clone())",
     "&& { let k = seen.names.len(); seen.names.insert(format!(\"{}#{k}\", asked.wanted.name)) }"),
    ("Q5", KILL, "any publish made gives the hook up",
     ".is_none_or(|wanted| wanted.name != name)", ".is_none()"),
    ("Q6", KILL, "any publish ended gives the hook up",
     ".find(|(name, _)| *name == wanted.name)", ".first()"),
    # Q6 as written above calls `first` on an iterator and does not
    # compile; Q6b is the same change, written so that it does.
    ("Q6b", KILL, "any publish ended gives the hook up, compiling",
     ".find(|(name, _)| *name == wanted.name)",
     ".find(|(name, _)| { let _ = (name, wanted); true })"),
    ("Q7", KILL, "the chosen cut not told apart in the give-up line",
     "seen.chosen.is_some_and(|chosen| chosen.row() == cut)", "false"),
    ("Q8", KILL, "the release fires under carry-put too",
     "        if self.at != CarryAt::Swap {\n            return;\n        }\n        let fired = self.fires(&mut self.seen(), |chosen| chosen.number == number);",
     "        let fired = self.fires(&mut self.seen(), |chosen| chosen.number == number);"),
    ("Q9", KILL, "the release of any cut fires carry-swap",
     "|chosen| chosen.number == number", "|chosen| { let _ = (chosen, number); true }"),
    # The earlier ones, again.
    ("P9", KILL, "an ask whose asker went still waited on (B1's test)",
     "if !reply.is_closed() {\n            self.seen().asked", "if true {\n            self.seen().asked"),
    ("M3", KILL, "C's bound: a cut numbered C chosen",
     "cut.number > self.after", "cut.number >= self.after"),
    ("M4", KILL, "G's bound: a cut of group G not chosen",
     "cut.group >= self.group", "cut.group > self.group"),
    ("M6", KILL, "C not checked",
     "&& cut.number > self.after", ""),
    ("M7", KILL, "a read from a later block is a new reading",
     "if from != 0 {\n            return;\n        }", ""),
    ("M13", KILL, "maker does not give the hook to the objects",
     "KillStore::new(objects, kill, Arc::clone(&die)).carrying(carry.clone())",
     "KillStore::new(objects, kill, Arc::clone(&die))"),
    ("M16", KILL, "an ask still waited on is kept after its asker went",
     "Err(oneshot::error::TryRecvError::Empty) if !asked.reply.is_closed() => {",
     "Err(oneshot::error::TryRecvError::Empty) => {"),
    # The reviewer's new ones.
    ("N1", KILL, "carry-swap fires at the release after the cut was let go",
     "        self.carry.letting_go(number);\n        self.inner.release(number).await\n",
     "        let released = self.inner.release(number).await;\n        self.carry.letting_go(number);\n        released\n"),
    ("N2", KILL, "a refused name counted among the names",
     "if answer == Answer::Accepted\n                        && seen.names.insert(asked.wanted.name.clone())",
     "if seen.names.insert(asked.wanted.name.clone())\n                        && answer == Answer::Accepted"),
    ("N3", KILL, "armed again at each new name after the Nth",
     "u64::try_from(seen.names.len()) == Ok(self.n)",
     "u64::try_from(seen.names.len()).is_ok_and(|len| len >= self.n)"),
    ("N4", KILL, "armed one name early",
     "u64::try_from(seen.names.len()) == Ok(self.n)",
     "u64::try_from(seen.names.len() + 1) == Ok(self.n)"),
    ("N5", KILL, "an end after the hook is done gives it up again (a second line)",
     "        let mut seen = self.seen();\n        if seen.done {\n            return;\n        }\n        let Some(wanted) = &seen.wanted else {\n            return;\n        };\n        let Some((name, progress))",
     "        let mut seen = self.seen();\n        let Some(wanted) = &seen.wanted else {\n            return;\n        };\n        let Some((name, progress))"),
    ("N6", KILL, "a publish made after the hook is done gives it up again (a second line)",
     "        if seen.done\n            || seen\n                .wanted",
     "        if seen\n                .wanted"),
    ("N7", KILL, "a publish made on the chosen cut does not give the hook up",
     "        seen.done = true;\n        let which = if",
     "        if seen.chosen.is_some_and(|chosen| chosen.row() == cut) {\n            return;\n        }\n        seen.done = true;\n        let which = if"),
    ("N8", KILL, "the arming line names G for C and C for G",
     "self.n, wanted.name, wanted.after, wanted.group", "self.n, wanted.name, wanted.group, wanted.after"),
    ("N9", KILL, "the end of a choice never said",
     "if let Some(gone) = seen.chosen {", "if let Some(gone) = None::<Uploadable> {"),
    ("N10", KILL, "a chosen cut that does not open as itself not said so",
     "if cut.itself {\n                    \"\"", "if true {\n                    \"\""),
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
