#!/usr/bin/env python3
"""Review of #161 (M2.6c-5c), head 3ea9be9: plants on a scratch copy of
the head, each applied alone as text (one or more exact replacements,
each site found exactly once), then the agent's tests, then restored in
a finally. One JSON line per plant: KILLED (the tests that failed,
named; or the build failed; or the 600 s limit), SURVIVED, or an error.
`key` lists which of the PR's new or changed tests failed.

W*: R1-reach (walk_limit for `zfs holds -r` and `tagged.lua`).
N*: NAMES (the command's own pool to `confirm`, `pool_of`).
M*: the pinned `Vanished` message.

usage: python3 -I plants161.py WORKTREE [ID,ID...]
"""
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(sys.argv[1])
AG = "crates/skyzfs-agent/src"
CLI = f"{AG}/zfs/cli.rs"

KEY = [
    "the_holds_and_the_tags_grow_with_the_snapshots_counted",
    "the_lists_limit_grows_with_the_snapshots_counted",
    "the_limits_grow_with_the_datasets_counted",
    "only_the_commands_own_pool_is_looked_for",
    "no_such_pool_is_a_missing_pool_only_where_zpool_list_agrees",
    "a_read_whose_snapshot_vanished_is_read_again",
    "a_read_that_goes_on_vanishing_is_an_error_that_names_it",
    "a_destroy_between_two_reads_is_no_missing_pool",
    "datasets_that_cannot_be_read_are_an_error",
    "a_value_read_alone_takes_a_reads_limit",
    "a_pool_that_does_not_exist_reads_as_recorded",
    "no_such_pool_is_no_pool",
]

P = []


def plant(pid, what, *files):
    P.append((pid, what, list(files)))


HOLDS_LIMIT = "        let limit = self.walk_limit(self.limits.holds_per_snapshot);\n"
TAGGED_LIMIT = "            let limit = zfs.walk_limit(zfs.limits.full_per_dataset);\n"
WALK = "            } => grown(self.read_limit(), per_snapshot, l.full_most, snapshots),\n            _ => self.read_limit(),\n"
WALK_GROWN = "            } => grown(self.read_limit(), per_snapshot, l.full_most, snapshots),\n"
CONFIRM = "            .any(|line| line == pool)\n"
POOL_OF = "    name.split(['/', '@']).next().unwrap_or(name)\n"
VANISHED = 'ZfsError::Vanished(format!("{said} (`zpool list` lists its pool)"))'

# --- R1-reach: walk_limit ---------------------------------------------------
plant("W1", "holds -r on a read's limit (the base's rule)", (CLI, [
    (HOLDS_LIMIT, "        let limit = self.read_limit();\n"),
]))
plant("W2", "tagged.lua on a read's limit (the base's rule)", (CLI, [
    (TAGGED_LIMIT, "            let limit = zfs.read_limit();\n"),
]))
plant("W3", "holds -r on tagged's allowance (full_per_dataset)", (CLI, [
    (HOLDS_LIMIT, HOLDS_LIMIT.replace("holds_per_snapshot", "full_per_dataset")),
]))
plant("W4", "tagged.lua on the holds allowance", (CLI, [
    (TAGGED_LIMIT, TAGGED_LIMIT.replace("full_per_dataset", "holds_per_snapshot")),
]))
plant("W5", "walk capped at most, not full_most", (CLI, [
    (WALK_GROWN, WALK_GROWN.replace("l.full_most", "l.most")),
]))
plant("W6", "walk's base the unscaled read (l.read), not read_limit()", (CLI, [
    (WALK_GROWN, WALK_GROWN.replace("grown(self.read_limit()", "grown(l.read")),
]))
plant("W7", "walk not capped at all", (CLI, [
    (WALK_GROWN, WALK_GROWN.replace("l.full_most", "Duration::MAX")),
]))
plant("W8", "a failed snapshot count gives the walk the gets' (full read) limit", (CLI, [
    (WALK, WALK.replace("            _ => self.read_limit(),\n",
                        "            Scale::Counted { snapshots: None, .. } => self.full_read_limit(),\n            _ => self.read_limit(),\n")),
]))
plant("W9", "walk counts datasets and snapshots at per_snapshot", (CLI, [
    ("            Scale::Counted {\n                snapshots: Some(snapshots),\n                ..\n            } => grown(self.read_limit(), per_snapshot, l.full_most, snapshots),\n",
     "            Scale::Counted {\n                datasets,\n                snapshots: Some(snapshots),\n            } => grown(self.read_limit(), per_snapshot, l.full_most, snapshots.saturating_add(datasets)),\n"),
]))
plant("W10", "no count yet (Base): the walk on full_most", (CLI, [
    (WALK, WALK.replace("            _ => self.read_limit(),\n",
                        "            Scale::Base => l.full_most,\n            _ => self.read_limit(),\n")),
]))
plant("W11", "default holds_per_snapshot 20 ms, not 200", (CLI, [
    ("            holds_per_snapshot: Duration::from_millis(200),\n",
     "            holds_per_snapshot: Duration::from_millis(20),\n"),
]))
plant("W12", "default holds_per_snapshot zero", (CLI, [
    ("            holds_per_snapshot: Duration::from_millis(200),\n",
     "            holds_per_snapshot: Duration::ZERO,\n"),
]))
plant("W13", "walk's base the change limit", (CLI, [
    (WALK_GROWN, WALK_GROWN.replace("grown(self.read_limit()", "grown(self.change_limit()")),
]))
plant("W14", "walk's base the gets' limit (full_read_limit)", (CLI, [
    (WALK_GROWN, WALK_GROWN.replace("grown(self.read_limit()", "grown(self.full_read_limit()")),
]))
plant("W15", "walk's per-snapshot term halved", (CLI, [
    (WALK_GROWN, WALK_GROWN.replace("per_snapshot, l.full_most", "per_snapshot / 2, l.full_most")),
]))
plant("W16", "snapshot count never kept (snapshots None after the pass)", (CLI, [
    ("        self.scale = Scale::Counted {\n            datasets,\n            snapshots,\n        };\n",
     "        let _ = snapshots;\n        self.scale = Scale::Counted {\n            datasets,\n            snapshots: None,\n        };\n"),
]))

# --- NAMES: the command's own pool ------------------------------------------
plant("N1", "confirm: the pool or any word of the command (the base's rule, widened)", (CLI, [
    (CONFIRM, "            .any(|line| line == pool || (!line.is_empty() && args.iter().any(|a| *a == line || a.starts_with(&format!(\"{line}/\")) || a.starts_with(&format!(\"{line}@\")))))\n"),
]))
plant("N2", "confirm: a listed line that begins with the pool", (CLI, [
    (CONFIRM, "            .any(|line| line.starts_with(pool))\n"),
]))
plant("N3", "confirm: a listed line the pool begins with", (CLI, [
    (CONFIRM, "            .any(|line| !line.is_empty() && pool.starts_with(line))\n"),
]))
plant("N4", "pool_of cuts at `/` only", (CLI, [
    (POOL_OF, "    name.split('/').next().unwrap_or(name)\n"),
]))
plant("N5", "pool_of cuts at `@` only", (CLI, [
    (POOL_OF, "    name.split('@').next().unwrap_or(name)\n"),
]))
plant("N6", "pool_of is the name", (CLI, [
    (POOL_OF, "    name\n"),
]))
plant("N7", "value() passes its dataset's name as the pool", (CLI, [
    ("self.zfs_bytes(&args, self.read_limit(), pool_of(name), false)?",
     "self.zfs_bytes(&args, self.read_limit(), name, false)?"),
]))
plant("N8", "holds -r passes its snapshot as the pool", (CLI, [
    ("self.zfs(&args, limit, true, pool_of(snapshot))", "self.zfs(&args, limit, true, snapshot)"),
]))
plant("N9", "zfs_bytes: every read names the pool (names_pool ignored, true)", (CLI, [
    ("        if names_pool {\n", "        if names_pool || true {\n"),
]))
plant("N10", "zfs_bytes: no read names the pool (names_pool ignored, false)", (CLI, [
    ("        if names_pool {\n", "        if names_pool && false {\n"),
]))
plant("N11", "props passes the subcommand (`get`) as the pool", (CLI, [
    ("            let error = self.failed(\"zpool\", &args, &output, false, pool);\n",
     "            let error = self.failed(\"zpool\", &args, &output, false, args[0]);\n"),
]))
plant("N12", "a program passes its last argument (the tag or t) as the pool", (CLI, [
    ("            Err(self.failed(\"zfs\", &all, &output, false, pool))\n",
     "            Err(self.failed(\"zfs\", &all, &output, false, all[all.len() - 1]))\n"),
]))
plant("N13", "confirm on the command's last word", (CLI, [
    (CONFIRM, "            .any(|line| args.last().is_some_and(|a| *a == line))\n"),
]))
plant("N14", "hold passes its snapshot as the pool", (CLI, [
    ("            return Err(self.failed(\"zfs\", &args, &output, true, pool));\n        }\n        // A member has the tag",
     "            return Err(self.failed(\"zfs\", &args, &output, true, &snapshot));\n        }\n        // A member has the tag"),
]))
plant("N15", "release passes its snapshot as the pool", (CLI, [
    ("            return match self.failed(\"zfs\", &args, &output, true, pool) {\n",
     "            return match self.failed(\"zfs\", &args, &output, true, &snapshot) {\n"),
]))
plant("N16", "destroy passes its snapshot as the pool", (CLI, [
    ("        match self.failed(\"zfs\", &args, &output, true, pool) {\n            ZfsError::NoSnapshot => Ok(()),\n            e => Err(e),\n        }\n    }\n\n    /// The snapshots under",
     "        match self.failed(\"zfs\", &args, &output, true, &snapshot) {\n            ZfsError::NoSnapshot => Ok(()),\n            e => Err(e),\n        }\n    }\n\n    /// The snapshots under"),
]))
plant("N17", "snapshot (the take) passes its snapshot as the pool", (CLI, [
    ("        Err(self.failed(\"zfs\", &args, &output, false, pool))\n    }\n\n    fn datasets",
     "        Err(self.failed(\"zfs\", &args, &output, false, &snapshot))\n    }\n\n    fn datasets"),
]))
plant("N18", "the count passes `list` as the pool", (CLI, [
    ("            .run(\"zfs\", &args, &[], self.limits.count, false, pool)\n",
     "            .run(\"zfs\", &args, &[], self.limits.count, false, args[0])\n"),
]))
plant("N19", "status passes `status` as the pool", (CLI, [
    ("        let out = self.zpool(&args, self.read_limit(), pool)?;\n",
     "        let out = self.zpool(&args, self.read_limit(), args[0])?;\n"),
]))
plant("N20", "the gets/list pass `list` as the pool to confirm (names_pool path)", (CLI, [
    ("                return Err(self.confirm(\"zfs\", args, &output, pool));\n",
     "                return Err(self.confirm(\"zfs\", args, &output, args[0]));\n"),
]))

# --- the author's plants not covered above (my versions) --------------------
plant("A4", "pool_of takes the last part of the name (author's W4)", (CLI, [
    (POOL_OF, "    name.rsplit(['/', '@']).next().unwrap_or(name)\n"),
]))
plant("A6", "walk's base grown by per_dataset up to full_most (author's R6)", (CLI, [
    (WALK_GROWN, WALK_GROWN.replace("grown(self.read_limit()", "grown(self.scaled(l.read, l.per_dataset, l.full_most)")),
]))
plant("A8a", "holds -r's snapshots at per_dataset (author's R8)", (CLI, [
    (HOLDS_LIMIT, HOLDS_LIMIT.replace("holds_per_snapshot", "per_dataset")),
]))
plant("A8b", "tagged.lua's snapshots at per_dataset (author's R8)", (CLI, [
    (TAGGED_LIMIT, TAGGED_LIMIT.replace("full_per_dataset", "per_dataset")),
]))
plant("N21", "props' fallback `zpool status` passes `status` as the pool", (CLI, [
    ("                    self.zpool(&[\"status\", \"-P\", pool], self.read_limit(), pool)\n",
     "                    self.zpool(&[\"status\", \"-P\", pool], self.read_limit(), \"status\")\n"),
]))
plant("N22", "assist's `zpool sync` passes `sync` as the pool", (CLI, [
    ("        self.zpool(&[\"sync\", pool], self.change_limit(), pool)?;\n",
     "        self.zpool(&[\"sync\", pool], self.change_limit(), \"sync\")?;\n"),
]))

# --- the Vanished message ---------------------------------------------------
plant("M1", "Vanished without why it is not a missing pool", (CLI, [
    (VANISHED, 'ZfsError::Vanished(said)'),
]))
plant("M2", "Vanished as a fixed text", (CLI, [
    (VANISHED, 'ZfsError::Vanished("a dataset vanished".into())'),
]))
plant("M3", "Vanished without the command's words", (CLI, [
    (VANISHED, 'ZfsError::Vanished(format!("{} (`zpool list` lists its pool)", String::from_utf8_lossy(&output.stderr).trim()))'),
]))
plant("M4", "Vanished with the message untrimmed", (CLI, [
    (VANISHED, 'ZfsError::Vanished(format!("{}: {} (`zpool list` lists its pool)", command(program, args), String::from_utf8_lossy(&output.stderr)))'),
]))

AGENT = ["cargo", "test", "-p", "skyzfs-agent", "-p", "skyzfs-agent-cli",
         "--features", "skyzfs-agent-cli/test-hooks", "--", "--test-threads=4"]


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
