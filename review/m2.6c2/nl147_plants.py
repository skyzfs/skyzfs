#!/usr/bin/env python3
"""Next look at #147: plants, each applied alone to a clean checkout of
the head, then the crate's own tests (`cargo test -p CRATE --lib`), then
restored. One JSON line per plant on stdout.

usage: python3 -I nl147_plants.py WORKTREE
Run as root, so that the client's test that needs root runs; the CI
runners are not root, where that test returns at once (see the note).
"""
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(sys.argv[1])

AG = "crates/skyzfs-agent/src"
CT = "crates/skyzfs-control/src"

PLANTS = [
    # A sample of the author's 21 (the answer at 0f709ae).
    ("P2", "the author's: a result of the wrong shape not destroyed", "skyzfs-agent", f"{AG}/zfs/cli.rs",
     "            Err(ZfsError::Other(why)) if !returned.contains_key(\"failed\") => {\n                Err(self.destroy_taken(pool, t, &why))\n            }\n",
     ""),
    ("P4", "the author's: a reported failure destroyed too", "skyzfs-agent", f"{AG}/zfs/cli.rs",
     "if !returned.contains_key(\"failed\") =>", "if true =>"),
    ("P6", "the author's: hold, a member without the tag missed", "skyzfs-agent", f"{AG}/zfs/cli.rs",
     "        if lacking.is_empty() {\n            Ok(())", "        if lacking.len() <= 1 {\n            Ok(())"),
    ("P10", "the author's: no group of its own", "skyzfs-agent", f"{AG}/host.rs",
     "            .process_group(0)\n", ""),
    ("P11", "the author's: the count never taken down", "skyzfs-agent", f"{AG}/host.rs",
     "        STUCK_NOW.fetch_sub(1, Ordering::SeqCst);\n", ""),
    ("P12", "the author's: the cap off by one", "skyzfs-agent", f"{AG}/host.rs",
     "if held >= STUCK_MAX {", "if held > STUCK_MAX {"),
    ("P13", "the author's: the path read through a link", "skyzfs-control", f"{CT}/client.rs",
     "std::fs::symlink_metadata(path)?", "std::fs::metadata(path)?"),
    ("P14", "the author's: the peer not read", "skyzfs-control", f"{CT}/client.rs",
     "    check_peer(path, &stream)?;\n", ""),
    ("P15", "the author's: the owner not read", "skyzfs-control", f"{CT}/client.rs",
     "    if !trusted(meta.uid()) {", "    if false && !trusted(meta.uid()) {"),
    ("P19", "the author's: a sign taken as hex", "skyzfs-agent", f"{AG}/manifest.rs",
     " || !text.bytes().all(|b| b.is_ascii_hexdigit())", ""),
    ("P20", "the author's: messages matched anywhere (S1)", "skyzfs-agent", f"{AG}/zfs/cli.rs",
     "        let message = line.rsplit_once(\": \").map_or(line, |(_, message)| message);\n        message.contains(what)",
     "        line.contains(what)"),
    # The review's own.
    ("N1", "manifest: 0x7f taken as printable", "skyzfs-agent", f"{AG}/manifest.rs",
     "(0x20..=0x7e).contains(b)", "(0x20..=0x7f).contains(b)"),
    ("N2", "adapter: hex of any case taken (no round trip to lower case)", "skyzfs-agent", f"{AG}/zfs/cli.rs",
     "Some(bytes) if manifest::hex(&bytes) == hex && !manifest::plain(&bytes) =>",
     "Some(bytes) if !manifest::plain(&bytes) =>"),
    ("N3", "adapter: hex of a printable value taken", "skyzfs-agent", f"{AG}/zfs/cli.rs",
     "Some(bytes) if manifest::hex(&bytes) == hex && !manifest::plain(&bytes) =>",
     "Some(bytes) if manifest::hex(&bytes) == hex =>"),
    ("N4", "destroy without -r (the members' @t left)", "skyzfs-agent", f"{AG}/zfs/cli.rs",
     "let args = [\"destroy\", \"-r\", snapshot.as_str()];", "let args = [\"destroy\", snapshot.as_str()];"),
    ("N5", "destroy: any failure read as `was not taken`", "skyzfs-agent", f"{AG}/zfs/cli.rs",
     "                e => format!(\"{snapshot} could not be destroyed: {e}\"),",
     "                _ => format!(\"{snapshot} was not taken\"),"),
    ("N6", "props: `missing pool name` read as NoPool with no look", "skyzfs-agent", f"{AG}/zfs/cli.rs",
     "            if matches!(error, ZfsError::Other(_)) && says(&output.stderr, \"missing pool name\") {",
     "            if matches!(error, ZfsError::Other(_)) && says(&output.stderr, \"missing pool name\") {\n                return Err(ZfsError::NoPool);\n            }\n            if false {"),
    ("N7", "hold: the holds' tag field not compared", "skyzfs-agent", f"{AG}/zfs/cli.rs",
     "                    (fields.next() == Some(tag)).then(|| name.to_string())",
     "                    { let _ = fields.next(); let _ = tag; Some(name.to_string()) }"),
    ("N8", "reaper: count taken down before the wait (a child stuck in the kernel uncounted)", "skyzfs-agent", f"{AG}/host.rs",
     "        let _ = child.wait();\n        STUCK_NOW.fetch_sub(1, Ordering::SeqCst);",
     "        STUCK_NOW.fetch_sub(1, Ordering::SeqCst);\n        let _ = child.wait();"),
    ("N9", "runner: the group not killed on try_wait's error", "skyzfs-agent", f"{AG}/host.rs",
     "                Err(e) => {\n                    abandon(child);\n                    return Err(e);",
     "                Err(e) => {\n                    return Err(e);"),
    ("N10", "client: root no longer trusted (own user only)", "skyzfs-control", f"{CT}/client.rs",
     "uid == 0 || uid == rustix::process::geteuid().as_raw()", "uid == rustix::process::geteuid().as_raw()"),
    ("N11", "client: any peer trusted", "skyzfs-control", f"{CT}/client.rs",
     "uid == 0 || uid == rustix::process::geteuid().as_raw()", "{ let _ = uid; true }"),
    ("N12", "says: first `: ` instead of the last", "skyzfs-agent", f"{AG}/zfs/cli.rs",
     "line.rsplit_once(\": \")", "line.split_once(\": \")"),
]


def run(crate):
    p = subprocess.run(
        ["cargo", "test", "-p", crate, "--lib", "--", "--test-threads=4"],
        cwd=ROOT, capture_output=True, text=True, timeout=1800,
    )
    out = p.stdout + p.stderr
    failed = [l.split(" ")[1] for l in out.splitlines() if l.startswith("test ") and l.endswith("FAILED")]
    if p.returncode != 0 and not failed:
        failed = ["(build failed)"] if "error[" in out or "error:" in out else ["(exit %d)" % p.returncode]
    return p.returncode, failed


for pid, what, crate, rel, old, new in PLANTS:
    path = ROOT / rel
    text = path.read_text()
    n = text.count(old)
    if n != 1:
        print(json.dumps({"id": pid, "what": what, "error": f"site found {n} times"}), flush=True)
        continue
    path.write_text(text.replace(old, new))
    try:
        code, failed = run(crate)
    finally:
        path.write_text(text)
    print(json.dumps({"id": pid, "what": what, "crate": crate,
                      "result": "caught" if code != 0 else "survives", "failed": failed}), flush=True)
