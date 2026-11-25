#!/usr/bin/env python3
"""Third look at #147: plants on the head (ab8db11), each applied alone
to a scratch worktree, then `cargo test -p skyzfs-agent --lib` (600 s
limit), then restored in a finally. One JSON line per plant: KILLED (a
test failed, or the build failed: named), SURVIVED, or an error.

usage: python3 -I tl147_plants.py WORKTREE [ID,ID...]
S4 was first planted as a deletion, which did not build; it was run
again alone as below.
"""
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(sys.argv[1])
AG = "crates/skyzfs-agent/src"
CLI = f"{AG}/zfs/cli.rs"

PLANTS = [
    # The partial take (ab8db11).
    ("T1", "partial take: never destroyed (the arm left out)", CLI,
     "            Err(ZfsError::Other(said)) if partly_taken(&returned) => {\n                Err(self.destroy_partial(pool, t, &said))\n            }\n",
     ""),
    ("T2", "partial take: EEXIST not excluded", CLI,
     "taken.is_some_and(|n| n > 0) && errno != Some(EEXIST)",
     "taken.is_some_and(|n| n > 0)"),
    ("T3", "partial take: taken 0 counted", CLI,
     "taken.is_some_and(|n| n > 0)", "taken.is_some_and(|n| n >= 0)"),
    ("T4", "partial take: no `taken` counted as taken", CLI,
     "taken.is_some_and(|n| n > 0)", "taken.is_none_or(|n| n > 0)"),
    ("T5", "partial take: `failed` not required (author: equivalent)", CLI,
     "returned.contains_key(\"failed\") && taken.is_some_and", "taken.is_some_and"),
    ("T6", "partial take: the destroy's outcome left out of the error", CLI,
     "        let gone = self.destroy(pool, t);\n        ZfsError::Other(format!(\"{said}; {gone}\"))",
     "        let _gone = self.destroy(pool, t);\n        ZfsError::Other(said.to_string())"),
    ("T7", "partial take: `destroyed` whatever the destroy gave", CLI,
     "        let gone = self.destroy(pool, t);\n        ZfsError::Other(format!(\"{said}; {gone}\"))",
     "        let _gone = self.destroy(pool, t);\n        ZfsError::Other(format!(\"{said}; {pool}@{t} destroyed\"))"),
    ("T8", "partial take: errno read from the wrong key (EEXIST never seen)", CLI,
     "    let errno = returned\n        .get(\"errno\")", "    let errno = returned\n        .get(\"errno_\")"),
    ("T9", "partial take: the destroy run without -r", CLI,
     "let args = [\"destroy\", \"-r\", snapshot.as_str()];", "let args = [\"destroy\", snapshot.as_str()];"),
    ("T10", "partial take: destroy_taken's wording used (program named twice)", CLI,
     "                Err(self.destroy_partial(pool, t, &said))",
     "                Err(self.destroy_taken(pool, t, &said))"),
    ("T11", "partial take: run before the unreadable arm (order swapped)", CLI,
     "        match parse_program(&returned, pool) {\n            Err(ZfsError::Other(why)) if !returned.contains_key(\"failed\") => {",
     "        match parse_program(&returned, pool) {\n            Err(ZfsError::Other(said)) if partly_taken(&returned) => {\n                Err(self.destroy_partial(pool, t, &said))\n            }\n            Err(ZfsError::Other(why)) if !returned.contains_key(\"failed\") => {"),
    # NL-S1.
    ("S1a", "destroy: the name guard left out", CLI,
     "        if !skyzfs_control::valid_name(t) {", "        if false {"),
    ("S1b", "destroy: the guard on the pool, not on t", CLI,
     "        if !skyzfs_control::valid_name(t) {", "        if !skyzfs_control::valid_name(pool) {"),
    ("S1c", "destroy: `dataset does not exist` read as `was not taken` again", CLI,
     "                ZfsError::NoSnapshot => format!(\"{snapshot} was not found\"),",
     "                ZfsError::NoSnapshot => format!(\"{snapshot} was not taken\"),"),
    ("S1d", "destroy: `could not find any snapshots` read as `was not found`", CLI,
     "                format!(\"{snapshot} was not taken\")\n            }",
     "                format!(\"{snapshot} was not found\")\n            }"),
    # NL-S4.
    ("S4", "the prefix not taken off (program named twice)", CLI,
     "                let why = why.strip_prefix(PROGRAM_SAYS).unwrap_or(&why);\n",
     "                let why = why.as_str();\n"),
    # NL-F1.
    ("F1a", "held: the root's tag alone", CLI,
     "        if !tagged.contains(&snapshot) {\n            return Ok(false);\n        }\n        let Some(members)",
     "        if true {\n            return Ok(tagged.contains(&snapshot));\n        }\n        let Some(members)"),
    ("F1b", "held: the root not checked first", CLI,
     "        if !tagged.contains(&snapshot) {\n            return Ok(false);\n        }\n        let Some(members)",
     "        let Some(members)"),
    ("F1c", "held: any member instead of every", CLI,
     "            .all(|m| tagged.contains(&format!(\"{}@{t}\", m.name))))",
     "            .any(|m| tagged.contains(&format!(\"{}@{t}\", m.name))))"),
    ("F1d", "held: members gone between the reads read as held", CLI,
     "        let Some(members) = self.members(pool, t)? else {\n            return Ok(false);\n        };\n        Ok(members",
     "        let Some(members) = self.members(pool, t)? else {\n            return Ok(true);\n        };\n        Ok(members"),
    ("F1e", "fake held: the root's tag alone", f"{AG}/engine/fake.rs",
     "            p.snaps.contains_key(&format!(\"{POOL}@{t}\"))\n                && p.snaps\n                    .iter()\n                    .filter(|(name, _)| name.ends_with(&suffix))\n                    .all(|(_, s)| s.holds.contains(tag))",
     "            p.snaps\n                .get(&format!(\"{POOL}@{t}\"))\n                .is_some_and(|s| s.holds.contains(tag) || suffix.is_empty())"),
    # NL-F3.
    ("F3a", "manifest: 0x7f printable (the review's N1)", f"{AG}/manifest.rs",
     "(0x20..=0x7e).contains(b)", "(0x20..=0x7f).contains(b)"),
    ("F3b", "manifest: 0x20 not printable", f"{AG}/manifest.rs",
     "(0x20..=0x7e).contains(b)", "(0x21..=0x7e).contains(b)"),
    ("F3c", "program: the pattern widened to DEL", f"{AG}/zfs/cli/snapshot.lua",
     "[^\\32-\\126]", "[^\\32-\\127]"),
    # NL-T3.
    ("T3n", "says: the first `: ` (the review's N12)", CLI,
     "line.rsplit_once(\": \")", "line.split_once(\": \")"),
]


def run():
    try:
        p = subprocess.run(
            ["cargo", "test", "-p", "skyzfs-agent", "--lib", "--", "--test-threads=4"],
            cwd=ROOT, capture_output=True, text=True, timeout=600,
        )
    except subprocess.TimeoutExpired:
        return "TIMEOUT", []
    out = p.stdout + p.stderr
    failed = [l.split(" ")[1] for l in out.splitlines()
              if l.startswith("test ") and l.endswith("FAILED")]
    if p.returncode != 0 and not failed:
        failed = ["(build failed)"] if ("error[" in out or "error:" in out) else [f"(exit {p.returncode})"]
    return ("KILLED" if p.returncode != 0 else "SURVIVED"), failed


ONLY = set(sys.argv[2].split(",")) if len(sys.argv) > 2 else None

for pid, what, rel, old, new in PLANTS:
    if ONLY is not None and pid not in ONLY:
        continue
    path = ROOT / rel
    text = path.read_text()
    n = text.count(old)
    if n != 1:
        print(json.dumps({"id": pid, "what": what, "error": f"site found {n} times"}), flush=True)
        continue
    try:
        path.write_text(text.replace(old, new))
        result, failed = run()
    finally:
        path.write_text(text)
    print(json.dumps({"id": pid, "what": what, "result": result, "failed": failed}), flush=True)
