#!/usr/bin/env python3
"""Review scratch (Q10, PR #152): each plant alone on a fresh copy of the
head's tests/ tree, then the tests that cover the planted file. KILLED =
a test exits non-zero; SURVIVED = every test passes with the plant in
place. Each plant is an exact text replacement that must occur once in
the file (else NOT APPLIED). The copy is rebuilt for each plant and
removed at the end (try/finally). Every test run has 600 s.

    python3 -I review/q10/plants_pr152.py HEAD WORKDIR [ID...]
"""
import os
import shutil
import subprocess
import sys

HEAD, WORK = sys.argv[1], sys.argv[2]
ONLY = set(sys.argv[3:])
REPO = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True,
                      check=True).stdout.strip()
TREE = os.path.join(WORK, "t")
K = "tests/kernel/"

SH = [["bash", K + "test_zfs_cli_record.sh"]]
PY_W = [["python3", K + "test_dsync_writer.py"]] + SH
PY_Q = [["python3", K + "test_txg_probe.py"]] + SH
PY_J = [["python3", K + "test_zfs_cli_json.py"]]
CUT = [["bash", K + "test_cut_record.sh"]]
REC = K + "zfs-cli-record.sh"
DW = K + "dsync_writer.py"
TP = K + "txg_probe.py"
ZJ = K + "zfs_cli_json.py"
CR = K + "cut-record.sh"

PLANTS = [
    # R1: cli_snapshot and partial
    ("A1", REC, "partial: keyed on \"taken\", not \"failed\"",
     "grep -q '\"failed\"' \"$OUT/$1/stdout\"; }", "grep -q '\"taken\"' \"$OUT/$1/stdout\"; }", SH),
    ("A2", REC, "partial: only when the count is there and above 0 (#147's rule)",
     "partial() { [ -s \"$OUT/$1/stdout\" ] && grep -q '\"failed\"' \"$OUT/$1/stdout\"; }",
     "partial() { [ -s \"$OUT/$1/stdout\" ] && grep -q '\"taken\": *\"[1-9]' \"$OUT/$1/stdout\"; }", SH),
    ("A3", REC, "cli_snapshot: no destroy of the partial take",
     "        rec \"$name-destroy-partial\" zfs destroy -r \"$p@t\"\n", "", SH),
    ("A4", REC, "cli_snapshot: the destroy without -r",
     "rec \"$name-destroy-partial\" zfs destroy -r \"$p@t\"", "rec \"$name-destroy-partial\" zfs destroy \"$p@t\"", SH),
    ("A5", REC, "cli_snapshot: the destroy at @c",
     "rec \"$name-destroy-partial\" zfs destroy -r \"$p@t\"", "rec \"$name-destroy-partial\" zfs destroy -r \"$p@c\"", SH),
    ("A6", REC, "cli_snapshot: no retake after the destroy",
     "        rec \"$name-destroy-partial\" zfs destroy -r \"$p@t\"\n",
     "        rec \"$name-destroy-partial\" zfs destroy -r \"$p@t\"\n        return 0\n", SH),
    ("A7", REC, "cli_snapshot: the about.txt line dropped",
     "        echo \"$name: $r failed after taking ${taken:-an unknown number of} snapshots: destroyed, and zfs snapshot -r takes @t\" >> \"$OUT/about.txt\"\n",
     "", SH),
    ("A8", REC, "cli_snapshot: the take whatever the list says",
     "    elif lim 120 zfs list -H -o name \"$p@t\" > /dev/null 2>&1; then\n        return 0\n", "", SH),
    ("A9", REC, "cli_snapshot: the list without lim",
     "    elif lim 120 zfs list -H -o name \"$p@t\" > /dev/null 2>&1; then",
     "    elif zfs list -H -o name \"$p@t\" > /dev/null 2>&1; then", SH),
    ("A10", REC, "cli_snapshot: the count from the last match, not the first",
     "| grep -o '[0-9][0-9]*' | head -n 1)", "| grep -o '[0-9][0-9]*' | tail -n 1)", SH),
    ("A11", REC, "members_line: counts the return's keys",
     "print(len(json.load(open(sys.argv[1]))[\"return\"][\"members\"]))",
     "print(len(json.load(open(sys.argv[1]))[\"return\"]))", SH),
    ("A12", REC, "members_line: not after the scale members",
     "                program \"scale-$n-members\" \"$p\" members.lua -n \"$p\" t\n                members_line \"scale-$n-members\"\n",
     "                program \"scale-$n-members\" \"$p\" members.lua -n \"$p\" t\n", SH),
    ("A13", REC, "record_pool: the old rule (list) back, no cli_snapshot",
     "    cli_snapshot \"$x-snapshot-cli\" \"$x-snapshot-1\" \"$p\"\n",
     "    zfs list -H -o name \"$p@t\" > /dev/null 2>&1 || rec \"$x-snapshot-cli\" zfs snapshot -r \"$p@t\"\n", SH),
    ("A14", REC, "scale: the old rule (list) back, no cli_snapshot",
     "            snapshot-cli) cli_snapshot \"scale-$n-snapshot-cli\" \"scale-$n-snapshot\" \"$p\" ;;",
     "            snapshot-cli) lim 120 zfs list -H -o name \"$p@t\" > /dev/null 2>&1 || rec \"scale-$n-snapshot-cli\" zfs snapshot -r \"$p@t\" ;;", SH),
    # zfs snapshot -r's failure records
    ("B1", REC, "cli-exists-child: another --expect pattern",
     "--expect 'dataset already exists' zfs snapshot -r \"$p@p\"", "--expect 'no such pool' zfs snapshot -r \"$p@p\"", SH),
    ("B2", REC, "cli-exists-root at @o0",
     "rec \"$x-cli-exists-root\" --expect 'dataset already exists' zfs snapshot -r \"$p@t\"",
     "rec \"$x-cli-exists-root\" --expect 'dataset already exists' zfs snapshot -r \"$p@o0\"", SH),
    ("B3", REC, "cli-limit: the limit on the root",
     "rec \"$x-cli-limit-set\" zfs set snapshot_limit=0 \"$p/child\"", "rec \"$x-cli-limit-set\" zfs set snapshot_limit=0 \"$p\"", SH),
    ("B4", REC, "cli-limit: the limit never lifted",
     "    lim 120 zfs set snapshot_limit=none \"$p/child\" > /dev/null 2>&1\n", "", SH),
    ("B5", REC, "cli-limit: @q never destroyed",
     "    lim 120 zfs destroy -r \"$p@q\" > /dev/null 2>&1\n", "", SH),
    ("B6", REC, "cli-limit: the take without -r",
     "rec \"$x-cli-limit\" zfs snapshot -r \"$p@q\"", "rec \"$x-cli-limit\" zfs snapshot \"$p@q\"", SH),
    ("B7", REC, "none: the take without -r",
     "rec \"$x-snapshot-cli\" zfs snapshot -r \"$p@t\"\n}", "rec \"$x-snapshot-cli\" zfs snapshot \"$p@t\"\n}", SH),
    ("B8", REC, "scale take-cli without -r",
     "rec \"scale-$n-take-cli\" zfs snapshot -r \"$p@c\"", "rec \"scale-$n-take-cli\" zfs snapshot \"$p@c\"", SH),
    ("B9", REC, "cli-exists-child-after dropped",
     "    rec \"$x-cli-exists-child-after\" zfs list -H -t snapshot -o name -r \"$p\"\n", "", SH),
    # R3, R4, R7 and the lines in writing()
    ("C1", REC, "R4: the writer's stdout is the script's again",
     "\"$log.tsv\" > /dev/null 2> \"$log.err\" &", "\"$log.tsv\" 2> \"$log.err\" &", SH),
    ("C2", REC, "R4: the probe's stdout is the script's again",
     "\"$log.txg.tsv\" > /dev/null 2> \"$log.txg.err\" &", "\"$log.txg.tsv\" 2> \"$log.txg.err\" &", SH),
    ("C3", REC, "stopped: the status never said",
     "    [ \"$BG_RC\" = 0 ] || STOPPED=\"$STOPPED exit=$BG_RC\"\n", "", SH),
    ("C4", REC, "stop_bg: BG_RC from kill -0, not wait",
     "    wait \"$1\"\n    BG_RC=$?\n", "    wait \"$1\"\n    BG_RC=0\n", SH),
    ("C5", REC, "settle: no lim on the zpool sync",
     "        lim 120 zpool sync \"$pool\" > /dev/null 2>&1 || rc=$?", "        zpool sync \"$pool\" > /dev/null 2>&1 || rc=$?", SH),
    ("C6", REC, "settle: rc not kept",
     "        lim 120 zpool sync \"$pool\" > /dev/null 2>&1 || rc=$?", "        lim 120 zpool sync \"$pool\" > /dev/null 2>&1 || :", SH),
    ("C7", REC, "window: t0 twice",
     "    printf '%s\\t%s\\n' \"$t0\" \"$t1\" > \"$OUT/$name/window\"", "    printf '%s\\t%s\\n' \"$t0\" \"$t0\" > \"$OUT/$name/window\"", SH),
    ("C8", REC, "the stall line on the window t0 t0 (W1)",
     "dsync_writer.py stall \"$log.tsv\" \"$t0\" \"$t1\"", "dsync_writer.py stall \"$log.tsv\" \"$t0\" \"$t0\"", SH),
    ("C9", REC, "the probe's line on the window t0 t0 (T2)",
     "txg_probe.py report \"$log.txg.tsv\" \"$t0\" \"$t1\"", "txg_probe.py report \"$log.txg.tsv\" \"$t0\" \"$t0\"", SH),
    ("C10", REC, "stopped: the wait message says 60 s whatever ZCR_STOP_TENTHS",
     "STOPPED=\"$what did not stop within $((${ZCR_STOP_TENTHS:-600} / 10)) s\"", "STOPPED=\"$what did not stop within 60 s\"", SH),
    ("C11", REC, "stop_bg: SIGKILL in place of SIGTERM (W3)",
     "    kill -TERM \"$1\" 2> /dev/null\n    for ((i = 0;", "    kill -KILL \"$1\" 2> /dev/null\n    for ((i = 0;", SH),
    # dsync_writer.py
    ("D1", DW, "stall: NOT COVERED only on a late start",
     "    if started > t0 or last < t1:", "    if started > t0:", PY_W),
    ("D2", DW, "stall: covered when the log reaches t1 exactly or not (<=)",
     "    if started > t0 or last < t1:", "    if started > t0 or last <= t1:", PY_W),
    ("D3", DW, "stall: a second covers only to its start",
     "                last = max(last or 0.0, sec + 1.0)", "                last = max(last or 0.0, float(sec))", PY_W),
    ("D4", DW, "stall: a slow write covers only to its start",
     "                last = max(last or 0.0, s + lat)", "                last = max(last or 0.0, s)", PY_W),
    ("D5", DW, "stall: the end line not counted",
     "                last = max(last or 0.0, float(f[1]))", "                pass", PY_W),
    ("D6", DW, "stall: whole second by floor",
     "    whole = math.ceil(t0) + 1 <= t1", "    whole = math.floor(t0) + 1 <= t1", PY_W),
    ("D7", DW, "stall: started >= t0 (a start at t0 itself not covered)",
     "    if started > t0 or last < t1:", "    if started >= t0 or last < t1:", PY_W),
    # txg_probe.py
    ("E1", TP, "report: covered without a read at or after t1",
     "    if start > t0 or last_tick is None or last_tick < t1:", "    if start > t0 or last_tick is None:", PY_Q),
    ("E2", TP, "report: covered by the stop, not the last read",
     "    if start > t0 or last_tick is None or last_tick < t1:", "    if start > t0 or stop < t1:", PY_Q),
    ("E3", TP, "report: a failed sync is a time",
     "            if rc != 0:\n                failed += 1\n            elif", "            if rc != 0:\n                failed += 1\n            if", PY_Q),
    ("E4", TP, "report: the baseline takes history from before the start",
     "    before = [g[1] for g in groups if start <= g[0] <= t0]", "    before = [g[1] for g in groups if g[0] <= t0]", PY_Q),
    ("E5", TP, "report: the baseline's syncs include failed ones",
     "if not is_open and rc == 0 and s0 + secs <= t0]", "if not is_open and s0 + secs <= t0]", PY_Q),
    ("E6", TP, "report: errors only in the window",
     "            elif f[0] == \"error\":\n                errors += 1",
     "            elif f[0] == \"error\":\n                errors += 1 if t0 <= float(f[1]) <= t1 else 0", PY_Q),
    ("E7", TP, "report: longest_group dirty and written swapped",
     "(longest[2], longest[3] / mib, longest[4] / mib)", "(longest[2], longest[4] / mib, longest[3] / mib)", PY_Q),
    ("E8", TP, "report: an open sync read as failed",
     "syncs.append((float(f[1]), float(f[2]), True, 0))", "syncs.append((float(f[1]), float(f[2]), True, 1))", PY_Q),
    ("E9", TP, "report: only syncs that began in the window (Q7)",
     "        if s0 < t1 and s0 + secs > t0:\n            n += 1", "        if t0 <= s0 < t1:\n            n += 1", PY_Q),
    ("E10", TP, "report: the baseline is the groups that ended by t1",
     "    before = [g[1] for g in groups if start <= g[0] <= t0]", "    before = [g[1] for g in groups if start <= g[0] <= t1]", PY_Q),
    # zfs_cli_json.py
    ("F1", ZJ, "scale records kept",
     "if name.startswith((\"scale-\", \".\")) or", "if name.startswith(\".\") or", PY_J),
    ("F2", ZJ, "rc kept as a string",
     "\"rc\": int(rc) if rc.lstrip(\"-\").isdigit() else rc", "\"rc\": rc", PY_J),
    ("F3", ZJ, "no hex kept",
     "            if raw is not None:\n                rec[stream + \"_hex\"] = raw\n", "", PY_J),
    ("F4", ZJ, "not sorted",
     "indent=1, sort_keys=True, ensure_ascii=False)", "indent=1, ensure_ascii=False)", PY_J),
    ("F5", ZJ, "ASCII escapes",
     "indent=1, sort_keys=True, ensure_ascii=False)", "indent=1, sort_keys=True)", PY_J),
    ("F6", ZJ, "no indent",
     "indent=1, sort_keys=True, ensure_ascii=False)", "sort_keys=True, ensure_ascii=False)", PY_J),
    ("F7", ZJ, "cmd's trailing newline dropped",
     "            rec[\"cmd\"] = text(f.read())[0]", "            rec[\"cmd\"] = text(f.read())[0].rstrip(\"\\n\")", PY_J),
    ("F8", ZJ, "a record whose stderr is missing dropped",
     "            except FileNotFoundError:\n                rec[stream], raw = \"\", None",
     "            except FileNotFoundError:\n                break", PY_J),
    # cut-record.sh end_row
    ("G1", CR, "end_row: no drain", "    drain end-drain.txt\n    : > \"$RDIR/end-rows.txt\"\n",
     "    : > \"$RDIR/end-rows.txt\"\n", CUT),
    ("G2", CR, "end_row: the expiry compared too",
     "        cur=$(awk -F '\\t' '$1 == \"lease\" { print $1 \"\\t\" $2 \"\\t\" $3; next } { print }' \"$RDIR/end-row.txt\")",
     "        cur=$(cat \"$RDIR/end-row.txt\")", CUT),
    ("G3", CR, "end_row: the lease line not compared at all",
     "'$1 == \"lease\" { print $1 \"\\t\" $2 \"\\t\" $3; next } { print }'", "'$1 == \"lease\" { next } { print }'", CUT),
    ("G4", CR, "end_row: only the head line compared",
     "'$1 == \"lease\" { print $1 \"\\t\" $2 \"\\t\" $3; next } { print }'", "'$1 == \"head\" { print }'", CUT),
    ("G5", CR, "end_row: the head's generation not compared",
     "'$1 == \"lease\" { print $1 \"\\t\" $2 \"\\t\" $3; next } { print }'",
     "'$1 == \"lease\" { print $1 \"\\t\" $2 \"\\t\" $3; next } $1 == \"head\" { print $1 \"\\t\" $2; next } { print }'", CUT),
    ("G6", CR, "end_row: three reads, not ten",
     "    for _i in 1 2 3 4 5 6 7 8 9 10; do\n        nbrow end-row.txt", "    for _i in 1 2 3; do\n        nbrow end-row.txt", CUT),
    ("G7", CR, "end_row: no second between reads",
     "        prev=$cur\n        sleep 1\n", "        prev=$cur\n", CUT),
    ("G8", CR, "end_row: the first read kept, not agreed",
     "        [ \"$_i\" -gt 1 ] && [ \"$cur\" = \"$prev\" ] && return 0", "        return 0", CUT),
    ("G9", CR, "end_row: an unsettled row is no error",
     "    rig_error \"node B's row at the end did not read the same twice in a row in ten reads (end-rows.txt)\"\n",
     "    return 0\n", CUT),
    ("G10", CR, "end_row: reads not kept",
     "        { echo \"read $_i\"; cat \"$RDIR/end-row.txt\"; } >> \"$RDIR/end-rows.txt\"\n", "", CUT),
]


def fresh():
    shutil.rmtree(TREE, ignore_errors=True)
    os.makedirs(TREE)
    # The workflows too: test_cut_record.sh reads the publish rig's.
    tar = subprocess.run(["git", "-C", REPO, "archive", HEAD, "tests/kernel", "tests/lib", ".github/workflows"],
                         capture_output=True, check=True).stdout
    subprocess.run(["tar", "-x", "-C", TREE], input=tar, check=True)


def run(cmds):
    for c in cmds:
        try:
            r = subprocess.run(c, cwd=TREE, capture_output=True, text=True, timeout=600)
        except subprocess.TimeoutExpired:
            return "%s: TIMEOUT" % c[-1]
        if r.returncode != 0:
            out = r.stdout + r.stderr
            why = [ln for ln in out.splitlines() if ln.startswith(("FAIL", "ERROR", "not ok"))]
            return "%s: %s" % (os.path.basename(c[-1]), (why[0] if why else "exit %d" % r.returncode)[:110])
    return None


def main():
    try:
        fresh()
        for cmds, label in ((SH, "shell"), (PY_W[:1], "writer"), (PY_Q[:1], "probe"), (PY_J, "json"),
                            (CUT, "cut")):
            print("base %s: %s" % (label, run(cmds) or "passes"), flush=True)
        for pid, f, what, old, new, cmds in PLANTS:
            if ONLY and pid not in ONLY:
                continue
            fresh()
            p = os.path.join(TREE, f)
            with open(p) as fh:
                text = fh.read()
            if text.count(old) != 1:
                print("%s\tNOT APPLIED (%d)\t%s" % (pid, text.count(old), what), flush=True)
                continue
            with open(p, "w") as fh:
                fh.write(text.replace(old, new))
            why = run(cmds)
            print("%s\t%s\t%s%s" % (pid, "KILLED" if why else "SURVIVED", what, "\t(%s)" % why if why else ""),
                  flush=True)
    finally:
        shutil.rmtree(TREE, ignore_errors=True)


main()
