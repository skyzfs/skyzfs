#!/bin/bash
# Tests of the agent rig's courses (agent-record.sh, sourced by
# cut-record.sh) with no kernel, ZFS, daemon or agent: the validation of
# its inputs, the shards of aplain's points, and the trace helpers.
# Run: bash tests/kernel/test_agent_record.sh
# The stand-ins' bodies are single-quoted: they expand in their own shell.
# The script's functions, eval'd here, call the stand-ins defined beside
# them and read the variables set for them, which shellcheck cannot see.
# A test that sets the script's variables does so in a subshell, so that
# the next test starts from the file's own.
# shellcheck disable=SC2016,SC2030,SC2031,SC2034,SC2329
HERE=$(cd "$(dirname "$0")" && pwd)
SCRIPT=$HERE/agent-record.sh
if ! T=$(mktemp -d) || [ -z "$T" ] || [ ! -d "$T" ]; then echo "no temporary directory"; exit 2; fi
trap 'rm -rf "$T"' EXIT
fails=0
is() { # name got want
    if [ "$2" = "skipped: not root and no sudo" ]; then echo "skip $1: $2"; return; fi
    if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got '$2', want '$3'"; fails=$((fails + 1)); fi
}
# A function as the script has it: from `NAME() {` to the `}` that ends
# it, or the one line where it ends on that line.
fn() {
    awk -v h="$1() {" '!on && index($0, h) == 1 { on = 1; print; if ($0 ~ /}$/) exit; next }
        on { print; if ($0 == "}") exit }' "$SCRIPT"
}
for f in rec_aplain point_reached wait_point case_name published; do
    is "$f is in the script" "$([ -n "$(fn "$f")" ] && echo yes)" yes
done

# The inputs: each refused before anything runs, with 64 and its reason.
# cut-record.sh refuses first to run without root: on a runner the tests
# are not root, and its sudo asks for no password.
# Where neither holds, these six are skipped and say so.
as_root=()
[ "$(id -u)" = 0 ] || as_root=(sudo -n)
refused() { # ENV... -> the status and the last line
    if [ ${#as_root[@]} -gt 0 ] && ! sudo -n true 2>/dev/null; then echo "skipped: not root and no sudo"; return; fi
    "${as_root[@]}" env "$@" CUT_SERVER=daemon CUT_UPLOAD=1 CUT_SKYZFSD=/bin/true CUT_NODEB=/bin/true SKZ_S3_ENDPOINT=x \
        SKZ_S3_BUCKET=b bash "$HERE/cut-record.sh" "$T/out" "$T/none" aplain > "$T/o" 2>&1
    echo "$? $(tail -n 1 "$T/o")"
}
is "an agent course with no agent" "$(refused CUT_AGENT=)" \
    "64 aplain needs CUT_AGENT, the agent's binary built with its test hooks"
is "an agent course with no way to freeze the store" "$(refused CUT_AGENT=/bin/true)" \
    "64 aplain needs CUT_S3_DOWN and CUT_S3_UP"
is "a shard past its count" "$(refused CUT_AGENT=/bin/true CUT_S3_DOWN=x CUT_S3_UP=y CUT_AGENT_SHARD=3/2)" \
    "64 CUT_AGENT_SHARD is K/N with 1 <= K <= N, not 3/2"
is "a shard of none" "$(refused CUT_AGENT=/bin/true CUT_S3_DOWN=x CUT_S3_UP=y CUT_AGENT_SHARD=0/2)" \
    "64 CUT_AGENT_SHARD is K/N with 1 <= K <= N, not 0/2"
is "a point that is not one" "$(refused CUT_AGENT=/bin/true CUT_S3_DOWN=x CUT_S3_UP=y CUT_AGENT_POINTS=ask:during:1)" \
    "64 CUT_AGENT_POINTS: not EFFECT:SIDE:N: ask:during:1"
is "a point's reach of 0" "$(refused CUT_AGENT=/bin/true CUT_S3_DOWN=x CUT_S3_UP=y CUT_AGENT_POINTS=ask:after:0)" \
    "64 CUT_AGENT_POINTS: not EFFECT:SIDE:N: ask:after:0"

# aplain's cases for a shard: the traced publish first, then every Nth
# point from the Kth, in the trace's order.
cases() { # SHARD [POINTS]
    # shellcheck disable=SC2034,SC2329
    (
        RDIR=$T/r; mkdir -p "$RDIR/cases/trace"
        AGENT_SHARD=$1 AGENT_POINTS=${2:-}
        agent_begin() { :; }; agent_end() { :; }
        rig_error() { echo "rig error: $*"; exit 2; }
        acheck() { printf 'ask:before:1\nask:after:1\nasked:after:1\ndone:before:1\ndone:before:2\n'; }
        acase() { printf '%s ' "$1${3:+@$3}"; }
        eval "$(fn rec_aplain)"
        rec_aplain
    )
}
is "shard 1/1: every point" "$(cases 1/1)" \
    "trace A-ask-before-1@ask:before:1 A-ask-after-1@ask:after:1 A-asked-after-1@asked:after:1 A-done-before-1@done:before:1 A-done-before-2@done:before:2 "
is "shard 1/2" "$(cases 1/2)" "trace A-ask-before-1@ask:before:1 A-asked-after-1@asked:after:1 A-done-before-2@done:before:2 "
is "shard 2/2" "$(cases 2/2)" "trace A-ask-after-1@ask:after:1 A-done-before-1@done:before:1 "
is "the points named in place of the trace's" "$(cases 1/1 'hold:after:1 look:before:2')" \
    "trace A-hold-after-1@hold:after:1 A-look-before-2@look:before:2 "
nopoints() {
    # shellcheck disable=SC2034,SC2329
    (
        RDIR=$T/n; rm -rf "$RDIR"; mkdir -p "$RDIR/cases/trace"
        AGENT_SHARD=1/1 AGENT_POINTS=""
        agent_begin() { :; }; agent_end() { :; }
        acheck() { echo "the trace reached no point" >&2; return 2; }
        acase() { printf '%s ' "$1${3:+@$3}"; }
        eval "$(fn rec_aplain)"
        rec_aplain
        cat "$RDIR/points-error.tsv"
    )
}
is "aplain: a trace that gives no points is points-error.tsv, not the course's end" "$(nopoints)" \
    "trace $(printf 'trace\tthe trace reached no point ')"

# The trace helpers, on a stand-in agent.
CDIR=$T/c; mkdir -p "$CDIR"
eval "$(fn point_reached)"; eval "$(fn wait_point)"; eval "$(fn case_name)"
printf '1\tenrol\tbefore\t\n2\tintent\tafter\tt1\n3\tsweep\tbefore\tstray\n' > "$CDIR/trace.tsv"
is "case_name: the last publish named, not a sweep's snapshot" "$(case_name)" t1
printf '4\tintent\tafter\tt2\n' >> "$CDIR/trace.tsv"
is "case_name: the publish's next name" "$(case_name)" t2
: > "$CDIR/trace.tsv"
is "case_name: no name" "$(case_name)" ""
sleep 30 & AGENT_PID=$!
(sleep 0.3; printf '5\tasked\tafter\tt\n' >> "$CDIR/trace.tsv") &
is "wait_point: the point reached while the agent runs" "$(wait_point asked after 20 && echo reached)" reached
kill "$AGENT_PID"; wait "$AGENT_PID" 2>/dev/null
t0=$SECONDS
is "wait_point: the agent ended before the point" "$(wait_point "done" before 20 || echo not)" not
is "wait_point: an agent that ended is not waited for to the limit" "$((SECONDS - t0 < 5))" 1
sleep 30 & AGENT_PID=$!
is "wait_point: the limit" "$(wait_point "done" before 1 || echo "not, $?")" "not, 1"
kill "$AGENT_PID"; wait "$AGENT_PID" 2>/dev/null

# published: a name whose snapshot node A does not have is a line of
# published-gone.tsv, not the rig's error (D1 of #158's review); a list
# that fails is the rig's error.
eval "$(fn published)"
gone() { # [ZFS_FAILS]: published g1 twice, then the gone list; or g2 with a list that fails
    # shellcheck disable=SC2329
    (
        RDIR=$T/p; CDIR=$RDIR/cases/gone; POOL=skzcut; rm -rf "$RDIR"; mkdir -p "$CDIR"; : > "$RDIR/published-gone.tsv"
        zfs() { [ -n "${1+x}" ] && [ -n "${ZFS_FAILS:-}" ] && { echo "no pool" >&2; return 1; }; printf '%s\n' "$POOL@other"; }
        rig_error() { echo "rig error: $*"; exit 2; }
        if [ -n "${1:-}" ]; then ZFS_FAILS=1 published g2; else published g1; published g1; cat "$RDIR/published-gone.tsv"; fi
    )
}
is "published: a name node A has no snapshot of" "$(gone)" "$(printf 'gone\tg1')"
is "published: a list that fails" "$(gone fails)" "rig error: zfs list of skzcut's snapshots failed: no pool"

# What the recorder saw the agent or the daemon do wrong is a verdict for
# the check, and the next case still runs (D5 and G6 of #158's review).
eval "$(fn verdict)"; eval "$(fn case_error)"; eval "$(fn await_agent)"; eval "$(fn recover)"
eval "$(fn reserve)"; eval "$(fn serve_log)"; eval "$(fn frozen_until_withdrawn)"; eval "$(fn scen_gone)"
log() { :; }
NAME=aplain; RDIR=$T/v; CDIR=$RDIR/cases/k1; mkdir -p "$CDIR"
verdict landed "$(printf 'snap/t\thad\nnot landed')"
is "verdict: one line, its tabs and newlines made spaces" "$(cat "$CDIR/verdicts.tsv")" "$(printf 'landed\tsnap/t had not landed')"
case_error "$(printf 'no\nname')"; case_error "again"
is "case_error: a line each" "$(cat "$CDIR/rig-error.txt")" "$(printf 'no name\nagain')"
rm -f "$CDIR"/*; echo "still going" > "$CDIR/publish.err"
sleep 30 & AGENT_PID=$!
await_agent publish 1 2>/dev/null
is "await_agent: an agent that does not end is a verdict" "$(cut -f1 "$CDIR/verdicts.tsv") $(cat "$CDIR/publish.rc") ${AGENT_PID:-none}" \
    "publish_ended timeout none"
rm -f "$CDIR"/*
(exit 3) & AGENT_PID=$!
await_agent publish 5
is "await_agent: an agent that ends, its status" "$(cat "$CDIR/publish.rc") $([ -e "$CDIR/verdicts.tsv" ] && echo verdict)" "3 "

# recover, on stand-ins of the store and the daemon.
thaw() { S3_IS_DOWN=""; echo thaw >> "$T/calls"; }
serve_again() { echo serve_again >> "$T/calls"; sleep 30 & SERVE_PID=$!; }
eval "$(awk '/^serve_end\(\) \{$/ { on = 1 } on { print } on && $0 == "}" { exit }' "$HERE/cut-record.sh")"
SERVE_K=2; echo "serve died" > "$RDIR/serve-2.log"
rm -f "$CDIR"/* "$T/calls"
sleep 30 & SERVE_PID=$!; AGENT_PID=""; S3_IS_DOWN=""
recover
is "recover: nothing left over, nothing done" "$(cat "$CDIR/recovered.txt" "$CDIR/verdicts.tsv" "$T/calls" 2>/dev/null)" ""
kill "$SERVE_PID"; wait "$SERVE_PID" 2>/dev/null
sleep 30 & AGENT_PID=$!; kill -STOP "$AGENT_PID"; S3_IS_DOWN=1
(exit 5) & SERVE_PID=$!; sleep 0.2
recover
is "recover: the agent killed, the store thawed, the daemon served again" \
    "$(tr '\n' ';' < "$CDIR/recovered.txt") $(tr '\n' ';' < "$T/calls") ${AGENT_PID:-none}" \
    "the agent killed;the store thawed;the daemon served again; thaw;serve_again; none"
is "recover: a daemon that had ended is a verdict, with its status" "$(cut -f1 "$CDIR/verdicts.tsv"): $(cut -f2 "$CDIR/verdicts.tsv")" \
    "daemon_serving: skyzfsd serve 2 had ended (status 5) with no kill of the rig's: serve died"
kill "$SERVE_PID"; wait "$SERVE_PID" 2>/dev/null
rm -f "$CDIR"/* "$T/calls"; echo "busy" > "$CDIR/rig-error.txt"
(exit 5) & SERVE_PID=$!; sleep 0.2
recover
is "recover: a daemon ended after the rig's own error in the case is no verdict" \
    "$([ -e "$CDIR/verdicts.tsv" ] && echo verdict) $(tr '\n' ';' < "$CDIR/recovered.txt")" \
    " skyzfsd serve 2 had ended (status 5), after the rig's own error in the case;the daemon served again;"
kill "$SERVE_PID"; wait "$SERVE_PID" 2>/dev/null

# reserve: a daemon that does not end 0 on SIGTERM is a verdict.
rm -f "$CDIR"/* "$T/calls"
bash -c 'trap "exit 1" TERM; sleep 30 & wait' & SERVE_PID=$!; sleep 0.2
reserve
is "reserve: a stop that ends 1 is a verdict, then served again" "$(cut -f1 "$CDIR/verdicts.tsv") $(cat "$T/calls")" \
    "daemon_stopped serve_again"
kill "$SERVE_PID"; wait "$SERVE_PID" 2>/dev/null
rm -f "$CDIR"/* "$T/calls"
bash -c 'trap "exit 0" TERM; sleep 30 & wait' & SERVE_PID=$!; sleep 0.2
reserve
is "reserve: a stop that ends 0" "$([ -e "$CDIR/verdicts.tsv" ] && echo verdict) $(cat "$T/calls")" " serve_again"
kill "$SERVE_PID"; wait "$SERVE_PID" 2>/dev/null

# frozen_until_withdrawn and gone, on a stand-in agent.
acmd() { echo "acmd $*" >> "$T/calls"; }
wait_point() { return 1; }
rm -f "$CDIR"/* "$T/calls"; CASE_AGE=()
sleep 30 & AGENT_PID=$!
frozen_until_withdrawn
is "frozen_until_withdrawn: an agent still running that has not withdrawn" "$(cut -f1 "$CDIR/verdicts.tsv") $(cat "$T/calls" 2>/dev/null)" "withdrew "
kill "$AGENT_PID"; wait "$AGENT_PID" 2>/dev/null
rm -f "$CDIR"/*
frozen_until_withdrawn
is "frozen_until_withdrawn: an agent killed before, a settle withdraws" "$(cat "$T/calls") $([ -e "$CDIR/verdicts.tsv" ] && echo verdict)" \
    "acmd settle-frozen settle --limit-s 30 "
rm -f "$CDIR"/* "$T/calls"
case_name() { :; }
scen_gone x --kill-at holding:before:1
is "gone: no name before the kill is the case's error, not the course's" "$(cat "$CDIR/rig-error.txt")" "no name in the trace before the kill"

# rig_error with a course's hook: given the message once; its 1 (a
# verdict) is the recording's status, anything else 2.
rig() { # HOOK_STATUS: rig_error's exit status and the hook's calls
    rm -f "$T/calls"
    (
        OUT=$T/rig; mkdir -p "$OUT"; : > "$OUT/recordings.tsv"; NAME=adaemon
        eval "$(awk '/^ON_RIG_ERROR=""$/ { on = 1 } on { print } on && $0 == "}" { exit }' "$HERE/cut-record.sh")"
        hook() { echo "hook: $1" >> "$T/calls"; [ "$1" != again ] || rig_error nested; return "$HOOK_RC"; }
        [ "$1" = none ] || ON_RIG_ERROR=hook
        HOOK_RC=$1 rig_error "$(printf 'zfs\tfailed')"
    )
    echo "$? $(tr '\n' ';' 2>/dev/null < "$T/calls") $(cut -f2 "$T/rig/recordings.tsv" | tr '\n' ';')"
    rm -f "$T/calls"
}
is "rig_error: no hook" "$(rig none)" "2  rig error;"
is "rig_error: a hook that found a verdict" "$(rig 1)" "1 hook: zfs failed; rig error;"
is "rig_error: a hook that found none" "$(rig 0)" "2 hook: zfs failed; rig error;"
is "rig_error: a hook of the rig's own error" "$(rig 2)" "2 hook: zfs failed; rig error;"

# agent_judged: the check on the cases done so far.
eval "$(fn agent_judged)"
judged() { # CHECK -> the hook's status, rig-error.tsv, recordings.tsv
    (
        OUT=$T/j; RDIR=$T/j; CDIR=$RDIR/cases/K-x; NAME=arare; VDEV_MB=1; CHECK=$1
        rm -rf "$OUT"; mkdir -p "$OUT"
        agent_judged "zfs holds failed"
        echo "$? $(cat "$RDIR/rig-error.tsv") $(cat "$OUT/recordings.tsv" 2>/dev/null)"
    )
}
echo 'exit 1' > "$T/check1.sh"
is "agent_judged: the check's verdict" "$(judged "bash $T/check1.sh")" "$(printf '1 K-x\tzfs holds failed arare\tchecked\texit 1')"
is "agent_judged: no check" "$(judged none)" "$(printf '2 K-x\tzfs holds failed ')"

# arare: every point the check gives is a case (the check keeps the first
# three reaches and the last, G1); a trace that gives none is
# points-error.tsv, and the course goes on.
rare() { # ACHECK_FAILS_FOR
    # shellcheck disable=SC2034,SC2329
    (
        RDIR=$T/a; rm -rf "$RDIR"; mkdir -p "$RDIR"; AGENT_RARE="destroy sweep"
        agent_begin() { :; }; agent_end() { :; }
        acheck() { case $2 in *"/$FAILS/"*) echo "the trace reached no point" >&2; return 2 ;; *destroy*) printf 'withdraw:before:1\nlook:before:7\n' ;; *) printf 'sweep:before:1\n' ;; esac; }
        acase() { printf '%s ' "$1${3:+@$3}"; }
        FAILS=$1
        eval "$(fn rec_arare)"
        rec_arare
        cat "$RDIR/points-error.tsv" 2>/dev/null
    )
}
is "arare: each point the check gives" "$(rare none)" \
    "trace destroy K-destroy-withdraw-before-1@withdraw:before:1 K-destroy-look-before-7@look:before:7 sweep K-sweep-sweep-before-1@sweep:before:1 gone@holding:before:1 unmarked "
is "arare: a trace that gives no points" "$(rare destroy)" \
    "trace destroy sweep K-sweep-sweep-before-1@sweep:before:1 gone@holding:before:1 unmarked $(printf 'destroy\tthe trace reached no point ')"

# G9 of #158's review: the shell plants that no test killed.
# published: a snapshot of another name that holds T's, or T's name on a
# descendant, is not T's (T02, T06).
near() {
    # shellcheck disable=SC2329
    (
        RDIR=$T/q; CDIR=$RDIR/cases/near; POOL=skzcut; rm -rf "$RDIR"; mkdir -p "$CDIR"; : > "$RDIR/published-gone.tsv"
        zfs() { printf '%s\n' "$POOL@g10" "$POOL/fs@g1"; }
        rig_error() { echo "rig error: $*"; exit 2; }
        published g1; cat "$RDIR/published-gone.tsv"
    )
}
is "published: g10, and g1 of a descendant, are not g1" "$(near)" "$(printf 'near\tg1')"

# agent_begin: each course starts with empty lists (T07).
begin() {
    # shellcheck disable=SC2329
    (
        RDIR=$T/b; rm -rf "$RDIR"; mkdir -p "$RDIR/run"; RUN=$RDIR/run
        python3 -c 'import socket, sys; socket.socket(socket.AF_UNIX).bind(sys.argv[1])' "$RDIR/run/control"
        create_pool() { :; }; load() { :; }; writer_on() { :; }; vrun() { printf '%s' "$RUN"; }
        rig_error() { echo "rig error: $*"; exit 2; }
        for f in cases.tsv published.tsv published-gone.tsv points-error.tsv rig-error.tsv; do echo old > "$RDIR/$f"; done
        eval "$(fn agent_begin)"
        agent_begin
        for f in cases.tsv published.tsv published-gone.tsv points-error.tsv rig-error.tsv; do
            printf '%s:%s ' "$f" "$(cat "$RDIR/$f" 2>/dev/null || echo none)"
        done
        echo "$ON_RIG_ERROR"
    )
}
is "agent_begin: the lists of the course before are gone" "$(begin)" \
    "cases.tsv: published.tsv: published-gone.tsv: points-error.tsv:none rig-error.tsv:none agent_judged"

# adump: the record's dump, its errors and its status kept apart (T10,
# T11).
eval "$(fn adump)"
printf '#!/bin/bash\necho "volume aaaa"\necho "skyzfs-agent: d: the record is corrupt" >&2\nexit 6\n' > "$T/agent6"
chmod +x "$T/agent6"
rm -f "$CDIR"/*
AGENT=$T/agent6 ADIR=x adump record-after
is "adump: a record that does not read" \
    "$(cat "$CDIR/record-after.txt") | $(cat "$CDIR/record-after.err") | $(cat "$CDIR/record-after.rc")" \
    "volume aaaa | skyzfs-agent: d: the record is corrupt | 6"

# asettle: the record dumped before the settle and after the case (T12).
settle() {
    # shellcheck disable=SC2329
    (
        RDIR=$T/s; CDIR=$RDIR/cases/k1; rm -rf "$RDIR"; mkdir -p "$CDIR"; : > "$RDIR/cases.tsv"
        ADIR=$RDIR/agent; HOLD_TAG=skyzfs-publish; SCEN=plain; KILL=""; CASE_AGE=(); POOL=skzcut
        echo 0 > "$CDIR/publish.rc"; echo 0 > "$CDIR/settle.rc"
        adump() { echo "adump $1" >> "$T/calls"; }
        acmd() { echo "acmd $1" >> "$T/calls"; }
        case_name() { echo t1; }; snaprow() { return 0; }; published() { echo "published $1" >> "$T/calls"; }
        zfs() { :; }; log() { :; }
        rig_error() { echo "rig error: $*"; exit 2; }
        eval "$(fn asettle)"
        asettle
        tr '\n' ';' < "$T/calls"; cat "$RDIR/cases.tsv"
    )
    rm -f "$T/calls"
}
is "asettle: dumps, settle, T published, the case's line" "$(settle)" \
    "adump record-before;acmd settle;published t1;adump record-after;$(printf 'k1\tplain\t-\t0\tt1')"

# sweep: the publish that marks the directory, then the scenario's (T20).
sweep() {
    # shellcheck disable=SC2329
    (
        CDIR=$T/w; rm -rf "$CDIR"; mkdir -p "$CDIR"; POOL=skzcut; HOLD_TAG=skyzfs-publish
        acmd() { echo "acmd $*" >> "$T/calls"; }
        z() { shift; echo "$*" >> "$T/calls"; }
        zfs() { :; }
        eval "$(fn scen_sweep)"
        scen_sweep sweep --kill-at sweep:before:1
        tr '\n' ';' < "$T/calls"
    )
    rm -f "$T/calls"
}
is "sweep: the first publish, the stray hold, the scenario's publish" "$(sweep)" \
    "acmd first publish --limit-s 600;zfs snapshot skzcut@sweep-w;zfs hold skyzfs-publish skzcut@sweep-w;acmd publish publish --limit-s 600 --kill-at sweep:before:1;"

# both: the daemon's status as the rig's SIGKILL left it (T30, T31).
eval "$(fn scen_both)"
abg() { :; }; await_agent() { :; }; serve_again() { :; }
rm -f "$CDIR"/*
sleep 30 & SERVE_PID=$!; scen_both x
is "both: the daemon serving, killed by the rig" "$(cat "$CDIR/both-kill.tsv")" "$(printf 'killed_rc\t137')"
(exit 0) & SERVE_PID=$!; sleep 0.3; scen_both x
is "both: the daemon had ended 0" "$(cat "$CDIR/both-kill.tsv")" "$(printf 'killed_rc\t0')"
(exit 7) & SERVE_PID=$!; sleep 0.3; scen_both x
is "both: the daemon had ended 7" "$(cat "$CDIR/both-kill.tsv")" "$(printf 'killed_rc\t7')"

[ "$fails" = 0 ] && echo "all passed"
exit $((fails > 0))
