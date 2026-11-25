"""Textual plants for the next look at #142 (M2.6b1) at b79083a and
347a17f (the D plants need 347a17f's `parent_of`), one at a time, each
file restored from its saved text in `finally`. A plant is a
list of edits (file, old, new); each old text must occur once. Run from a
worktree of the head:
WORKTREE=<path> OUT=<json> python3 plants142b.py [name prefixes...]
The suite skips the review's own scratch tests (`--skip review`)."""
import json, os, subprocess, sys, time

W = os.environ.get("WORKTREE", ".")
OUT = os.environ.get("OUT", "plants142b.json")
ENV = dict(os.environ, CARGO_INCREMENTAL="0")
SUITE = ["cargo", "test", "-p", "skyzfs-agent", "--lib", "--", "--skip", "review"]
RC = "crates/skyzfs-agent/src/record.rs"
RT = "crates/skyzfs-agent/src/record/tests.rs"
PL = "crates/skyzfs-agent/src/plan.rs"
PT = "crates/skyzfs-agent/src/plan/tests.rs"

# Edits of the table used by several plants.
ROW_FIXTURE = "            seen().at(UNTIL + 1).cuts(carrying()).assisted(UNTIL),\n            Action::Wait,\n"
# The held_until row's fixture as it was at 05d79f4.
FIXTURE_REVERTED = (PT, ROW_FIXTURE, "            seen().at(UNTIL + 1).cuts(carrying()),\n            Action::Wait,\n")
# The same, with the expectation the new rule gives (a sync is due).
FIXTURE_REVERTED_ASSIST = (PT, ROW_FIXTURE, "            seen().at(UNTIL + 1).cuts(carrying()),\n            Action::Assist,\n")
# The held_until row dropped.
HERE_ROW = ("        (\n            \"here, past held_until: a publish pending keeps its hold\",\n            asked(),\n"
            "            // Helped a moment ago, so that no sync is due either.\n" + ROW_FIXTURE + "        ),\n")
HERE_ROW_DROPPED = (PT, HERE_ROW, "")
# T3's row dropped.
T3_ROW = ("        (\n            \"here, the premise refused\",\n            intent(),\n            none().no_premise(),\n"
          "            Action::Take,\n        ),\n")
T3_ROW_DROPPED = (PT, T3_ROW, "")
# T4's two rows dropped.
T4_ROWS = ("            rows.push((\n                what,\n                phase.clone(),\n"
           "                ended().looked(SnapSeen::Ours(COMMIT)).unheld(),\n                Action::Done(COMMIT),\n            ));\n"
           "            rows.push((\n                what,\n                phase.clone(),\n"
           "                ended().looked(SnapSeen::Ours(COMMIT)).gone(),\n                Action::Done(COMMIT),\n            ));\n")
T4_ROWS_DROPPED = (PT, T4_ROWS, "")

CARRIED = ("    if carried {\n        let from = asked_ms.saturating_add(ms(ASSIST_AFTER));\n"
           "        let from = assisted_ms.map_or(from, |last| last.max(from));\n        after(from, ASSIST_AFTER)\n    } else {\n")
FROM2X = "        let from = asked_ms.saturating_add(ms(ASSIST_AFTER));\n"
LASTMAX = "        let from = assisted_ms.map_or(from, |last| last.max(from));\n"
AFTERFROM = "        after(from, ASSIST_AFTER)\n"
HERE_SPLIT = ("        DaemonSeen::Here { cuts, answer } => (cuts, answer),\n    };\n")
INTENT_TAKE = "            (None, DaemonSeen::Here { .. }) => Action::Take,\n"
LOOK_OURS = "        Some(SnapSeen::Ours(commit)) => Action::Done(commit),\n"
PENDING_DUE = "            if assist_due(carried, asked_ms, seen.assisted_ms, seen.now_ms) {\n"
MAKE_DIR_SYNC = ("    match parent_of(dir) {\n        Some(parent) => File::open(parent)?.sync_all(),\n"
                 "        None => Ok(()),\n    }\n")

P = [
    # The assist (b79083a): the author's four, then the review's own.
    ("A1 the old rule: no sync while a cut carries the request", [(PL, CARRIED, "    if carried {\n        false\n    } else {\n")]),
    ("A2 a sync every pause while a cut carries it", [(PL, "    if carried {\n", "    if carried && false {\n")]),
    ("A3 the start at twice ASSIST_AFTER dropped", [(PL, FROM2X, "        let from = asked_ms;\n")]),
    ("A4 the last sync not counted while carried", [(PL, LASTMAX, "        let from = assisted_ms.map_or(from, |_| from);\n")]),
    ("A5 the start at twice ASSIST_AFTER only where no sync is recorded", [(PL, LASTMAX, "        let from = assisted_ms.map_or(from, |last| last);\n")]),
    ("A6 due a ms after the boundary (> for >=)", [(PL, AFTERFROM, "        now_ms > from.saturating_add(ms(ASSIST_AFTER))\n")]),
    ("A7 carried: the interval ASSIST_PAUSE", [(PL, AFTERFROM, "        after(from, ASSIST_PAUSE)\n")]),
    ("A8 carried: one sync only", [(PL, AFTERFROM, "        assisted_ms.is_none() && after(from, ASSIST_AFTER)\n")]),
    ("A9 carried: twice ASSIST_AFTER after the last sync", [(PL, LASTMAX, "        let from = assisted_ms.map_or(from, |last| last.saturating_add(ms(ASSIST_AFTER)));\n")]),
    ("A10 carried: min for max", [(PL, LASTMAX, "        let from = assisted_ms.map_or(from, |last| last.min(from));\n")]),
    ("A11 carried: from three times ASSIST_AFTER", [(PL, FROM2X, "        let from = asked_ms.saturating_add(2 * ms(ASSIST_AFTER));\n")]),
    ("A12 the cut's test ignored (always one a second)", [(PL, "            let carried = qualifies(cuts, &publish.count, taken);\n", "            let carried = false && qualifies(cuts, &publish.count, taken);\n")]),
    # The held_until row's fixture.
    ("H0 the row's fixture reverted, nothing planted (a baseline)", [FIXTURE_REVERTED]),
    ("H1 the hold released past held_until with the daemon here", [(PL, HERE_SPLIT, HERE_SPLIT + "    if let Some((_, until)) = asked {\n        if seen.now_ms >= until && pool.held {\n            return Action::Release;\n        }\n    }\n")]),
    ("H1d H1 with the row dropped", [(PL, HERE_SPLIT, HERE_SPLIT + "    if let Some((_, until)) = asked {\n        if seen.now_ms >= until && pool.held {\n            return Action::Release;\n        }\n    }\n"), HERE_ROW_DROPPED]),
    ("H1r H1 with the fixture reverted and Assist expected", [(PL, HERE_SPLIT, HERE_SPLIT + "    if let Some((_, until)) = asked {\n        if seen.now_ms >= until && pool.held {\n            return Action::Release;\n        }\n    }\n"), FIXTURE_REVERTED_ASSIST]),
    ("H2 released past held_until where no sync is due", [(PL, PENDING_DUE, "            if !assist_due(carried, asked_ms, seen.assisted_ms, seen.now_ms)\n                && asked.is_some_and(|(_, until)| seen.now_ms >= until)\n            {\n                return Action::Release;\n            }\n" + PENDING_DUE)]),
    ("H2r H2 with the fixture reverted and Assist expected", [(PL, PENDING_DUE, "            if !assist_due(carried, asked_ms, seen.assisted_ms, seen.now_ms)\n                && asked.is_some_and(|(_, until)| seen.now_ms >= until)\n            {\n                return Action::Release;\n            }\n" + PENDING_DUE), FIXTURE_REVERTED_ASSIST]),
    ("H3 withdrawn past held_until with the daemon here", [(PL, HERE_SPLIT, HERE_SPLIT + "    if asked.is_some_and(|(_, until)| seen.now_ms >= until) && matches!(answer, Reply::Pending) {\n        return Action::Withdraw;\n    }\n")]),
    # T3: the intent with the premise refused.
    ("I1 the intent waits where the premise is refused", [(PL, INTENT_TAKE, "            (None, DaemonSeen::Here { .. }) if !pool.premise => Action::Wait,\n" + INTENT_TAKE)]),
    ("I2 the intent gives up where the premise is refused", [(PL, INTENT_TAKE, "            (None, DaemonSeen::Here { .. }) if !pool.premise => Action::Abandon(Why::Premise),\n" + INTENT_TAKE)]),
    ("I1d I1 with T3's row dropped", [(PL, INTENT_TAKE, "            (None, DaemonSeen::Here { .. }) if !pool.premise => Action::Wait,\n" + INTENT_TAKE), T3_ROW_DROPPED]),
    # T4: ours with the hold or pool@t gone.
    ("U1 ours with the hold gone: Hold", [(PL, LOOK_OURS, "        Some(SnapSeen::Ours(_)) if !pool.held => Action::Hold,\n" + LOOK_OURS)]),
    ("U2 ours with the hold gone: the name given up", [(PL, LOOK_OURS, "        Some(SnapSeen::Ours(_)) if !pool.held => give_up(pool, Why::NameTaken),\n" + LOOK_OURS)]),
    ("U4 ours with pool@t gone: given up as destroyed", [(PL, LOOK_OURS, "        Some(SnapSeen::Ours(_)) if pool.snapshot.is_none() => give_up(pool, Why::Destroyed),\n" + LOOK_OURS)]),
    ("U1d U1 with T4's rows dropped", [(PL, LOOK_OURS, "        Some(SnapSeen::Ours(_)) if !pool.held => Action::Hold,\n" + LOOK_OURS), T4_ROWS_DROPPED]),
    ("U2d U2 with T4's rows dropped", [(PL, LOOK_OURS, "        Some(SnapSeen::Ours(_)) if !pool.held => give_up(pool, Why::NameTaken),\n" + LOOK_OURS), T4_ROWS_DROPPED]),
    ("U4d U4 with T4's rows dropped", [(PL, LOOK_OURS, "        Some(SnapSeen::Ours(_)) if pool.snapshot.is_none() => give_up(pool, Why::Destroyed),\n" + LOOK_OURS), T4_ROWS_DROPPED]),
    # T1: the record's format.
    ("R1 two fields of a publish swapped", [(RC,
        "    /// Whether the agent believes its hold placed on `pool@<name>`.\n    pub held: bool,\n    /// The requests made of the daemon for it.\n    pub attempts: u32,\n",
        "    /// The requests made of the daemon for it.\n    pub attempts: u32,\n    /// Whether the agent believes its hold placed on `pool@<name>`.\n    pub held: bool,\n")]),
    ("R2 a reason inserted first in Why", [(RC, "pub enum Why {\n", "pub enum Why {\n    /// Planted.\n    Planted,\n")]),
    ("R3 bytes after the body read", [(RC, "        if !left.is_empty() {\n", "        if false && !left.is_empty() {\n")]),
    ("R9 the version and flags big endian, written and read", [
        (RC, "    bytes.extend_from_slice(&VERSION.to_le_bytes());\n", "    bytes.extend_from_slice(&VERSION.to_be_bytes());\n"),
        (RC, "        let field = |at: usize| u16::from_le_bytes([head[at], head[at + 1]]);\n", "        let field = |at: usize| u16::from_be_bytes([head[at], head[at + 1]]);\n")]),
    ("R10 the body's length big endian, written and read", [
        (RC, "    bytes.extend_from_slice(&len.to_le_bytes());\n", "    bytes.extend_from_slice(&len.to_be_bytes());\n"),
        (RC, "        let said = u32::from_le_bytes([head[8], head[9], head[10], head[11]]);\n", "        let said = u32::from_be_bytes([head[8], head[9], head[10], head[11]]);\n")]),
    ("R11 the hash over the body alone, written and read", [
        (RC, "    let sum = blake3::hash(&bytes);\n", "    let sum = blake3::hash(body);\n"),
        (RC, "        if blake3::hash(&bytes[..HEAD + is]).as_bytes() != sum {\n", "        if blake3::hash(body).as_bytes() != sum {\n")]),
    ("R12 the count's after and load_guid swapped", [(RC,
        "    /// The cuts counted in it, C.\n    pub after: u64,\n    /// The pool's `load_guid`, which a new import changes (rule (L)).\n    pub load_guid: u64,\n",
        "    /// The pool's `load_guid`, which a new import changes (rule (L)).\n    pub load_guid: u64,\n    /// The cuts counted in it, C.\n    pub after: u64,\n")]),
    ("R13 the enrolled pool_guid and leaf_guid swapped", [(RC,
        "    /// The pool's GUID.\n    pub pool_guid: u64,\n    /// The data leaf's vdev GUID.\n    pub leaf_guid: u64,\n",
        "    /// The data leaf's vdev GUID.\n    pub leaf_guid: u64,\n    /// The pool's GUID.\n    pub pool_guid: u64,\n")]),
    ("R14 a name given up: why and at_ms swapped", [(RC,
        "    /// Why.\n    pub why: Why,\n    /// When (the wall clock, in ms since 1970).\n    pub at_ms: u64,\n",
        "    /// When (the wall clock, in ms since 1970).\n    pub at_ms: u64,\n    /// Why.\n    pub why: Why,\n")]),
    ("R15 the phases Taken and Held in the other order", [(RC,
        "    /// The snapshot taken and its manifest on disk.\n    Taken(Taken),\n    /// The hold placed, and the members, the layout and `load_guid`\n    /// verified.\n    Held(Taken),\n",
        "    /// The hold placed, and the members, the layout and `load_guid`\n    /// verified.\n    Held(Taken),\n    /// The snapshot taken and its manifest on disk.\n    Taken(Taken),\n")]),
    ("R16 asked_ms and held_until_ms swapped", [(RC,
        "        /// When the daemon last answered `accepted`.\n        asked_ms: u64,\n        /// The last `accepted`, plus the expiry, plus the channel's\n        /// margin.\n        held_until_ms: u64,\n",
        "        /// The last `accepted`, plus the expiry, plus the channel's\n        /// margin.\n        held_until_ms: u64,\n        /// When the daemon last answered `accepted`.\n        asked_ms: u64,\n")]),
    ("R17 the manifest's length and hash swapped", [(RC,
        "    /// The manifest's length in bytes.\n    pub manifest_len: u32,\n    /// The manifest's BLAKE3 hash.\n    pub manifest_hash: [u8; 32],\n",
        "    /// The manifest's BLAKE3 hash.\n    pub manifest_hash: [u8; 32],\n    /// The manifest's length in bytes.\n    pub manifest_len: u32,\n")]),
    ("R18 another magic", [(RC, "pub const MAGIC: [u8; 4] = *b\"SZAG\";\n", "pub const MAGIC: [u8; 4] = *b\"SZAH\";\n")]),
    ("R19 attempts written at a fixed width", [(RC,
        "    /// The requests made of the daemon for it.\n    pub attempts: u32,\n",
        "    /// The requests made of the daemon for it.\n    #[serde(with = \"postcard::fixint::le\")]\n    pub attempts: u32,\n")]),
    ("R20 two reasons of Why in the other order", [(RC,
        "    /// `pool@<name>` was destroyed before it was published.\n    Destroyed,\n    /// A member's `@<name>` is not the one the manifest names (destroyed,\n    /// or destroyed and taken again).\n    Replaced,\n",
        "    /// A member's `@<name>` is not the one the manifest names (destroyed,\n    /// or destroyed and taken again).\n    Replaced,\n    /// `pool@<name>` was destroyed before it was published.\n    Destroyed,\n")]),
    ("R21 a new reason appended to Why (version 1 still reads)", [(RC, "    /// A rewind of the pool lost `pool@<name>`.\n    Rewound,\n}\n", "    /// A rewind of the pool lost `pool@<name>`.\n    Rewound,\n    /// Planted.\n    Planted,\n}\n")]),
    # 347a17f (Codex's second P2): the directory that holds a new one.
    ("D1 the old behaviour: an empty parent not synced", [(RC,
        "        Some(parent) if parent.as_os_str().is_empty() => Some(Path::new(\".\")),\n",
        "        Some(parent) if parent.as_os_str().is_empty() => None,\n")]),
    ("D2 make_dir's old body back (parent_of unused)", [(RC, MAKE_DIR_SYNC,
        "    match dir.parent() {\n        Some(parent) if !parent.as_os_str().is_empty() => File::open(parent)?.sync_all(),\n        _ => Ok(()),\n    }\n")]),
    ("D3 make_dir syncs the new directory, not the one that holds it", [(RC, MAKE_DIR_SYNC,
        "    match parent_of(dir) {\n        Some(_) => File::open(dir)?.sync_all(),\n        None => Ok(()),\n    }\n")]),
    ("D4 make_dir syncs nothing", [(RC, MAKE_DIR_SYNC,
        "    match parent_of(dir) {\n        Some(_) => Ok(()),\n        None => Ok(()),\n    }\n")]),
    ("D5 parent_of gives . for every relative path", [(RC, "        parent => parent,\n",
        "        _ if dir.is_relative() => Some(Path::new(\".\")),\n        parent => parent,\n")]),
    ("D6 parent_of gives / for a root", [(RC, "        parent => parent,\n",
        "        None => Some(Path::new(\"/\")),\n        parent => parent,\n")]),
]

only = sys.argv[1:]
res = []
for name, edits in P:
    if only and not any(name.startswith(o + " ") or name == o for o in only):
        continue
    saved = {}
    bad = None
    for f, old, new in edits:
        path = os.path.join(W, f)
        if path not in saved:
            with open(path) as h:
                saved[path] = h.read()
    texts = dict(saved)
    for f, old, new in edits:
        path = os.path.join(W, f)
        n = texts[path].count(old)
        if n != 1:
            bad = f"NOT APPLIED ({f}: found {n})"
            break
        texts[path] = texts[path].replace(old, new)
    if bad:
        res.append((name, bad))
        print(f"{name}: {bad}", flush=True)
        continue
    t = time.time()
    verdict = "SURVIVED"
    try:
        for path, text in texts.items():
            with open(path, "w") as h:
                h.write(text)
        try:
            r = subprocess.run(SUITE, cwd=W, env=ENV, capture_output=True, text=True, timeout=600)
        except subprocess.TimeoutExpired:
            r = None
            verdict = "KILLED (timeout, a hang)"
        if r is not None:
            out = r.stdout + r.stderr
            if "error[E" in out or "could not compile" in out:
                verdict = "BUILD ERROR"
                print(out[-3000:])
            elif r.returncode != 0:
                failed = [l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.endswith("FAILED")]
                verdict = "KILLED: " + ", ".join(failed[:4]) + (f" and {len(failed) - 4} more" if len(failed) > 4 else "")
    finally:
        for path, text in saved.items():
            with open(path, "w") as h:
                h.write(text)
    res.append((name, verdict))
    print(f"{name}: {verdict} ({time.time() - t:.0f}s)", flush=True)
    with open(OUT, "w") as h:
        json.dump(res, h, indent=1)
print("done")
