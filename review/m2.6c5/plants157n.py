#!/usr/bin/env python3
"""Next look at #157 (M2.6c-5), head 4edc1b3: plants on a scratch copy of
the head, each applied alone as text (one or more exact replacements,
each site found exactly once), then the agent's tests, then restored in
a finally. One JSON line per plant: KILLED (the tests that failed,
named; or the build failed; or the 600 s limit), SURVIVED, or an error.
`key` lists which of the tests this look asks about failed.

C*: the counters (B1's fix) and what else the read may leave out.
R*, V*: T2 and T4 again. Q*: T3 (the effects' macro). P*: N5 (NoPool).
D*: the first recordings test's derivation of the pairs pass (a plant
on test code: the second recordings test is the check on it).

usage: python3 -I plants157n.py WORKTREE [ID,ID...]
"""
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(sys.argv[1])
AG = "crates/skyzfs-agent/src"
ENG = f"{AG}/engine.rs"
ENGT = f"{AG}/engine/tests.rs"
CLI = f"{AG}/zfs/cli.rs"
REC = f"{AG}/zfs/cli/recordings.rs"

KEY = [
    "the_manifests_read_as_recorded",
    "the_three_reads_as_recorded",
    "counts_that_rise_with_a_take_do_not_fail_its_check",
    "the_counters_libzfs_calls_local_are_left_out",
    "a_value_with_a_newline_is_read_again_alone",
    "datasets_that_cannot_be_read_are_an_error",
    "every_effect_is_in_the_list_of_all",
    "each_effect_is_named_by_a_word_of_its_own",
]

P = []


def plant(pid, what, *files):
    P.append((pid, what, list(files)))


COUNTERS = 'const COUNTERS: [&str; 2] = ["filesystem_count", "snapshot_count"];\n'
CARRIED = "    property != ASSIST_PROPERTY && !COUNTERS.contains(&property)\n"

# --- the counters ---------------------------------------------------------
plant("C1", "snapshot_count not in COUNTERS", (CLI, [
    (COUNTERS, 'const COUNTERS: [&str; 1] = ["filesystem_count"];\n'),
]))
plant("C2", "filesystem_count not in COUNTERS", (CLI, [
    (COUNTERS, 'const COUNTERS: [&str; 1] = ["snapshot_count"];\n'),
]))
plant("C3", "COUNTERS empty (the read of e35f9e9)", (CLI, [
    (COUNTERS, 'const COUNTERS: [&str; 0] = [];\n'),
]))
plant("C4", "a counter read alone where its dataset is read again", (CLI, [
    ("            .filter(|(name, property)| again.contains(name) && carried(property));\n",
     "            .filter(|(name, _property)| again.contains(name));\n"),
]))
plant("C5", "the counters read, read alone, and kept (only the assist left out)", (CLI, [
    ("            .filter(|(name, property)| again.contains(name) && carried(property));\n",
     "            .filter(|(name, _property)| again.contains(name));\n"),
    ("            if !carried(&property) {\n", "            if property == ASSIST_PROPERTY {\n"),
]))
plant("C6", "over-broad: every property whose name ends _count left out", (CLI, [
    (CARRIED, '    property != ASSIST_PROPERTY && !COUNTERS.contains(&property) && !property.ends_with("_count")\n'),
]))
plant("C6b", "over-broad: every user property whose name ends :snapshot_count left out", (CLI, [
    (CARRIED, '    property != ASSIST_PROPERTY && !COUNTERS.contains(&property) && !property.ends_with(":snapshot_count")\n'),
]))
plant("C7", "over-broad: the limits left out too", (CLI, [
    (CARRIED, '    property != ASSIST_PROPERTY && !COUNTERS.contains(&property) && !property.ends_with("_limit")\n'),
]))
plant("C8", "over-broad: volsize and refreservation left out (libzfs calls them local too)", (CLI, [
    (CARRIED, '    property != ASSIST_PROPERTY && !COUNTERS.contains(&property) && property != "volsize" && property != "refreservation"\n'),
]))
plant("C9", "the assist property kept", (CLI, [
    (CARRIED, "    !COUNTERS.contains(&property)\n"),
]))

# --- T2, T4 ---------------------------------------------------------------
plant("R9", "the order compared by dataset only", (CLI, [
    ("                (a, b) if a == b => continue,\n",
     "                (a, b) if a.map(|x| x.0) == b.map(|x| x.0) => continue,\n"),
]))
plant("V1", "VALUE_MOST = 8192", (CLI, [
    ("pub const VALUE_MOST: usize = 8191;\n", "pub const VALUE_MOST: usize = 8192;\n"),
]))
plant("V2", "VALUE_MOST = 8190", (CLI, [
    ("pub const VALUE_MOST: usize = 8191;\n", "pub const VALUE_MOST: usize = 8190;\n"),
]))

# --- T3: the effects' macro -----------------------------------------------
PROBE = ('    Recover => "recover",\n',
         '    Recover => "recover",\n    /// A plant.\n    Probe => "probe",\n')
plant("Q19a", "an effect appended to the macro, the test's match not extended", (ENG, [PROBE]))
plant("Q19b", "an effect appended to the macro and to the test's match (expected: in ALL by construction)",
      (ENG, [PROBE]),
      (ENGT, [
          ("            Effect::Recover => 27,\n        }\n",
           "            Effect::Recover => 27,\n            Effect::Probe => 28,\n        }\n"),
          ("    assert_eq!(Effect::ALL.len(), place(Effect::Recover) + 1);\n",
           "    assert_eq!(Effect::ALL.len(), place(Effect::Probe) + 1);\n"
           "    assert!(Effect::ALL.contains(&Effect::Probe));\n"
           "    assert_eq!(Effect::from_word(\"PROBE\"), Some(Effect::Probe));\n"),
      ]))
plant("Q20", "two effects with one word", (ENG, [
    ('    Destroy => "destroy",\n', '    Destroy => "failed",\n'),
]))
plant("Q21", "two effects swapped in the list", (ENG, [
    ('    Take => "take",\n', '    TAKE_PLACEHOLDER\n'),
]))

# --- N5 -------------------------------------------------------------------
plant("P1", "NoPool on any 'dataset does not exist' of a read naming the pool (e35f9e9)", (CLI, [
    ("                .any(|line| line == gone)\n",
     "                .any(|line| line.contains(\"dataset does not exist\") || line == gone)\n"),
]))
plant("P2", "NoPool on a name that begins with the pool's", (CLI, [
    ("                .any(|line| line == gone)\n",
     "                .any(|line| line.starts_with(&format!(\"cannot open '{pool}\")) && line.ends_with(\"dataset does not exist\"))\n"),
]))
plant("P3", "never NoPool for 'dataset does not exist'", (CLI, [
    ("                .any(|line| line == gone)\n", "                .any(|line| line == gone && false)\n"),
]))

# --- the derivation of the pairs pass (test code) -------------------------
plant("D1", "pairs_of does not skip the lines that continue a value", (REC, [
    ("            .map_or(0, |value| value.matches('\\n').count().saturating_sub(1));\n",
     "            .map_or(0, |_value| 0usize);\n"),
]))
plant("D2", "pairs_of skips one line too many after a value of several lines", (REC, [
    ("            .map_or(0, |value| value.matches('\\n').count().saturating_sub(1));\n",
     "            .map_or(0, |value| value.matches('\\n').count());\n"),
]))

AGENT = ["cargo", "test", "-p", "skyzfs-agent", "-p", "skyzfs-agent-cli",
         "--features", "skyzfs-agent-cli/test-hooks"]


def swap_take_failed(text):
    """Q21: Take and Failed swapped in the macro's list."""
    a = '    /// `zfs snapshot -r` (the take).\n    TAKE_PLACEHOLDER\n'
    i = text.find(a)
    j = text.find('    Failed => "failed",\n')
    if i < 0 or j < 0:
        return None
    text = text.replace('    Failed => "failed",\n', '    Take => "take",\n', 1)
    return text.replace('    TAKE_PLACEHOLDER\n', '    Failed => "failed",\n', 1)


def run(p):
    pid, what, files = p
    texts = {}
    try:
        for rel, subs in files:
            path = ROOT / rel
            if rel not in texts:
                texts[rel] = path.read_text()
            new = path.read_text()
            for old, rep in subs:
                if new.count(old) != 1:
                    return {"id": pid, "what": what, "result": f"ERROR: site in {rel} found {new.count(old)} times"}
                new = new.replace(old, rep)
            if pid == "Q21":
                new = swap_take_failed(new)
                if new is None:
                    return {"id": pid, "what": what, "result": "ERROR: Q21 sites"}
            path.write_text(new)
        try:
            out = subprocess.run(AGENT, cwd=ROOT, capture_output=True, text=True, timeout=600)
        except subprocess.TimeoutExpired:
            return {"id": pid, "what": what, "result": "KILLED (timeout 600 s)"}
        lines = (out.stdout + out.stderr).splitlines()
        failed = [l.split()[1] for l in lines if l.startswith("test ") and l.endswith("FAILED")]
        key = [k for k in KEY if any(f.endswith(k) for f in failed)]
        if "could not compile" in out.stderr:
            errs = [l for l in out.stderr.splitlines() if l.startswith("error")][:3]
            return {"id": pid, "what": what, "result": "KILLED (build)", "by": errs}
        if out.returncode != 0:
            return {"id": pid, "what": what, "result": "KILLED", "key": key, "failed": len(failed), "by": failed[:12]}
        passed = [l for l in lines if l.startswith("test result")]
        return {"id": pid, "what": what, "result": "SURVIVED", "tests": passed}
    finally:
        for rel, text in texts.items():
            (ROOT / rel).write_text(text)


def main():
    only = set(sys.argv[2].split(",")) if len(sys.argv) > 2 else None
    for p in P:
        if only and p[0] not in only:
            continue
        print(json.dumps(run(p)), flush=True)
    st = subprocess.run(["git", "status", "--short"], cwd=ROOT, capture_output=True, text=True)
    print(json.dumps({"worktree after": st.stdout.strip() or "clean"}))


main()
