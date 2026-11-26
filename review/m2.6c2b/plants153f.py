#!/usr/bin/env python3
"""Sixth look at #153 (M2.6c-2b), head 291746c: plants on a scratch copy of
the head, each applied alone as text (one or more exact replacements),
then the agent's tests: `cargo test -p skyzfs-agent -p skyzfs-agent-cli
--features skyzfs-agent-cli/test-hooks --no-fail-fast -- --skip
review_scratch` (600 s limit), then restored in a finally. One JSON line
per plant: KILLED (tests that failed, named; or the build failed),
SURVIVED, or an error.

Q*: D2's fix (the count before the destroy, the give-up with no take),
the report before the destroy, and what the merge with #149 brought in
(the rename's chain, the begin's count, the order of `Why`, the CLI's
word and report line). C*: T10, the limits of the other reads.

usage: python3 -I plants153f.py WORKTREE [ID,ID...]
"""
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(sys.argv[1])
AG = "crates/skyzfs-agent/src"
ENG = f"{AG}/engine.rs"
CLI = f"{AG}/zfs/cli.rs"
REC = f"{AG}/record.rs"
PROG = f"{AG}/program.rs"

FAILED_COMMIT = "        self.commit(Effect::Failed, |r| set(r, name, |p| p.attempts = failed))?;\n"
RENAME_COMMIT = "        self.commit(Effect::Intent, |r| set(r, name, |p| p.name = next.clone()))?;\n"
GIVE_UP_CHECK = ".is_some_and(|p| p.attempts >= plan::CHECKS_FAILED_MAX)"
PERSON = """    /// A person gave it up ([`Person::GiveUp`]). Where the daemon could
    /// not be asked whether the request had landed (away, or silent)
    /// until the hold's time was over, the request may still land after
    /// the give-up: object storage then has `snap/<t>` though the outcome
    /// says given up (an observation of the next look at #149).
    Person,
"""
NOT_WHOLE = """    /// [`crate::plan::CHECKS_FAILED_MAX`] takes in a row were not one
    /// whole take of the pool, or their members not as read before them
    /// (a dataset that `zfs snapshot -r` skips, for one); the last was
    /// destroyed (the lead's ruling on D1 of #153's review).
    NotWhole,
"""

PLANTS = [
    ("Q1", "the count after the destroy (the order D2 found)", ENG, [
        (FAILED_COMMIT, ""),
        (RENAME_COMMIT,
         "        self.commit(Effect::Intent, |r| set(r, name, |p| {\n"
         "            p.name = next.clone();\n"
         "            if failed {\n"
         "                p.attempts = p.attempts.saturating_add(1);\n"
         "            }\n"
         "        }))?;\n"),
    ]),
    ("Q2", "no give-up without a take after the last destroy", ENG, [
        (GIVE_UP_CHECK, ".is_some_and(|p| false && p.attempts >= plan::CHECKS_FAILED_MAX)"),
    ]),
    ("Q3", "the count only on a retake, not at the last check", ENG, [
        (FAILED_COMMIT + "        if failed < plan::CHECKS_FAILED_MAX {\n",
         "        if failed < plan::CHECKS_FAILED_MAX {\n" + FAILED_COMMIT),
    ]),
    ("Q4", "no report at the give-up without a take", ENG, [
        ("            self.tell(name, Report::NotWhole(Vec::new()));\n", ""),
    ]),
    ("Q5", "the give-up without a take recorded as too old", ENG, [
        ("            self.abandon(name, Why::NotWhole)?;\n            return Ok(Moved::Yes);\n",
         "            self.abandon(name, Why::TooOld)?;\n            return Ok(Moved::Yes);\n"),
    ]),
    ("Q6", "the give-up without a take one check late (>)", ENG, [
        (GIVE_UP_CHECK, ".is_some_and(|p| p.attempts > plan::CHECKS_FAILED_MAX)"),
    ]),
    ("Q7", "the last check's report after the give-up's commit (D1p)", ENG, [
        ("        self.tell(name, Report::NotWhole(names));\n        self.point(Effect::Destroy, false)?;\n",
         "        self.point(Effect::Destroy, false)?;\n"),
        ("        self.abandon(name, why)?;\n        Ok(Moved::Yes)\n    }\n\n    /// Destroy the take of",
         "        self.abandon(name, why)?;\n        self.tell(name, Report::NotWhole(names));\n"
         "        Ok(Moved::Yes)\n    }\n\n    /// Destroy the take of"),
    ]),
    ("Q8", "a not-whole take adopted with no reading not counted (D1l)", ENG, [
        ("            zfs::Taken::NotWhole(breaking) => return self.failed(name, breaking),\n",
         "            zfs::Taken::NotWhole(breaking) => {\n"
         "                if self.memory.get(name).and_then(|m| m.before.as_ref()).is_none() {\n"
         "                    return self.retake(name, false);\n"
         "                }\n"
         "                return self.failed(name, breaking);\n"
         "            }\n"),
    ]),
    ("Q9", "the rename leaves no link in `renamed` (merge)", ENG, [
        ("        self.renamed.insert(name.to_string(), next.clone());\n        // What this process knows",
         "        // What this process knows"),
    ]),
    ("Q10", "begin does not count (merge)", ENG, [
        ("        let _counted = self.zfs.count(&self.pool);\n        self.sweep()?;\n        let props",
         "        self.sweep()?;\n        let props"),
    ]),
    ("Q11", "begin counts after its sweep (merge)", ENG, [
        ("        let _counted = self.zfs.count(&self.pool);\n        self.sweep()?;\n        let props",
         "        self.sweep()?;\n        let _counted = self.zfs.count(&self.pool);\n        let props"),
    ]),
    ("Q12", "Why::NotWhole ordered before Why::Person (merge)", REC, [
        (PERSON + NOT_WHOLE, NOT_WHOLE + PERSON),
    ]),
    ("Q13", "the CLI's word for NotWhole the same as Person's (merge)", PROG, [
        ('        Why::NotWhole => "not-whole",', '        Why::NotWhole => "person",'),
    ]),
    ("Q14", "the report line names every dataset (N7's cap gone)", PROG, [
        ("        .take(NOT_WHOLE_NAMED)\n", ""),
    ]),
    ("Q15", "the report line after a restart names an empty list", PROG, [
        ('        return format!("not-whole: {given_up}; the datasets are not known after a restart");',
         '        return format!("not-whole: {given_up}; datasets: ");'),
    ]),
    ("Q16", "the give-up without a take not told to the sink (merge)", ENG, [
        ("            self.tell(name, Report::NotWhole(Vec::new()));\n",
         "            self.reports.push((name.to_string(), Report::NotWhole(Vec::new())));\n"),
    ]),
    ("Q17", "the count of a failed check saved at the points of Intent", ENG, [
        ("self.commit(Effect::Failed,", "self.commit(Effect::Intent,"),
    ]),
    ("Q18", "Failed missing from Effect::ALL (no --kill-at failed)", ENG, [
        ("    pub const ALL: [Effect; 28] = [", "    pub const ALL: [Effect; 27] = ["),
        ("        Effect::Take,\n        Effect::Failed,\n        Effect::Destroy,\n        Effect::Manifest,",
         "        Effect::Take,\n        Effect::Destroy,\n        Effect::Manifest,"),
    ]),
    # T8 (the names in a report), the review's D1m, D1n and D1q at this head.
    ("N1", "the report of a take not as before names nothing (D1m)", ENG, [
        ("                let differ = differing(&program.members, before);",
         "                let differ: Vec<String> = Vec::new();"),
    ]),
    ("N2", "differing(): the datasets read before and missing from the take not named (D1n)", ENG, [
        ("    names.extend(\n", "    names.truncate(names.len());\n    let _ = (\n"),
    ]),
    ("N3", "taken(): only the first dataset that breaks the take named (D1q)", f"{AG}/zfs.rs", [
        ("    if !breaking.is_empty() {\n", "    breaking.truncate(1);\n    if !breaking.is_empty() {\n"),
    ]),
    # T10.
    ("C2", "the plain members program given the full read's limit", CLI, [
        ("self.program(pool, MEMBERS_PROGRAM, &[pool, t], true, self.read_limit())",
         "self.program(pool, MEMBERS_PROGRAM, &[pool, t], true, self.full_read_limit())"),
    ]),
    ("C3", "tagged given the full read's limit", CLI, [
        ("self.program(pool, TAGGED_PROGRAM, &[pool, tag], true, self.read_limit())",
         "self.program(pool, TAGGED_PROGRAM, &[pool, tag], true, self.full_read_limit())"),
    ]),
    ("C9", "zpool status given the full read's limit", CLI, [
        ("        let out = self.zpool(&args, self.read_limit())?;\n        parse_status",
         "        let out = self.zpool(&args, self.full_read_limit())?;\n        parse_status"),
    ]),
    ("C12", "zpool get given the full read's limit (not in T10)", CLI, [
        ('        let output = self.exec("zpool", &args, &[], self.read_limit())?;',
         '        let output = self.exec("zpool", &args, &[], self.full_read_limit())?;'),
    ]),
]

CMD = ["cargo", "test", "-p", "skyzfs-agent", "-p", "skyzfs-agent-cli",
       "--features", "skyzfs-agent-cli/test-hooks", "--no-fail-fast",
       "--", "--skip", "review_scratch", "--test-threads=4"]


def run(plant):
    pid, what, rel, subs = plant
    path = ROOT / rel
    text = path.read_text()
    new_text = text
    for old, new in subs:
        if new_text.count(old) != 1:
            return {"id": pid, "what": what, "result": f"ERROR: site found {new_text.count(old)} times"}
        new_text = new_text.replace(old, new)
    try:
        path.write_text(new_text)
        try:
            out = subprocess.run(CMD, cwd=ROOT, capture_output=True, text=True, timeout=600)
        except subprocess.TimeoutExpired:
            return {"id": pid, "what": what, "result": "KILLED (timeout 600 s)"}
        lines = (out.stdout + out.stderr).splitlines()
        failed = [l.split()[1] for l in lines if l.startswith("test ") and l.endswith("FAILED")]
        if "could not compile" in out.stderr:
            errs = [l for l in out.stderr.splitlines() if l.startswith("error")][:3]
            return {"id": pid, "what": what, "result": "KILLED (build)", "by": errs}
        if out.returncode != 0:
            return {"id": pid, "what": what, "result": "KILLED", "by": failed}
        passed = [l for l in lines if l.startswith("test result")]
        return {"id": pid, "what": what, "result": "SURVIVED", "tests": passed}
    finally:
        path.write_text(text)


def main():
    only = set(sys.argv[2].split(",")) if len(sys.argv) > 2 else None
    for plant in PLANTS:
        if only and plant[0] not in only:
            continue
        print(json.dumps(run(plant)), flush=True)
    st = subprocess.run(["git", "status", "--short"], cwd=ROOT, capture_output=True, text=True)
    print(json.dumps({"worktree after": st.stdout.strip() or "clean"}))


main()
