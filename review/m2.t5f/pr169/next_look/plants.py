#!/usr/bin/env python3
"""Plants for the next look at #169 (head af0c183). Each plant is applied
alone to a worktree at the head, the suite named runs under a 600 s
limit, its FAIL lines are kept, and the file is restored with
`git checkout`; `git diff --stat` must then be empty.
usage: python3 -I plants.py WORKTREE [ID...]"""
import subprocess, sys, os, time

K = "tests/kernel/"
ZCR, AGR, CUT = K + "zfs-cli-record.sh", K + "agent-record.sh", K + "cut-record.sh"
TZ, TA, TC = K + "test_zfs_cli_record.sh", K + "test_agent_record.sh", K + "test_cut_record.sh"

PLANTS = [
    # D1: the wait
    ("P01", "D1: the rest of the window not waited (the fix removed)", ZCR,
     '    [ -z "$left" ] || sleep "$left"\n', '    :\n', [TZ]),
    ("P02", "D1: half a second waited past the window", ZCR,
     "l = s - (now - t0); if (l > 0)", "l = s - (now - t0) + 0.5; if (l > 0)", [TZ]),
    ("P03", "D1: the about line's windows swapped between the sides", ZCR,
     'rw+=("$(window_of "$r")") iw+=("$(window_of "$c")")', 'rw+=("$(window_of "$c")") iw+=("$(window_of "$r")")', [TZ]),
    ("P04", "D1: the idle side's window read from the read's record", ZCR,
     'iw+=("$(window_of "$c")")', 'iw+=("$(window_of "$r")")', [TZ]),
    # R3 and failed=
    ("P05", "failed= dropped from the about line", ZCR,
     "reads run ${runs[*]:-none}, failed ${fl[*]:-none}; ", "reads run ${runs[*]:-none}; ", [TZ]),
    ("P06", "failed= read from runs=", ZCR,
     'fl+=("$(field_of "$r" stdout failed)")', 'fl+=("$(field_of "$r" stdout runs)")', [TZ]),
    ("P07", "R3: a step read that failed other than at its limit gets rounds", ZCR,
     '        elif [ "$src" != 0 ]; then\n', '        elif false; then\n', [TZ]),
    # D3
    ("P08", "D3: NOT COVERED lines read again", ZCR,
     "sed -n -e '/^NOT /d' -e ", "sed -n -e ", [TZ]),
    # octal settle
    ("P09", "the settle's limit back to octal in the budget", ZCR,
     "need=$((2 * (w + 2 * (${m:-0} + 1) + ss + 2)))", "need=$((2 * (w + 2 * (${m:-0} + 1) + ${ZCR_SETTLE_S:-30} + 2)))", [TZ]),
    ("P10", "the settle's limit ignored in the budget (always 30)", ZCR,
     "then ss=$((10#$ss)); else ss=30; fi", "then ss=30; else ss=30; fi", [TZ]),
    # S01
    ("P11", "S01: the settle after the margin", ZCR,
     '    settle_txgs "$log.txg.tsv"\n    sleep "$margin"\n', '    sleep "$margin"\n    settle_txgs "$log.txg.tsv"\n', [TZ]),
    # S10, S12, and the other direction
    ("P12", "S10: the line's default for the groups 3", ZCR,
     "opens after ZCR_SETTLE_TXGS=${ZCR_SETTLE_TXGS:-2} groups", "opens after ZCR_SETTLE_TXGS=${ZCR_SETTLE_TXGS:-3} groups", [TZ]),
    ("P13", "S12: the line's default for the margin 5", ZCR,
     "ZCR_WRITER_MARGIN=${ZCR_WRITER_MARGIN:-2} s alone", "ZCR_WRITER_MARGIN=${ZCR_WRITER_MARGIN:-5} s alone", [TZ]),
    ("P14", "the code's default for the groups 3, the line's still 2", ZCR,
     "local log=$1 n=${ZCR_SETTLE_TXGS:-2} limit", "local log=$1 n=${ZCR_SETTLE_TXGS:-3} limit", [TZ]),
    ("P15", "the code's default margin 3, the line's still 2", ZCR,
     "local name=$1 dir=$2 pool=$3 margin=${ZCR_WRITER_MARGIN:-2} ", "local name=$1 dir=$2 pool=$3 margin=${ZCR_WRITER_MARGIN:-3} ", [TZ]),
    ("P16", "the code's default limit 40, the line's still 30", ZCR,
     "n=${ZCR_SETTLE_TXGS:-2} limit=${ZCR_SETTLE_S:-30} t0", "n=${ZCR_SETTLE_TXGS:-2} limit=${ZCR_SETTLE_S:-40} t0", [TZ]),
    # K04
    ("P17", "K04: @c not let go before counts-destroy", ZCR,
     '    release_quietly "$p" c\n    rec "$x-counts-destroy"', '    rec "$x-counts-destroy"', [TZ]),
    ("P18", "K04: release_quietly lists but releases nothing", ZCR,
     '        zfs release "$TAG" "$s" > /dev/null 2>&1\n', '        : "$s"\n', [TZ]),
    # Q3
    ("Q01", "Q3 reserve: the pid not let go", AGR,
     'serve_end 10 || { SERVE_PID=""; rig_error "skyzfsd serve $SERVE_K did not end 10 s after SIGKILL"; }',
     'serve_end 10 || { rig_error "skyzfsd serve $SERVE_K did not end 10 s after SIGKILL"; }', [TA]),
    ("Q02", "Q3 reserve: the pid let go after rig_error", AGR,
     'serve_end 10 || { SERVE_PID=""; rig_error "skyzfsd serve $SERVE_K did not end 10 s after SIGKILL"; }',
     'serve_end 10 || { rig_error "skyzfsd serve $SERVE_K did not end 10 s after SIGKILL"; SERVE_PID=""; }', [TA]),
    ("Q03", "Q3 end_daemon: the pid not let go", CUT,
     'serve_end 10 || { SERVE_PID=""; rig_error "skyzfsd serve did not end 10 s after SIGKILL"; }',
     'serve_end 10 || { rig_error "skyzfsd serve did not end 10 s after SIGKILL"; }', [TA, TC]),
    ("Q04", "Q3 end_daemon: the pid let go after rig_error", CUT,
     'serve_end 10 || { SERVE_PID=""; rig_error "skyzfsd serve did not end 10 s after SIGKILL"; }',
     'serve_end 10 || { rig_error "skyzfsd serve did not end 10 s after SIGKILL"; SERVE_PID=""; }', [TA, TC]),
]

def run(wt, ids):
    for pid, what, f, old, new, suites in PLANTS:
        if ids and pid not in ids:
            continue
        path = os.path.join(wt, f)
        src = open(path).read()
        n = src.count(old)
        if n != 1:
            print("%s %s: NOT APPLIED (%d matches)" % (pid, what, n), flush=True)
            continue
        open(path, "w").write(src.replace(old, new))
        fails, info = [], []
        for s in suites:
            t0 = time.time()
            try:
                p = subprocess.run(["bash", s], cwd=wt, capture_output=True, text=True, timeout=600)
                out, rc = p.stdout + p.stderr, p.returncode
            except subprocess.TimeoutExpired as e:
                out, rc = (e.stdout or b"").decode() if isinstance(e.stdout, bytes) else (e.stdout or ""), "timeout"
            fl = [l for l in out.splitlines() if "FAIL" in l]
            fails += ["  [%s] %s" % (os.path.basename(s), l[:300]) for l in fl]
            info.append("%s rc=%s %.0fs" % (os.path.basename(s), rc, time.time() - t0))
        subprocess.run(["git", "checkout", "--", f], cwd=wt, check=True)
        d = subprocess.run(["git", "diff", "--stat"], cwd=wt, capture_output=True, text=True).stdout.strip()
        verdict = "killed" if fails else "SURVIVES"
        print("%s %s: %s (%s)%s" % (pid, what, verdict, "; ".join(info), "" if not d else " DIFF LEFT: " + d), flush=True)
        for l in fails[:12]:
            print(l, flush=True)

if __name__ == "__main__":
    run(sys.argv[1], set(sys.argv[2:]))
