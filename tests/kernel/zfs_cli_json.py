#!/usr/bin/env python3
"""The recorder's records as the agent's fixture (#147's
`crates/skyzfs-agent/src/zfs/cli/recorded/<image>.json`).

    zfs_cli_json.py OUT SOURCE > IMAGE.json

OUT is zfs-cli-record.sh's output directory. Every record (a directory
with an `rc` file) but the scale pools' (`scale-*`, whose outputs run to
megabytes) becomes `records[NAME]`: `cmd` (as the recorder quoted it for
bash), `rc` (an integer), `stdout` and `stderr`, and `note` where the
record has one (what an `--expect` or a catch found, "not caught" among
them). An output that is not UTF-8 is kept as text with U+FFFD in
place of what is not, and in full as hex in `stdout_hex` or
`stderr_hex`. SOURCE is the fixture's `source`
line: the recorder's commit, the run and the artifact. Exit 2 when OUT
holds no record.
"""
import json
import os
import sys


def text(raw):
    """-> (text, hex or None)."""
    try:
        return raw.decode("utf-8"), None
    except UnicodeDecodeError:
        return raw.decode("utf-8", errors="replace"), raw.hex()


def records(out):
    got = {}
    for name in sorted(os.listdir(out)):
        d = os.path.join(out, name)
        if name.startswith(("scale-", ".")) or not os.path.isfile(os.path.join(d, "rc")):
            continue
        with open(os.path.join(d, "rc")) as f:
            rc = f.read().strip()
        rec = {"cmd": "", "rc": int(rc) if rc.lstrip("-").isdigit() else rc}
        with open(os.path.join(d, "cmd"), "rb") as f:
            rec["cmd"] = text(f.read())[0]
        for stream in ("stdout", "stderr"):
            try:
                with open(os.path.join(d, stream), "rb") as f:
                    rec[stream], raw = text(f.read())
            except FileNotFoundError:
                rec[stream], raw = "", None
            if raw is not None:
                rec[stream + "_hex"] = raw
        try:
            with open(os.path.join(d, "note"), "rb") as f:
                rec["note"] = text(f.read())[0]
        except FileNotFoundError:
            pass
        got[name] = rec
    return got


def main(argv):
    if len(argv) != 2:
        print(__doc__, file=sys.stderr)
        return 64
    got = records(argv[0])
    if not got:
        print("no record in " + argv[0], file=sys.stderr)
        return 2
    json.dump({"records": got, "source": argv[1]}, sys.stdout, indent=1, sort_keys=True, ensure_ascii=False)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
