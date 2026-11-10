#!/usr/bin/env python3
"""Minimal V3.2-style test coordinator for e2e scenarios S11-S13.

Drives the skynbd ingestion control plane (design §9A) over HTTP:
chunks a local file into object_chunk_size CAS objects, EnsureObject-
dedupes, PutObject + AttachRange, and publishes. This is the Tier A
stand-in for the real dm-thin coordinator (M7 runs the full V3.2 loop).

Usage:
  coordinator.py upload   <api> <vol> <file> <snap_name> [--base NAME]
  coordinator.py resume   <api> <vol> <prepared_id> <file> <snap_name> \
      [--from-op N]
  coordinator.py prepare  <api> <vol> [--base NAME]           -> prints id
  coordinator.py attach-half <api> <vol> <prepared_id> <file> -> attaches
      the first half of the file's chunks (for crash tests), prints next op
"""

import json
import os
import sys
import urllib.request

try:
    from blake3 import blake3
except ImportError:
    sys.exit("pip install blake3")

CHUNK = 64 * 1024


def req(method, url, data=None, ctype="application/json"):
    r = urllib.request.Request(url, data=data, method=method)
    tok = os.environ.get("SKYNBD_TOKEN")
    if tok:
        r.add_header("authorization", f"Bearer {tok}")
    if data is not None:
        r.add_header("content-type", ctype)
    with urllib.request.urlopen(r) as resp:
        body = resp.read()
        return resp.status, json.loads(body) if body else None


def chunks_of(path):
    out = []
    data = open(path, "rb").read()
    for off in range(0, len(data), CHUNK):
        c = data[off:off + CHUNK]
        out.append((off, c, blake3(c).hexdigest()))
    return out


def put_missing(api, vol, pid, chunk_list):
    hashes = [h for _, _, h in chunk_list]
    _, r = req("POST", f"{api}/v1/volumes/{vol}/prepared/{pid}/ensure",
               json.dumps({"hashes": hashes}).encode())
    missing = [c for c, exists in zip(chunk_list, r["exists"]) if not exists]
    for _, data, h in missing:
        req("PUT", f"{api}/v1/volumes/{vol}/objects/{h}", data,
            "application/octet-stream")
    return len(chunk_list) - len(missing)


def attach_all(api, vol, pid, chunk_list, start_op=1):
    op = start_op
    for off, data, h in chunk_list:
        body = {"ranges": [{"off": off, "len": len(data), "hash": h}],
                "op_id": op}
        req("POST", f"{api}/v1/volumes/{vol}/prepared/{pid}/attach",
            json.dumps(body).encode())
        op += 1
    return op


def main():
    cmd, api, vol = sys.argv[1], sys.argv[2], sys.argv[3]
    if cmd == "prepare":
        base = sys.argv[sys.argv.index("--base") + 1] if "--base" in sys.argv else None
        _, r = req("POST", f"{api}/v1/volumes/{vol}/prepared",
                   json.dumps({"base": base}).encode())
        print(r["snap_id"])
    elif cmd == "upload":
        path, name = sys.argv[4], sys.argv[5]
        base = sys.argv[sys.argv.index("--base") + 1] if "--base" in sys.argv else None
        _, r = req("POST", f"{api}/v1/volumes/{vol}/prepared",
                   json.dumps({"base": base}).encode())
        pid = r["snap_id"]
        cl = chunks_of(path)
        deduped = put_missing(api, vol, pid, cl)
        attach_all(api, vol, pid, cl)
        _, r = req("POST", f"{api}/v1/volumes/{vol}/prepared/{pid}/publish",
                   json.dumps({"name": name, "holder": "coordinator"}).encode())
        print(json.dumps({"published": name, "commit": r["commit"],
                          "deduped_objects": deduped, "chunks": len(cl)}))
    elif cmd == "attach-half":
        pid, path = sys.argv[4], sys.argv[5]
        cl = chunks_of(path)
        half = cl[: len(cl) // 2]
        put_missing(api, vol, pid, half)
        next_op = attach_all(api, vol, pid, half)
        print(next_op)
    elif cmd == "resume":
        pid, path, name = sys.argv[4], sys.argv[5], sys.argv[6]
        _, st = req("GET", f"{api}/v1/volumes/{vol}/prepared/{pid}")
        watermark = st["op_watermark"]
        cl = chunks_of(path)
        put_missing(api, vol, pid, cl)
        # Replay everything above the durable watermark (idempotent below).
        attach_all(api, vol, pid, cl, start_op=1)
        _, r = req("POST", f"{api}/v1/volumes/{vol}/prepared/{pid}/publish",
                   json.dumps({"name": name, "holder": "coordinator"}).encode())
        print(json.dumps({"resumed_from_watermark": watermark,
                          "published": name, "commit": r["commit"]}))
    else:
        sys.exit(f"unknown command {cmd}")


if __name__ == "__main__":
    main()
