"""Plants for #82, one at a time; each file is restored after."""
import subprocess, sys, pathlib, re
root = pathlib.Path(sys.argv[1])
lib, rep, hole, bench = "tests/lib/skz-store.sh", "tests/kernel/replay-rig.sh", "tests/kernel/hole-rig.sh", "tests/bench/topology-bench.sh"
rep_check = '''if [ -n "$SKYZFSD" ] && ! skz_store_args "$SKYZFSD" "${SERVE_ARGS[@]}"; then
    rig_error "RIG_SERVE_ARGS (${RIG_SERVE_ARGS:-}): $SKZ_STORE_WHY"
fi
'''
plants = [
  ("P1 the help probe never finds [--store", [(lib, "[[ $help != *'[--store'* ]] || choice=1", "[[ $help != *'[--store-never'* ]] || choice=1")]),
  ("P2 the value of --store is not checked", [(lib, 'if [ "$v" != in-place ]; then', 'if false; then')]),
  ("P3 a log option is matched only without =VALUE", [(lib, '*" ${o%%=*} "*)', '*" $o "*)')]),
  ("P4 a named --store in-place is not remembered", [(lib, "                named=1 ;;", "                : ;;")]),
  ("P5 the replay rig's check comes after its trap and modprobe", [(rep, rep_check, ""),
      (rep, "for m in ublk_drv dm_log_writes dm_snapshot zfs; do modprobe \"$m\" 2>/dev/null || true; done\n",
            "for m in ublk_drv dm_log_writes dm_snapshot zfs; do modprobe \"$m\" 2>/dev/null || true; done\n" + rep_check)]),
  ("P6 the hole rig ends with 64, not its own error 2", [(hole, "    echo \"RIG_SERVE_ARGS (${RIG_SERVE_ARGS:-}): $SKZ_STORE_WHY\" >&2\n    exit 2\n", "    echo \"RIG_SERVE_ARGS (${RIG_SERVE_ARGS:-}): $SKZ_STORE_WHY\" >&2\n    exit 64\n")]),
  ("P7 the bench's early check removed", [(bench, '            skz_store_args "$SKYZFSD" "${SKZ_CHECK_ARGS[@]}" || { echo "TB_SKYZFSD_ARGS ($SKYZFSD_ARGS): $SKZ_STORE_WHY" >&2; exit 64; }\n', "")]),
]
for name, edits in plants:
    origs = {}
    try:
        for path, a, b in edits:
            f = root / path
            if path not in origs: origs[path] = f.read_text()
            t = f.read_text(); assert t.count(a) == 1, (name, a[:50]); f.write_text(t.replace(a, b))
        p = subprocess.run(["bash", "tests/lib/test_skz_store.sh"], cwd=root, capture_output=True, text=True, timeout=600)
        fails = [l[:150] for l in p.stdout.splitlines() if l.startswith("FAIL")]
        print(f"{name}: rc {p.returncode}; {len(fails)} failed", flush=True)
        for l in fails[:4]: print("   ", l)
    finally:
        for path, t in origs.items(): (root / path).write_text(t)
