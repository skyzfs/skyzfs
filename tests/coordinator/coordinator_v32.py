#!/usr/bin/env python3
"""Reference V3.2 coordinator — Phase A of the snapshot design
(docs/skynbd-snapshot-design-v3_2.md), driven against the skynbd
ingestion control plane (server design §9A). Scenario S14's engine.

Implements the V3.2 loop on a real dm-thin stack:

  bootstrap:  fsfreeze → lvcreate -s S0 → thaw → full objectize from S0
              → publish G0 (pinned) → promote baseline B0
  cycle n:    fsfreeze → lvcreate -s Sn → thaw
              → reserve_metadata_snap → thin_delta(B_{n-1}, Sn) → release
              → objectize different/right_only from Sn, ClearRange left_only
              → publish Gn → verify blake3(Sn) == blake3(Gn export)
              → pin transfer (V3.2 §15.4) → promote Sn to baseline

Design constraints honored:
  - object_chunk_size == thin_chunk_size (64 KiB), so left_only holes are
    whole-chunk and ClearRange is always safe (V3.2 §9.6).
  - all data reads go through the activated read-only snapshot device,
    never the origin (V3.2 §10.4); snapshots activated -K -pr (§9.3).
  - freeze window is timed and bounded (§7.11): capture is metadata-only.
  - XML thin_delta output is acceptable for a test coordinator (§10.1).

Usage:
  coordinator_v32.py bootstrap --api URL --vol NAME --vg VG --origin LV \
      --pool POOL --mount DIR --state FILE
  coordinator_v32.py cycle     (same args)
  coordinator_v32.py selftest              # no root: parser/chunking tests
"""

import json
import os
import subprocess
import sys
import time
import urllib.request
import xml.etree.ElementTree as ET

CHUNK = 64 * 1024  # == thin chunk == object chunk (design §8.2 alignment)
FREEZE_BUDGET_S = 2.0  # capture must be metadata-only (V3.2 §16.1)


def blake3_hasher():
    try:
        from blake3 import blake3
        return blake3()
    except ImportError:
        sys.exit("pip install blake3")


def sh(*cmd, check=True):
    r = subprocess.run(cmd, capture_output=True, text=True)
    if check and r.returncode != 0:
        raise RuntimeError(f"{' '.join(cmd)}: {r.stderr.strip()}")
    return r.stdout


def api_req(method, url, data=None, ctype="application/json"):
    r = urllib.request.Request(url, data=data, method=method)
    if data is not None:
        r.add_header("content-type", ctype)
    with urllib.request.urlopen(r) as resp:
        body = resp.read()
        return json.loads(body) if body else None


class Coordinator:
    def __init__(self, api, vol, vg, pool, origin, mount, state_path):
        self.api, self.vol = api, vol
        self.vg, self.pool, self.origin, self.mount = vg, pool, origin, mount
        self.state_path = state_path
        self.state = (json.load(open(state_path))
                      if os.path.exists(state_path) else
                      {"generation": -1, "baseline_lv": None,
                       "baseline_snap": None})

    # ------------------------------------------------------------- local

    def save(self):
        json.dump(self.state, open(self.state_path, "w"))

    def capture(self, name):
        """fsfreeze → thin snapshot → thaw. The V3.2 §9.3 window."""
        t0 = time.monotonic()
        sh("fsfreeze", "--freeze", self.mount)
        try:
            sh("lvcreate", "-s", "-n", name, f"{self.vg}/{self.origin}")
        finally:
            sh("fsfreeze", "--unfreeze", self.mount)
        window = time.monotonic() - t0
        # Enforced immutability: activate read-only (V3.2 §7.3, §9.3).
        sh("lvchange", "-ay", "-K", f"{self.vg}/{name}")
        sh("lvchange", "-pr", f"{self.vg}/{name}", check=False)
        print(f"freeze window: {window * 1000:.0f}ms")
        if window > FREEZE_BUDGET_S:
            raise RuntimeError(
                f"freeze window {window:.2f}s exceeds budget (V3.2 §7.11)")
        return f"/dev/{self.vg}/{name}"

    def thin_id(self, lv):
        return sh("lvs", "--noheadings", "-o", "thin_id",
                  f"{self.vg}/{lv}").strip()

    def thin_delta(self, left_lv, right_lv):
        """Mapping delta via a reserved metadata snapshot (V3.2 §10.2)."""
        dm = f"{self.vg.replace('-', '--')}-{self.pool.replace('-', '--')}"
        tpool, tmeta = f"{dm}-tpool", f"/dev/mapper/{dm}_tmeta"
        sh("dmsetup", "message", tpool, "0", "reserve_metadata_snap")
        try:
            xml_out = sh("thin_delta", "--metadata-snap",
                         "--snap1", self.thin_id(left_lv),
                         "--snap2", self.thin_id(right_lv), tmeta)
        finally:
            sh("dmsetup", "message", tpool, "0", "release_metadata_snap")
        return xml_out

    # ------------------------------------------------------------ remote

    def prepare(self, base):
        r = api_req("POST", f"{self.api}/v1/volumes/{self.vol}/prepared",
                    json.dumps({"base": base}).encode())
        return r["snap_id"]

    def objectize(self, pid, dev, ranges, start_op=1):
        """Read chunks from the snapshot device (never the origin, V3.2
        §10.4), EnsureObject-dedupe, PutObject the misses, AttachRange."""
        hasher_mod = __import__("blake3")
        op = start_op
        with open(dev, "rb", buffering=0) as f:
            for off, length in ranges:
                pos = off
                while pos < off + length:
                    clen = min(CHUNK, off + length - pos)
                    f.seek(pos)
                    data = f.read(clen)
                    h = hasher_mod.blake3(data).hexdigest()
                    r = api_req(
                        "POST",
                        f"{self.api}/v1/volumes/{self.vol}/prepared/{pid}/ensure",
                        json.dumps({"hashes": [h]}).encode())
                    if not r["exists"][0]:
                        api_req("PUT",
                                f"{self.api}/v1/volumes/{self.vol}/objects/{h}",
                                data, "application/octet-stream")
                    api_req(
                        "POST",
                        f"{self.api}/v1/volumes/{self.vol}/prepared/{pid}/attach",
                        json.dumps({"ranges": [{"off": pos, "len": clen,
                                                "hash": h}],
                                    "op_id": op}).encode())
                    op += 1
                    pos += clen
        return op

    def clear(self, pid, ranges, op):
        for off, length in ranges:
            api_req("POST",
                    f"{self.api}/v1/volumes/{self.vol}/prepared/{pid}/clear",
                    json.dumps({"ranges": [{"off": off, "len": length}],
                                "op_id": op}).encode())
            op += 1
        return op

    def publish(self, pid, name):
        return api_req(
            "POST",
            f"{self.api}/v1/volumes/{self.vol}/prepared/{pid}/publish",
            json.dumps({"name": name, "holder": "coordinator"}).encode())

    def verify(self, dev, snap_name, nbd_port):
        """hash(S_n) == hash(G_n) — V3.2 §10.7, the S14 gate."""
        h_local = blake3_hasher()
        with open(dev, "rb", buffering=0) as f:
            while True:
                b = f.read(1 << 20)
                if not b:
                    break
                h_local.update(b)
        dump = f"{self.state_path}.dump"
        sh("qemu-img", "convert", "-f", "raw", "-O", "raw",
           f"nbd://127.0.0.1:{nbd_port}/{self.vol}/snap/{snap_name}", dump)
        h_remote = blake3_hasher()
        with open(dump, "rb") as f:
            while True:
                b = f.read(1 << 20)
                if not b:
                    break
                h_remote.update(b)
        os.unlink(dump)
        if h_local.hexdigest() != h_remote.hexdigest():
            raise RuntimeError(
                f"VERIFY FAILED: hash(S_n) != hash(G_n) for {snap_name}")
        print(f"verify {snap_name}: hash(S_n) == hash(G_n) "
              f"[{h_local.hexdigest()[:16]}…]")

    def promote(self, new_lv, new_snap):
        """Baseline promotion per V3.2 §15.4: pin as baseline, release
        old holders, then delete the old local baseline."""
        api_req("POST",
                f"{self.api}/v1/volumes/{self.vol}/snapshots/{new_snap}/pins",
                json.dumps({"holder": "baseline"}).encode())
        old_lv, old_snap = self.state["baseline_lv"], self.state["baseline_snap"]
        if old_snap:
            api_req("DELETE",
                    f"{self.api}/v1/volumes/{self.vol}/snapshots/{old_snap}"
                    "/pins/baseline")
        api_req("DELETE",
                f"{self.api}/v1/volumes/{self.vol}/snapshots/{new_snap}"
                "/pins/coordinator")
        self.state.update(baseline_lv=new_lv, baseline_snap=new_snap)
        self.save()
        if old_lv:
            sh("lvremove", "-f", f"{self.vg}/{old_lv}")

    # ------------------------------------------------------------ phases

    def bootstrap(self, nbd_port):
        n = 0
        lv = f"S{n}"
        dev = self.capture(lv)
        pid = self.prepare(base=None)
        # Full objectize: every non-zero chunk of S0 (zero chunks are
        # implicit holes server-side — identical read semantics).
        size = int(sh("blockdev", "--getsize64", dev))
        ranges, off = [], 0
        with open(dev, "rb", buffering=0) as f:
            while off < size:
                data = f.read(CHUNK)
                if data.strip(b"\x00"):
                    ranges.append((off, len(data)))
                off += len(data)
        self.objectize(pid, dev, ranges)
        self.publish(pid, f"g{n}")
        time.sleep(0.3)  # export publication
        self.verify(dev, f"g{n}", nbd_port)
        self.promote(lv, f"g{n}")
        self.state["generation"] = n
        self.save()
        print(json.dumps({"bootstrap": f"g{n}", "chunks": len(ranges)}))

    def cycle(self, nbd_port):
        n = self.state["generation"] + 1
        lv, base_lv = f"S{n}", self.state["baseline_lv"]
        dev = self.capture(lv)
        xml_out = self.thin_delta(base_lv, lv)
        data_ranges, clear_ranges, stats = parse_delta(xml_out)
        print(f"thin_delta: {stats}")
        pid = self.prepare(base=self.state["baseline_snap"])
        op = self.objectize(pid, dev, data_ranges)
        self.clear(pid, clear_ranges, op)
        self.publish(pid, f"g{n}")
        time.sleep(0.3)
        self.verify(dev, f"g{n}", nbd_port)
        self.promote(lv, f"g{n}")
        self.state["generation"] = n
        self.save()
        print(json.dumps({"published": f"g{n}",
                          "data_bytes": sum(l for _, l in data_ranges),
                          "cleared_bytes": sum(l for _, l in clear_ranges)}))


def parse_delta(xml_text):
    """thin_delta XML → (data_ranges, clear_ranges, stats) in bytes.
    different/right_only → objectize from S_n; left_only → ClearRange
    (whole-chunk safe because object chunk == thin chunk, V3.2 §9.6)."""
    root = ET.fromstring(xml_text)
    block = int(root.get("data_block_size")) * 512
    data_ranges, clear_ranges = [], []
    stats = {"same": 0, "different": 0, "right_only": 0, "left_only": 0}
    diff = root.find("diff") if root.tag == "superblock" else root
    for el in diff:
        begin = int(el.get("begin")) * block
        length = int(el.get("length")) * block
        stats[el.tag] = stats.get(el.tag, 0) + length
        if el.tag in ("different", "right_only"):
            data_ranges.append((begin, length))
        elif el.tag == "left_only":
            clear_ranges.append((begin, length))
    return data_ranges, clear_ranges, stats


def selftest():
    xml_text = """<superblock uuid="" time="1" transaction="3"
        data_block_size="128" nr_data_blocks="1024">
      <diff left="1" right="2">
        <same begin="0" length="4"/>
        <different begin="4" length="2"/>
        <right_only begin="10" length="1"/>
        <left_only begin="20" length="3"/>
      </diff></superblock>"""
    data, clear, stats = parse_delta(xml_text)
    blk = 128 * 512
    assert data == [(4 * blk, 2 * blk), (10 * blk, 1 * blk)], data
    assert clear == [(20 * blk, 3 * blk)], clear
    assert stats["same"] == 4 * blk and stats["left_only"] == 3 * blk
    # Chunking invariant: object chunk == thin chunk means delta blocks
    # land exactly on object boundaries.
    assert blk == CHUNK
    print("selftest OK")


def main():
    if len(sys.argv) >= 2 and sys.argv[1] == "selftest":
        return selftest()
    import argparse
    ap = argparse.ArgumentParser()
    ap.add_argument("phase", choices=["bootstrap", "cycle"])
    for a in ("--api", "--vol", "--vg", "--pool", "--origin", "--mount",
              "--state"):
        ap.add_argument(a, required=True)
    ap.add_argument("--nbd-port", required=True, type=int)
    args = ap.parse_args()
    c = Coordinator(args.api, args.vol, args.vg, args.pool, args.origin,
                    args.mount, args.state)
    if args.phase == "bootstrap":
        c.bootstrap(args.nbd_port)
    else:
        c.cycle(args.nbd_port)


if __name__ == "__main__":
    main()
