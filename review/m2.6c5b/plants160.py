#!/usr/bin/env python3
"""Review of #160 (M2.6c-5b), head 9354462: plants on a scratch copy of
the head, each applied alone as text (one or more exact replacements,
each site found exactly once), then the agent's tests, then restored in
a finally. One JSON line per plant: KILLED (the tests that failed,
named; or the build failed; or the 600 s limit), SURVIVED, or an error.
`key` lists which of the PR's new or changed tests failed.

L*: R1 (the list's limit counts the snapshots). V*: the destroy fix
(NoPool only where `zpool list` agrees; Vanished read again). T5*, N7*:
#157's T5 and N7. F*: not plants: the head with a test's fake answer
made as real zfs prints it (expected: PASS). S1: a scratch engine test
(the rig's shape in one process), run at the head and under V1.

usage: python3 -I plants160.py WORKTREE SCRATCH_RS [ID,ID...]
"""
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(sys.argv[1])
SCRATCH = pathlib.Path(sys.argv[2]).read_text()
AG = "crates/skyzfs-agent/src"
ZFS = f"{AG}/zfs.rs"
ENGT = f"{AG}/engine/tests.rs"
CLI = f"{AG}/zfs/cli.rs"
CLIT = f"{AG}/zfs/cli/tests.rs"
REC = f"{AG}/zfs/cli/recordings.rs"

KEY = [
    "the_lists_limit_grows_with_the_snapshots_counted",
    "the_limits_grow_with_the_datasets_counted",
    "a_value_read_alone_takes_a_reads_limit",
    "the_count_as_recorded",
    "a_destroy_between_two_reads_is_no_missing_pool",
    "a_read_whose_snapshot_vanished_is_read_again",
    "a_read_that_goes_on_vanishing_is_an_error_that_names_it",
    "no_such_pool_is_a_missing_pool_only_where_zpool_list_agrees",
    "datasets_that_cannot_be_read_are_an_error",
    "a_pool_that_does_not_exist_reads_as_recorded",
    "no_such_pool_is_no_pool",
    "a_persons_properties_named_like_the_counters_are_kept",
    "the_manifests_read_as_recorded",
    "the_three_reads_as_recorded",
    "review_s1_destroy_between_reads_in_one_process",
]

P = []


def plant(pid, what, *files):
    P.append((pid, what, list(files)))


LIST_LIMIT_GROWN = """            } => grown(
                l.read,
                l.full_per_dataset,
                l.full_most,
                datasets.saturating_add(snapshots),
            ),
"""
SNAP_PASS = """        let snapshots = self
            .run("zfs", &args, &[], self.read_limit(), false)
            .ok()
"""
END_COUNT = """        self.scale = Scale::Counted {
            datasets,
            snapshots,
        };
        Ok(datasets)
"""
NAMES = ".is_some_and(|rest| rest.is_empty() || rest.starts_with('/') || rest.starts_with('@'))\n"
MEMBERS_AGAIN = """        let returned = self.again(|zfs| {
            let limit = zfs.read_limit();
            zfs.program(pool, MEMBERS_PROGRAM, &[pool, t], true, limit)
        })?;
"""
TAGGED_AGAIN = """        let returned = self.again(|zfs| {
            let limit = zfs.read_limit();
            zfs.program(pool, TAGGED_PROGRAM, &[pool, tag], true, limit)
        })?;
"""

# --- R1: the list's limit --------------------------------------------------
plant("L1", "the list on the gets' limit", (CLI, [
    ("let listed = self.zfs_bytes(&list, self.list_limit(), Some(pool))?;",
     "let listed = self.zfs_bytes(&list, limit, Some(pool))?;"),
]))
plant("L2", "snapshots counted but not added", (CLI, [
    ("                datasets.saturating_add(snapshots),\n",
     "                datasets.saturating_add(snapshots * 0),\n"),
]))
plant("L3", "the list capped at most, not full_most", (CLI, [
    ("                l.full_most,\n                datasets.saturating_add(snapshots),\n",
     "                l.most,\n                datasets.saturating_add(snapshots),\n"),
]))
plant("L4", "the gets on the list's limit", (CLI, [
    ("        let limit = self.full_read_limit();\n        let list = list_words(pool);\n",
     "        let limit = self.list_limit();\n        let list = list_words(pool);\n"),
]))
plant("L5", "the snapshot pass within the count's own limit", (CLI, [
    (SNAP_PASS, SNAP_PASS.replace("self.read_limit()", "self.limits.count")),
]))
plant("L6", "a failed snapshot count fails the count", (CLI, [
    (END_COUNT, END_COUNT.replace("        Ok(datasets)\n",
                                  "        if snapshots.is_none() {\n            return Err(ZfsError::Other(\"snapshots\".into()));\n        }\n        Ok(datasets)\n")),
]))
plant("L7", "a failed snapshot count sets the list to the base", (CLI, [
    ("            _ => self.full_read_limit(),\n",
     "            Scale::Counted { snapshots: None, .. } => l.read,\n            _ => self.full_read_limit(),\n"),
]))
plant("L8", "snapshots at per_dataset, datasets at full_per_dataset", (CLI, [
    (LIST_LIMIT_GROWN,
     "            } => grown(l.read, l.full_per_dataset, l.full_most, datasets)\n"
     "                .saturating_add(l.per_dataset * u32::try_from(snapshots).unwrap_or(u32::MAX))\n"
     "                .min(l.full_most),\n"),
]))
plant("L9", "the larger of datasets and snapshots, not the sum", (CLI, [
    ("                datasets.saturating_add(snapshots),\n",
     "                datasets.max(snapshots),\n"),
]))
plant("L10", "count returns datasets and snapshots", (CLI, [
    (END_COUNT, END_COUNT.replace("        Ok(datasets)\n",
                                  "        Ok(datasets + snapshots.unwrap_or(0))\n")),
]))
plant("L11", "the snapshot pass within the base read limit (not scaled)", (CLI, [
    (SNAP_PASS, SNAP_PASS.replace("self.read_limit()", "self.limits.read")),
]))
plant("L12", "the snapshot pass run after a failed dataset count too", (CLI, [
    ("            Err(e) => {\n                self.scale = Scale::Base;\n                return Err(e);\n",
     "            Err(e) => {\n                self.scale = Scale::Base;\n"
     "                let _ = self.run(\"zfs\", &[\"list\", \"-r\", \"-H\", \"-o\", \"name\", \"-t\", \"snapshot\", pool], &[], self.read_limit(), false);\n"
     "                return Err(e);\n"),
]))

# --- the destroy fix ------------------------------------------------------
plant("V1", "no zpool list: every 'no such pool' is NoPool (the base)", (CLI, [
    ("            ZfsError::NoPool(_) => self.confirm(program, args, output),\n",
     "            e @ ZfsError::NoPool(_) => e,\n"),
]))
plant("V2", "a vanished read not made again", (CLI, [
    ("        let mut tries = 1;\n", "        let mut tries = VANISHED_READS;\n"),
]))
plant("V3", "one read more than VANISHED_READS", (CLI, [
    ("if tries < VANISHED_READS => tries += 1,", "if tries <= VANISHED_READS => tries += 1,"),
]))
plant("V4", "a pool named by any word that begins with its name", (CLI, [
    (NAMES, ".is_some()\n"),
]))
plant("V5a", "a zpool list that exits non-zero read as no pool listed", (CLI, [
    ("            Ok(out) if out.code == Some(0) => out.stdout,\n",
     "            Ok(out) if out.code == Some(0) || out.code.is_some() => out.stdout,\n"),
]))
plant("V5b", "a zpool list that does not run read as no pool listed", (CLI, [
    ('            Err(e) => return ZfsError::Other(format!("{said}; and {e}")),\n',
     "            Err(_) => Vec::new(),\n"),
]))
plant("V6", "members not read again", (CLI, [
    (MEMBERS_AGAIN, "        let limit = self.read_limit();\n"
                    "        let returned = self.program(pool, MEMBERS_PROGRAM, &[pool, t], true, limit)?;\n"),
]))
plant("V7", "held's holds -r not read again", (CLI, [
    ("self.again(|zfs| zfs.tagged_snapshots(&snapshot, tag))?", "self.tagged_snapshots(&snapshot, tag)?"),
]))
plant("V8", "the exact-name path of zfs list/get not confirmed", (CLI, [
    ('                return Err(self.confirm("zfs", args, &output));\n',
     '                return Err(ZfsError::NoPool(command("zfs", args)));\n'),
]))
plant("V9", "tagged not read again", (CLI, [
    (TAGGED_AGAIN, "        let limit = self.read_limit();\n"
                   "        let returned = self.program(pool, TAGGED_PROGRAM, &[pool, tag], true, limit)?;\n"),
]))
plant("V10", "a listed pool read as NoPool", (CLI, [
    ('            ZfsError::Vanished(format!("{said} (`zpool list` lists its pool)"))\n',
     "            ZfsError::NoPool(command(program, args))\n"),
]))
plant("V11", "names: a snapshot of the pool (pool@t) not the pool's", (CLI, [
    (NAMES, ".is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))\n"),
]))
plant("V12", "names: a dataset in the pool (pool/x) not the pool's", (CLI, [
    (NAMES, ".is_some_and(|rest| rest.is_empty() || rest.starts_with('@'))\n"),
]))
plant("V13", "only the command's last word checked", (CLI, [
    (".any(|pool| args.iter().any(|arg| names(arg, pool)));",
     ".any(|pool| args.last().is_some_and(|arg| names(arg, pool)));"),
]))
plant("V14", "again reads again on any error", (CLI, [
    ("Err(ZfsError::Vanished(_)) if tries < VANISHED_READS", "Err(_) if tries < VANISHED_READS"),
]))
plant("V15", "again bounded at 100", (CLI, [
    ("if tries < VANISHED_READS => tries += 1,", "if tries < 100 => tries += 1,"),
]))
plant("V16", "Vanished shown without its command and message", (ZFS, [
    ('            ZfsError::Vanished(what) => write!(f, "{what}"),\n',
     '            ZfsError::Vanished(_) => write!(f, "a dataset vanished"),\n'),
]))
plant("V17", "Vanished without the command's message", (CLI, [
    ('            String::from_utf8_lossy(&output.stderr).trim()\n        );\n        let list = ',
     '            ""\n        );\n        let list = '),
]))
plant("V18", "a missing pool's NoPool without the command's words", (CLI, [
    ("        } else {\n            ZfsError::NoPool(command(program, args))\n        }\n",
     "        } else {\n            ZfsError::NoPool(String::new())\n        }\n"),
]))
plant("V19", "names matched only on the dataset part before '@' of the listed pool (pool == arg)", (CLI, [
    (NAMES, ".is_some_and(|rest| rest.is_empty())\n"),
]))

# --- T5, N7 -----------------------------------------------------------------
CARRIED = "    property != ASSIST_PROPERTY && !COUNTERS.contains(&property)\n"
plant("T5a", "the counter filter widened to *_count", (CLI, [
    (CARRIED, '    property != ASSIST_PROPERTY && !COUNTERS.contains(&property) && !property.ends_with("_count")\n'),
]))
plant("T5b", "the counter filter widened to names that contain a counter", (CLI, [
    (CARRIED, "    property != ASSIST_PROPERTY && !COUNTERS.iter().any(|c| property.contains(c))\n"),
]))
plant("T5c", "the counter filter widened to *:snapshot_count", (CLI, [
    (CARRIED, '    property != ASSIST_PROPERTY && !COUNTERS.contains(&property) && !property.ends_with(":snapshot_count")\n'),
]))
plant("N7", "the values pass given as lossy text", (REC, [
    ('            fake.give(&get_words("name,property,value"), Ok(given(&values)));\n',
     '            fake.ok(&get_words("name,property,value"), &values.stdout);\n'),
]))
plant("N7b", "every value read alone given as lossy text", (REC, [
    ("                fake.give(&words, Ok(output.clone()));\n            }\n            // A value not recorded alone",
     "                let mut o = output.clone();\n"
     "                o.stdout = String::from_utf8_lossy(&o.stdout).into_owned().into_bytes();\n"
     "                fake.give(&words, Ok(o));\n            }\n            // A value not recorded alone"),
]))
plant("N7c", "check: odd-raw is never read alone (assert added; expected PASS)", (REC, [
    ("            let raw = b\"a\\xff\\xfeb\\xc3\".as_slice();\n",
     "            let raw = b\"a\\xff\\xfeb\\xc3\".as_slice();\n"
     "            let alone_raw = format!(\"zfs get -H -p -o value skyzfs:v {pool}/odd-raw\");\n"
     "            assert!(!zfs.host().words().contains(&alone_raw.as_str()), \"REVIEW odd-raw read alone\");\n"),
]))

# --- fidelity checks (not plants; expected PASS) ----------------------------
plant("F1", "engine test: the second holds -r answered as -r really answers (exit 0, 'no datasets available')", (ENGT, [
    ("""        host.fails(
            &words,
            1,
            &format!("cannot open '{pool}@{t}': dataset does not exist\\n"),
        );
""", """        host.give(
            &words,
            Ok(crate::host::fake::output(0, "", "no datasets available\\n")),
        );
"""),
]))
plant("F2", "unit test: the same for a_read_whose_snapshot_vanished_is_read_again", (CLIT, [
    ("""            .fails(
                HOLDS_R,
                1,
                "cannot open 'tank@skz-1': dataset does not exist\\n",
            )
""", """            .give(
                HOLDS_R,
                Ok(crate::host::fake::output(0, "", "no datasets available\\n")),
            )
"""),
]))
plant("S1", "scratch: the rig's shape in one process, at the head (expected PASS)", (ENGT, [("__APPEND__", SCRATCH)]))
plant("S1V1", "scratch S1 under V1 (expected KILLED)", (ENGT, [("__APPEND__", SCRATCH)]), (CLI, [
    ("            ZfsError::NoPool(_) => self.confirm(program, args, output),\n",
     "            e @ ZfsError::NoPool(_) => e,\n"),
]))

AGENT = ["cargo", "test", "-p", "skyzfs-agent", "-p", "skyzfs-agent-cli",
         "--features", "skyzfs-agent-cli/test-hooks", "--", "--nocapture", "--test-threads=8"]


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
                if old == "__APPEND__":
                    new = new + rep
                    continue
                if new.count(old) != 1:
                    return {"id": pid, "what": what, "result": f"ERROR: site in {rel} found {new.count(old)} times"}
                new = new.replace(old, rep)
            path.write_text(new)
        try:
            out = subprocess.run(AGENT, cwd=ROOT, capture_output=True, text=True, timeout=600)
        except subprocess.TimeoutExpired:
            return {"id": pid, "what": what, "result": "KILLED (timeout 600 s)"}
        lines = (out.stdout + out.stderr).splitlines()
        failed = [l.split()[1] for l in lines if l.startswith("test ") and l.endswith("FAILED")]
        key = [k for k in KEY if any(f.endswith(k) for f in failed)]
        notes = [l for l in lines if "REVIEW-S1" in l]
        if "could not compile" in out.stderr:
            errs = [l for l in out.stderr.splitlines() if l.startswith("error")][:3]
            return {"id": pid, "what": what, "result": "KILLED (build)", "by": errs}
        if out.returncode != 0:
            return {"id": pid, "what": what, "result": "KILLED", "key": key, "failed": len(failed), "by": failed[:12], "notes": notes}
        passed = [l for l in lines if l.startswith("test result")]
        return {"id": pid, "what": what, "result": "SURVIVED", "tests": passed, "notes": notes}
    finally:
        for rel, text in texts.items():
            (ROOT / rel).write_text(text)


def main():
    only = set(sys.argv[3].split(",")) if len(sys.argv) > 3 else None
    for p in P:
        if only and p[0] not in only:
            continue
        print(json.dumps(run(p)), flush=True)
    st = subprocess.run(["git", "status", "--short"], cwd=ROOT, capture_output=True, text=True)
    print(json.dumps({"worktree after": st.stdout.strip() or "clean"}))


main()
