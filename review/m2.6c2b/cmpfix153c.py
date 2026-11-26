"""Compare a committed fixture (full-<image>.json at the head) with the
recorder's artifact: its composed JSON and the raw per-record files."""
import json, subprocess, sys, pathlib
repo, ref, image, art = sys.argv[1], sys.argv[2], sys.argv[3], pathlib.Path(sys.argv[4])
path = f"crates/skyzfs-agent/src/zfs/cli/recorded/full-{image}.json"
text = subprocess.run(["git", "-C", repo, "show", f"{ref}:{path}"], capture_output=True, check=True).stdout
fix = json.loads(text)
whole_text = (art / f"recorded-{image}.json").read_bytes()
whole = json.loads(whole_text)
ok = True
print("fixture records", len(fix["records"]), "artifact records", len(whole["records"]))
print("source starts with the artifact's:", fix["source"].startswith(whole["source"]), "| suffix:", fix["source"][len(whole["source"]):])
for k, r in fix["records"].items():
    w = whole["records"].get(k)
    same = (w == r)
    raw = {}
    d = art / k
    for f in ("cmd", "rc", "stdout", "stderr"):
        p = d / f
        raw[f] = p.read_bytes() if p.exists() else None
    rawsame = (raw["cmd"] is not None and raw["cmd"].decode() == r["cmd"]
               and int(raw["rc"].decode().strip()) == r["rc"]
               and raw["stdout"].decode() == r["stdout"] and raw["stderr"].decode() == r["stderr"])
    extra = sorted(set(r) - {"cmd", "rc", "stdout", "stderr"})
    if not (same and rawsame) or extra:
        ok = False
    print(f"  {k}: same as composed JSON {same}; same as raw files {rawsame}; other keys {extra}")
# The fixture's text: the artifact's JSON cut to these records, in the same layout?
sub = {"records": {k: whole["records"][k] for k in fix["records"]}, "source": fix["source"]}
for indent in (1, 2):
    for ea in (True, False):
        cand = (json.dumps(sub, indent=indent, ensure_ascii=ea, sort_keys=True) + "\n").encode()
        if cand == text:
            print(f"fixture text = artifact JSON cut to these keys, indent={indent} ensure_ascii={ea} sort_keys")
# The artifact's own layout
for indent in (1, 2):
    for ea in (True, False):
        if (json.dumps(whole, indent=indent, ensure_ascii=ea, sort_keys=True) + "\n").encode() == whole_text:
            print(f"artifact JSON layout: indent={indent} ensure_ascii={ea} sort_keys")
# Which records of the kinds kept are left out?
kinds = ("count", "members-full", "version", "nocacheflush")
left = [k for k in whole["records"] if any(x in k for x in kinds) and k not in fix["records"]]
print("records of those kinds left out:", left)
print("ALL SAME" if ok else "DIFFERENCES")
