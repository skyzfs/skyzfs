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
nopoints_empty() {
    # shellcheck disable=SC2034,SC2329
    (
        RDIR=$T/n; rm -rf "$RDIR"; mkdir -p "$RDIR/cases/trace"
        AGENT_SHARD=1/1 AGENT_POINTS=""
        agent_begin() { :; }; agent_end() { :; }
        acheck() { :; }
        acase() { printf '%s ' "$1${3:+@$3}"; }
        eval "$(fn rec_aplain)"
        rec_aplain
        cat "$RDIR/points-error.tsv"
    )
}
is "aplain: a trace that gives no kill point is said" "$(nopoints_empty)" \
    "trace $(printf 'trace\tthe trace gives no kill point: no A case ran')"

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
SCEN=dhook recover
is "recover: in dhook, a daemon ended after the rig's own error in the case is no verdict" \
    "$([ -e "$CDIR/verdicts.tsv" ] && echo verdict) $(tr '\n' ';' < "$CDIR/recovered.txt")" \
    " skyzfsd serve 2 had ended (status 5), after the rig's own error in the case;the daemon served again;"
kill "$SERVE_PID"; wait "$SERVE_PID" 2>/dev/null
# With no error of the rig's in the dhook case, a daemon that ended is a
# verdict: the exemption must not hide it (X06b in G1 of #163's review).
for err in none empty; do
    rm -f "$CDIR"/* "$T/calls"; [ "$err" = none ] || : > "$CDIR/rig-error.txt"
    (exit 5) & SERVE_PID=$!; sleep 0.2
    SCEN=dhook recover
    is "recover: in dhook with no error of the rig's ($err), a daemon that ended is a verdict" "$(cut -f1 "$CDIR/verdicts.tsv" 2>/dev/null)" daemon_serving
    kill "$SERVE_PID" 2>/dev/null; wait "$SERVE_PID" 2>/dev/null
done
# Once the hook's end is recorded (kill.tsv), the instance served after
# it has no hook: one that ended is a verdict, whatever error the case
# had (G-a of #164's review).
rm -f "$CDIR"/* "$T/calls"; echo "busy" > "$CDIR/rig-error.txt"; printf 'killed_rc\t3\nserve\t1\n' > "$CDIR/kill.tsv"
(exit 5) & SERVE_PID=$!; sleep 0.2
SCEN=dhook recover
is "recover: in dhook after the hook's end, a daemon that ended is a verdict" "$(cut -f1 "$CDIR/verdicts.tsv" 2>/dev/null)" daemon_serving
kill "$SERVE_PID" 2>/dev/null; wait "$SERVE_PID" 2>/dev/null
# D9 of #159's review: where the rig never stops the daemon, one that
# ended is a verdict, whatever error the case had.
for sc in destroy gone plain; do
    rm -f "$CDIR"/* "$T/calls"; echo "busy" > "$CDIR/rig-error.txt"
    (exit 5) & SERVE_PID=$!; sleep 0.2
    SCEN=$sc recover
    is "recover: in $sc, a daemon ended after the case's error is a verdict" "$(cut -f1 "$CDIR/verdicts.tsv")" daemon_serving
    kill "$SERVE_PID"; wait "$SERVE_PID" 2>/dev/null
done

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
# A daemon that had ended before the SIGTERM, even with 0, ended unasked:
# a verdict, then served again (G-b of #164's review).
for st in 0 5; do
    rm -f "$CDIR"/* "$T/calls"
    (exit "$st") & SERVE_PID=$!
    while kill -0 "$SERVE_PID" 2>/dev/null; do sleep 0.05; done
    reserve
    is "reserve: a daemon that had ended ($st) before the stop is a verdict, then served again" \
        "$(cut -f1 "$CDIR/verdicts.tsv" 2>/dev/null) $(cut -f2 "$CDIR/verdicts.tsv" 2>/dev/null | cut -d: -f1) $(cat "$T/calls")" \
        "daemon_stopped skyzfsd serve 2 had ended before SIGTERM (status $st) serve_again"
done

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
        eval "$(sed -n '/^rig_error() {$/,/^}$/p' "$HERE/cut-record.sh")"
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
        agent_quiet() { :; }
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
        acheck() { case $2 in *"/$FAILS/"*) echo "the trace reached no point" >&2; return 2 ;; *"/$EMPTY/"*) ;; *destroy*) printf 'withdraw:before:1\nlook:before:7\n' ;; *) printf 'sweep:before:1\n' ;; esac; }
        acase() { printf '%s ' "$1${3:+@$3}"; }
        FAILS=$1 EMPTY=${2:-none}
        eval "$(fn rec_arare)"
        rec_arare
        cat "$RDIR/points-error.tsv" 2>/dev/null
    )
}
is "arare: each point the check gives" "$(rare none)" \
    "trace destroy K-destroy-withdraw-before-1@withdraw:before:1 K-destroy-look-before-7@look:before:7 sweep K-sweep-sweep-before-1@sweep:before:1 gone@holding:before:1 unmarked "
is "arare: a trace that gives no points" "$(rare destroy)" \
    "trace destroy sweep K-sweep-sweep-before-1@sweep:before:1 gone@holding:before:1 unmarked $(printf 'destroy\tthe trace reached no point ')"
is "arare: a scenario that adds no point is said" "$(rare none destroy)" \
    "trace destroy sweep K-sweep-sweep-before-1@sweep:before:1 gone@holding:before:1 unmarked $(printf 'destroy\tits trace adds no kill point to the plain trace'"'"'s: no K-destroy case ran')"

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
        echo "$ON_RIG_ERROR $ON_DAEMON_WRONG"
    )
}
is "agent_begin: the lists of the course before are gone; the course's hooks set" "$(begin)" \
    "cases.tsv: published.tsv: published-gone.tsv: points-error.tsv:none rig-error.tsv:none agent_judged agent_daemon_wrong"

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
settle() { # [two|fail]: what zfs list of the snapshots after gives
    # shellcheck disable=SC2329
    (
        local list=${1:-}
        RDIR=$T/s; CDIR=$RDIR/cases/k1; rm -rf "$RDIR"; mkdir -p "$CDIR"; : > "$RDIR/cases.tsv"
        ADIR=$RDIR/agent; HOLD_TAG=skyzfs-publish; SCEN=plain; KILL=""; CASE_AGE=(); POOL=skzcut
        echo 0 > "$CDIR/publish.rc"; echo 0 > "$CDIR/settle.rc"
        adump() { echo "adump $1" >> "$T/calls"; }
        acmd() { echo "acmd $1" >> "$T/calls"; }
        case_name() { echo t1; }; snaprow() { return 0; }; published() { echo "published $1" >> "$T/calls"; }
        zfs() {
            [ "$*" = "list -H -o name -t snapshot -r skzcut" ] || return 0
            case $list in
                two) printf 'skzcut@a\nskzcut@b\n' ;;
                fail) echo "pool busy" >&2; return 1 ;;
            esac
        }
        log() { :; }
        rig_error() { echo "rig error: $*"; exit 2; }
        eval "$(fn asettle)"
        asettle
        tr '\n' ';' < "$T/calls"; cat "$RDIR/cases.tsv"
        [ -z "$list" ] || echo " after: $(tr '\n' ';' < "$CDIR/snapshots-after.txt")"
    )
    rm -f "$T/calls"
}
is "asettle: dumps, settle, T published, the case's line" "$(settle)" \
    "adump record-before;acmd settle;published t1;adump record-after;$(printf 'k1\tplain\t-\t0\tt1')"
# The snapshots after the case are listed for the judge, and a list that
# fails is the rig's error (X12 in G1 of #163's review).
is "asettle: the snapshots after the case" "$(settle two)" \
    "adump record-before;acmd settle;published t1;adump record-after;$(printf 'k1\tplain\t-\t0\tt1')
 after: skzcut@a;skzcut@b;"
is "asettle: a list of the snapshots that fails" "$(settle fail)" \
    "rig error: zfs list of skzcut's snapshots after the case failed: pool busy"

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
    "acmd first publish --limit-s 600;zfs snapshot skzcut@skz-sweep-w;zfs hold skyzfs-publish skzcut@skz-sweep-w;acmd publish publish --limit-s 600 --kill-at sweep:before:1;"

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

# #159's review: D7, D9, D10 and the shell plants that no test killed.
# Functions of cut-record.sh as it has them.
fnc() {
    awk -v h="$1() {" '!on && index($0, h) == 1 { on = 1; print; if ($0 ~ /}$/) exit; next }
        on { print; if ($0 == "}") exit }' "$HERE/cut-record.sh"
}
# z, the pool commands' limit, under lim and not timeout(1), which waits
# for a child that cannot be killed (G2 of #163's review).
is "z: under lim" "$(fnc z | grep -c '^ *lim "\$secs" "\$@" || rig_error')/$(fnc z | grep -c '^ *timeout ')" 1/0
for f in daemon_wrong cleanup serve_start end_daemon end; do
    is "$f is in cut-record.sh" "$([ -n "$(fnc "$f")" ] && echo yes)" yes
done
for f in akill_serve agent_daemon_wrong agent_quiet acase scen_restart scen_destroy scen_dhook scen_late vrun; do
    is "$f is in the script" "$([ -n "$(fn "$f")" ] && echo yes)" yes
done

# akill_serve: a daemon that had ended by itself before the rig's kill is
# a verdict with its status; one the kill ended is not (D10).
eval "$(fn akill_serve)"
akill() { # STATUS|serving
    rm -f "${CDIR:?}"/*
    if [ "$1" = serving ]; then sleep 30 & SERVE_PID=$!; else (exit "$1") & SERVE_PID=$!; sleep 0.3; fi
    akill_serve
    echo "${SERVE_PID:-none} $(cut -f1 "$CDIR/verdicts.tsv" 2>/dev/null): $(cut -f2 "$CDIR/verdicts.tsv" 2>/dev/null)"
}
SERVE_K=2
is "akill_serve: a serving daemon, killed" "$(akill serving)" "none : "
is "akill_serve: a daemon that had ended 0" "$(akill 0)" \
    "none daemon_serving: skyzfsd serve 2 had ended (status 0) before the rig's kill: serve died"
is "akill_serve: a daemon that had ended 9" "$(akill 9)" \
    "none daemon_serving: skyzfsd serve 2 had ended (status 9) before the rig's kill: serve died"

# serve_log: the first instance's log and the Kth's (U30).
is "serve_log: the first instance" "$(RDIR=/r serve_log 1)" /r/serve.log
is "serve_log: the third" "$(RDIR=/r serve_log 3)" /r/serve-3.log

# vrun: the volume's socket directory as the daemon names it (S21a).
touch "$T/vol"
is "vrun: RUN/v-<dev>-<ino> in hex" "$(eval "$(fn vrun)"; RUN=/run/x VOL=$T/vol vrun)" \
    "$(printf '/run/x/v-%x-%x' "$(stat -c %d "$T/vol")" "$(stat -c %i "$T/vol")")"

# case_name and point_reached: a line with no name after a named one,
# and a point of another side (S03, S05).
eval "$(fn case_name)"
printf '1\tintent\tafter\tt1\n2\tdone\tbefore\t\n' > "$CDIR/trace.tsv"
is "case_name: a line with an empty name does not clear T" "$(case_name)" t1
printf '1\tasked\tbefore\tt1\n' > "$CDIR/trace.tsv"
is "point_reached: the effect on the other side is not the point" "$(point_reached asked after && echo reached || echo not)" not
is "point_reached: the point" "$(point_reached asked before && echo reached)" reached

# published: a second name gone is a second line (T09).
gone2() {
    # shellcheck disable=SC2329
    (
        RDIR=$T/p2; CDIR=$RDIR/cases/gone; POOL=skzcut; rm -rf "${RDIR:?}"; mkdir -p "$CDIR"; : > "$RDIR/published-gone.tsv"
        zfs() { printf '%s\n' "$POOL@other"; }
        rig_error() { echo "rig error: $*"; exit 2; }
        published g1; published g2; tr '\n' ';' < "$RDIR/published-gone.tsv"
    )
}
is "published: two names gone, two lines" "$(gone2)" "$(printf 'gone\tg1;gone\tg2;')"

# asettle: snap/T absent at node B is a retry; the settle has the case's
# bound; the case's kill is in cases.tsv; the holds are read before they
# are let go (S15, S17, S22, S23).
settle2() { # SNAPROW_RC KILL
    # shellcheck disable=SC2329
    (
        RDIR=$T/s2; CDIR=$RDIR/cases/k2; rm -rf "${RDIR:?}"; mkdir -p "$CDIR"; : > "$RDIR/cases.tsv"
        ADIR=$RDIR/agent; HOLD_TAG=skyzfs-publish; SCEN=tooold; KILL=$2; CASE_AGE=(--age-max-s 8); POOL=skzcut
        echo 3 > "$CDIR/publish.rc"; echo 0 > "$CDIR/settle.rc"
        adump() { :; }
        acmd() { echo "acmd $*" >> "$T/calls"; [ "$1" != retry ] || echo "snap/n1 abc" > "$CDIR/retry.out"; }
        case_name() { echo t1; }; snaprow() { return "$SNAPROW_RC"; }; published() { echo "published $1" >> "$T/calls"; }
        # zfs as a command too: xargs runs it.
        mkdir -p "$T/bin"
        printf '#!/bin/bash\ncase "$1 ${2:-}" in\n"list -H") echo skzcut@t1 ;;\n"holds -H") printf "%%s\\t%%s\\t%%s\\n" "$3" skyzfs-publish Thu ;;\n"release skyzfs-publish") echo "release $3" >> "%s/calls" ;;\nesac\n' "$T" > "$T/bin/zfs"
        chmod +x "$T/bin/zfs"; PATH=$T/bin:$PATH
        unset -f zfs
        log() { :; }
        rig_error() { echo "rig error: $*"; exit 2; }
        eval "$(fn asettle)"
        SNAPROW_RC=$1 asettle
        tr '\n' ';' < "$T/calls"; echo; cut -f1 "$CDIR/holds.txt"; cat "$CDIR/holds-released.txt"; cat "$RDIR/cases.tsv"
    )
    rm -f "${T:?}/calls"
}
is "asettle: snap/T absent, a retry; its name published; the hold read, then let go; the kill in the line" "$(settle2 1 look:before:2)" \
    "acmd settle settle --limit-s 600 --age-max-s 8;acmd retry publish --limit-s 600;published n1;published n1;release skzcut@t1;
skzcut@t1
skzcut@t1
$(printf 'k2\ttooold\tlook:before:2\t3\tt1')"

# acase: recover after every scenario, then the settle, and no case under
# way after it (U13; the nit on agent_judged's case).
acase_t() {
    # shellcheck disable=SC2329
    (
        RDIR=$T/ac; rm -rf "${RDIR:?}"
        scen_x() { echo "scen_x $*" >> "$T/calls"; }
        recover() { echo recover >> "$T/calls"; }
        asettle() { echo "asettle $(basename "$CDIR")" >> "$T/calls"; }
        eval "$(fn acase)"
        acase c1 x kp:before:1 arg
        tr '\n' ';' < "$T/calls"; echo "CDIR=${CDIR:-none}"
    )
    rm -f "${T:?}/calls"
}
is "acase: the scenario, recover, the settle; then no case" "$(acase_t)" \
    "scen_x arg --trace-points $T/ac/cases/c1/trace.tsv --kill-at kp:before:1;recover;asettle c1;CDIR=none"

# destroy and gone: the release and the destroy bounded (lim), the
# release before the destroy; a failure is the case's error, with its log
# kept apart, and the case goes no further (D9, S18, U14, U15, U16); no
# asked:after with no kill is a verdict, with a kill none (the nit).
destroy_t() { # FAIL_AT KILL REACHED
    # shellcheck disable=SC2329
    (
        CDIR=$T/d; rm -rf "${CDIR:?}"; mkdir -p "$CDIR"; POOL=skzcut; HOLD_TAG=skyzfs-publish; KILL=$2
        freeze() { echo freeze >> "$T/calls"; }; thaw() { echo thaw >> "$T/calls"; }
        abg() { echo abg >> "$T/calls"; }; await_agent() { echo "await $*" >> "$T/calls"; }
        wait_point() { [ "$REACHED" = 1 ]; }; case_name() { echo t1; }
        mark() { echo "mark $1" >> "$T/calls"; }; frozen_until_withdrawn() { echo frozen >> "$T/calls"; }
        lim() { echo "lim $1 $2 $3" >> "$T/calls"; shift; [ "$2" != "$FAIL_AT" ] || { echo "busy" >&2; return 1; }; }
        log() { :; }
        eval "$(fn scen_destroy)"
        FAIL_AT=$1 REACHED=$3 scen_destroy x
        tr '\n' ';' < "$T/calls"; echo; cat "$CDIR/rig-error.txt" "$CDIR/verdicts.tsv" 2>/dev/null; find "$CDIR" -mindepth 1 -printf '%P\n' | sort | tr '\n' ' '
    )
    rm -f "${T:?}/calls"
}
is "destroy: release, destroy, both bounded" "$(destroy_t none '' 1)" \
    "freeze;abg;lim 60 zfs release;lim 60 zfs destroy;mark destroyed-d;frozen;thaw;await publish 700;
destroy.log "
is "destroy: a release that fails" "$(destroy_t release '' 1)" \
    "freeze;abg;lim 60 zfs release;
releasing and destroying skzcut@t1 failed: busy
destroy-failed.log rig-error.txt "
is "destroy: no asked with no kill is a verdict" "$(destroy_t none '' 0 | sed -n 2p | cut -f1)" asked
is "destroy: no asked with a kill is none" "$(destroy_t none withdraw:before:1 0)" "freeze;abg;thaw;await publish 700;"
gone_t() {
    # shellcheck disable=SC2329
    (
        CDIR=$T/g; rm -rf "${CDIR:?}"; mkdir -p "$CDIR"; POOL=skzcut
        acmd() { :; }; case_name() { echo t1; }; log() { :; }
        lim() { echo "lim $*" >> "$T/calls"; echo "dataset is busy" >&2; return 1; }
        eval "$(fn scen_gone)"
        scen_gone x
        tr '\n' ';' < "$T/calls"; cat "$CDIR/rig-error.txt"; find "$CDIR" -mindepth 1 -printf '%P\n' | sort | tr '\n' ' '
    )
    rm -f "${T:?}/calls"
}
is "gone: the destroy bounded; its failure the case's error, its log kept apart" "$(gone_t)" \
    "lim 60 zfs destroy skzcut@t1;destroying skzcut@t1 failed: dataset is busy
destroy-failed.log rig-error.txt "

# restart: the daemon killed with its status judged (akill_serve); no
# asked with no kill a verdict.
restart_t() { # REACHED KILL
    # shellcheck disable=SC2329
    (
        CDIR=$T/rs; rm -rf "${CDIR:?}"; mkdir -p "$CDIR"; KILL=$2
        freeze() { echo freeze >> "$T/calls"; }; thaw() { echo thaw >> "$T/calls"; }
        abg() { echo abg >> "$T/calls"; }; await_agent() { echo "await $*" >> "$T/calls"; }
        wait_point() { [ "$REACHED" = 1 ]; }; case_name() { echo t1; }; snaprow() { echo "snaprow $*" >> "$T/calls"; }
        akill_serve() { echo akill >> "$T/calls"; }; serve_again() { echo serve_again >> "$T/calls"; }; log() { :; }
        eval "$(fn scen_restart)"
        REACHED=$1 scen_restart x
        tr '\n' ';' < "$T/calls"; cut -f1 "$CDIR/verdicts.tsv" 2>/dev/null
    )
    rm -f "${T:?}/calls"
}
is "restart: killed at asked, the store thawed, served again" "$(restart_t 1 '')" \
    "freeze;abg;akill;thaw;snaprow t1 between.txt;serve_again;await publish 700;"
is "restart: no asked with no kill" "$(restart_t 0 '')" "freeze;abg;thaw;await publish 700;asked"
is "restart: no asked with a kill" "$(restart_t 0 count:before:1)" "freeze;abg;thaw;await publish 700;"

# dhook: a carry hook freezes the store before the agent starts; no asked
# is a verdict; a daemon whose end serve_end does not see is the case's
# error (S16, U18, U21).
dhook_t() { # HOOK SERVE_END_RC
    # shellcheck disable=SC2329
    (
        CDIR=$T/dh; RDIR=$T/dhr; rm -rf "${CDIR:?}" "${RDIR:?}"; mkdir -p "$CDIR" "$RDIR"; SERVE_K=3
        # The stand-in's daemon ends at once where the hook is armed.
        # shellcheck disable=SC2120
        reserve() { SERVE_K=4; echo "reserve $*" >> "$T/calls"; /bin/sleep 30 & SERVE_PID=$!; [ -n "${1:-}" ] || return 0; kill "$SERVE_PID"; /bin/sleep 0.2; }
        freeze() { echo freeze >> "$T/calls"; }; thaw() { echo thaw >> "$T/calls"; }
        sleep() { :; }
        abg() { echo abg >> "$T/calls"; /bin/sleep 30 & AGENT_PID=$!; }; await_agent() { echo "await $*" >> "$T/calls"; }
        wait_point() { return 1; }; serve_again() { echo serve_again >> "$T/calls"; }
        serve_end() { return "$SERVE_END_RC"; }
        log() { :; }
        echo "killed" > "$RDIR/serve-4.log"
        eval "$(fn scen_dhook)"
        SERVE_END_RC=$2 scen_dhook "$1"
        tr '\n' ';' < "$T/calls"; cut -f1 "$CDIR/verdicts.tsv" 2>/dev/null; cat "$CDIR/rig-error.txt" 2>/dev/null
        kill "$AGENT_PID" 2>/dev/null
    )
    rm -f "${T:?}/calls"
}
is "dhook: carry, the store frozen first; no asked a verdict" "$(dhook_t carry-put:1 0)" \
    "reserve --kill-at-upload carry-put:1;freeze;abg;thaw;serve_again;await publish 700;asked"
is "dhook: a daemon whose end is not seen is the case's error" "$(dhook_t put:1 1)" \
    "reserve --kill-at-upload put:1;abg;the daemon's pid went away and serve_end did not see it end"

# late: no asked is a verdict and the case goes no further; a row that
# does not land is one too (U19, U20).
late_t() { # REACHED [LANDED]
    # shellcheck disable=SC2329
    (
        CDIR=$T/lt; rm -rf "${CDIR:?}"; mkdir -p "$CDIR"
        freeze() { echo freeze >> "$T/calls"; }; thaw() { echo thaw >> "$T/calls"; }
        sleep() { :; }; kill() { echo "kill $1" >> "$T/calls"; }
        abg() { echo abg >> "$T/calls"; AGENT_PID=99999; }; await_agent() { echo "await $*" >> "$T/calls"; }
        wait_point() { [ "$REACHED" = 1 ]; }; case_name() { echo t1; }; snaprow() { [ "$LANDED" = 1 ]; }
        mark() { :; }; adump() { echo "adump $1" >> "$T/calls"; }; akill_serve() { echo akill >> "$T/calls"; }
        kill_serve() { echo kill_serve >> "$T/calls"; }; serve_again() { echo serve_again >> "$T/calls"; }
        log() { :; }
        eval "$(fn scen_late)"
        REACHED=$1 LANDED=${2:-0} scen_late x
        tr '\n' ';' < "$T/calls"; cut -f1 "$CDIR/verdicts.tsv" 2>/dev/null
    )
    rm -f "${T:?}/calls"
}
is "late: no asked, a verdict, nothing more" "$(late_t 0)" "freeze;abg;asked"
is "late: a row that does not land, a verdict, nothing more" "$(late_t 1)" "freeze;abg;kill -STOP;adump record-stopped;thaw;landed"
# Landed: the daemon is killed with its status judged, served again, and
# the agent let go (X04, X04b in G1 of #163's review).
is "late: landed, akill_serve, served again, the agent continued" "$(late_t 1 1)" \
    "freeze;abg;kill -STOP;adump record-stopped;thaw;akill;serve_again;kill -CONT;await publish 700;"

# reserve: a stop that hangs is a verdict, then killed (U11, U12), on a
# serve_end with its bound cut to a second.
eval "$(fnc serve_end | sed 's/^serve_end()/serve_end_real()/')"
hung() {
    # shellcheck disable=SC2329
    (
        serve_end() { serve_end_real 1; }
        serve_again() { echo serve_again >> "$T/calls"; }
        rig_error() { echo "rig error: $*" >> "$T/calls"; exit 2; }
        rm -f "${CDIR:?}"/* "${T:?}/calls"
        bash -c 'trap "" TERM; sleep 30' & SERVE_PID=$!; sleep 0.2
        reserve
        tr '\n' ';' < "$T/calls"; cut -f1 "$CDIR/verdicts.tsv"
        [ -z "$SERVE_PID" ] || kill "$SERVE_PID" 2>/dev/null
    )
    rm -f "${T:?}/calls"
}
is "reserve: a stop that hangs, a verdict, killed, served again" "$(hung)" "serve_again;daemon_stopped"

# rig_error: a hook's status other than 1 is 2; a hook's own rig_error
# does not call it again (V01, V02, V05).
is "rig_error: a hook that ends 3" "$(rig 3)" "2 hook: zfs failed; rig error;"
rig_again() {
    rm -f "${T:?}/calls"
    (
        OUT=$T/rig; mkdir -p "$OUT"; : > "$OUT/recordings.tsv"; NAME=adaemon
        eval "$(fnc rig_error)"
        hook() { echo "hook: $1" >> "$T/calls"; rig_error again; }
        ON_RIG_ERROR=hook
        rig_error first
    )
    echo "$? $(tr '\n' ';' < "$T/calls")"
    rm -f "${T:?}/calls"
}
is "rig_error: the hook's own rig_error ends it, once" "$(rig_again)" "2 hook: first;"

# daemon_wrong: the rig's error with no hook; with the agent's, a
# verdict in the case under way or the course's (D10).
eval "$(fn agent_daemon_wrong)"
dw() { # CASE|none [HOOK|-]
    (
        RDIR=$T/dw; rm -rf "${RDIR:?}"; mkdir -p "$RDIR"; CDIR=""; OUT=$RDIR; NAME=adaemon
        [ "$1" = none ] || { CDIR=$RDIR/cases/$1; mkdir -p "$CDIR"; }
        rig_error() { echo "rig error: $*"; exit 2; }
        eval "$(fnc daemon_wrong)"
        ON_DAEMON_WRONG=${2:-agent_daemon_wrong}
        [ "$ON_DAEMON_WRONG" != - ] || ON_DAEMON_WRONG=""
        daemon_wrong daemon_stopped "ended with 1"
        echo "went on ${CDIR:-none}"
        find "$RDIR/cases" -name verdicts.tsv -printf '%P ' 2>/dev/null; cat "$RDIR"/cases/*/verdicts.tsv 2>/dev/null
    )
}
is "daemon_wrong: no hook, the rig's error" "$(dw none -)" "rig error: ended with 1"
is "daemon_wrong: in a case, its verdict" "$(dw d1)" "went on $T/dw/cases/d1
d1/verdicts.tsv $(printf 'daemon_stopped\tended with 1')"
is "daemon_wrong: after the cases, the course's" "$(dw none)" "went on none
course/verdicts.tsv $(printf 'daemon_stopped\tended with 1')"

# serve_start: an instance that ends before it is ready, with its status:
# the hook's verdict, then the rig's error (D10).
printf '#!/bin/bash\necho "no lease" >&2\nexit 10\n' > "$T/skyzfsd-ends"; chmod +x "$T/skyzfsd-ends"
ss() { # HOOK
    (
        RDIR=$T/ss; rm -rf "${RDIR:?}"; mkdir -p "$RDIR"; VOL=$RDIR/vol; RUN=$RDIR/run; SKYZFSD=$T/skyzfsd-ends
        HOLD=0; UPLOAD=0; SERVE_ARGS=(); SERVE_PID=""
        log() { :; }
        rig_error() { echo "rig error: $*"; exit 2; }
        hook() { echo "verdict $1: $2"; }
        ON_DAEMON_WRONG=$1
        eval "$(fnc serve_end)"; eval "$(fnc serve_start)"
        serve_start 2
    )
}
is "serve_start: ended before ready, with no hook" "$(ss '')" "rig error: skyzfsd serve 2 ended (status 10) before it was ready: no lease"
is "serve_start: ended before ready, the hook's verdict first" "$(ss hook)" \
    "verdict daemon_ready: skyzfsd serve 2 ended (status 10) before it was ready: no lease
rig error: skyzfsd serve 2 ended (status 10) before it was ready: no lease"

# serve_start makes the ready file and the instance's log before the
# fork: the child's redirections come after it, and the loop reads both
# at once (run 45836590019: sed found no serve-7.log, and pipefail with
# errexit ended the course).
before_fork() { # FILE-WORD
    awk -v w="$1" '/^serve_start\(\) \{/ { on = 1 } on && $0 == "    : > " w { made = NR }
        on && /"\$SKYZFSD" serve --dir/ { print (made && made < NR) ? "made" : "not made"; exit }' "$HERE/cut-record.sh"
}
# shellcheck disable=SC2016 # the words as written in the script
is "serve_start: the ready file before the fork" "$(before_fork '"$VOL.ready"')" made
# shellcheck disable=SC2016
is "serve_start: the log before the fork" "$(before_fork '"$slog"')" made
# And with them made, a stale ready file is not read.
is "serve_start: a stale ready file emptied" "$(
    RDIR=$T/ss; rm -rf "${RDIR:?}"; mkdir -p "$RDIR"; VOL=$RDIR/vol; RUN=$RDIR/run; SKYZFSD=$T/skyzfsd-ends
    echo 'ready /dev/stale' > "$VOL.ready"
    HOLD=0; UPLOAD=0; SERVE_ARGS=(); SERVE_PID=""; ON_DAEMON_WRONG=""
    log() { :; }; rig_error() { exit 2; }
    eval "$(fnc serve_end)"; eval "$(fnc serve_start)"
    (serve_start 2) > /dev/null; grep -c stale "$VOL.ready")" 0

# end_daemon: a daemon that does not end 0 on SIGTERM; its log is the
# current instance's (D10).
printf '#!/bin/bash\n[ "$1" != status ] || echo "cut_state idle"\n' > "$T/skyzfsd-ok"; chmod +x "$T/skyzfsd-ok"
ed_t() { # HOOK [hang|gone]
    (
        RDIR=$T/ed; rm -rf "${RDIR:?}"; mkdir -p "$RDIR"; VOL=$RDIR/vol; RUN=$RDIR/run; SKYZFSD=$T/skyzfsd-ok
        HOLD_PID=""; WATCH_PID=""; SERVE_LOG=$RDIR/serve-3.log; echo "lease kept" > "$SERVE_LOG"
        log() { :; }
        rig_error() { echo "rig error: $*"; exit 2; }
        hook() { echo "verdict $1: $2"; }
        ON_DAEMON_WRONG=$1
        eval "$(fnc daemon_wrong)"; eval "$(fnc end_daemon)"
        if [ "${2:-}" = hang ]; then
            # A stop that SIGTERM does not end, with serve_end's bounds
            # cut to a second.
            eval "$(fnc serve_end | sed 's/^serve_end()/serve_end_real()/')"
            serve_end() { serve_end_real 1; }
            bash -c 'trap "" TERM; while :; do /bin/sleep 0.1; done' > /dev/null 2>&1 & SERVE_PID=$!
            echo "$SERVE_PID" > "$T/ed.pid"
        elif [ "${2:-}" = gone ]; then
            # Ended by itself, with 0, before the stop.
            eval "$(fnc serve_end)"
            ( exit 0 ) & SERVE_PID=$!
            while kill -0 "$SERVE_PID" 2>/dev/null; do /bin/sleep 0.05; done
        else
            eval "$(fnc serve_end)"
            bash -c 'trap "exit 1" TERM; while :; do /bin/sleep 0.1; done' & SERVE_PID=$!
        fi
        /bin/sleep 0.2
        end_daemon
        echo "went on, detached $([ -e "$RDIR/detach.log" ] && echo yes)"
    )
    # A stand-in left running by a stop that did not kill it.
    if [ -s "$T/ed.pid" ]; then kill -9 "$(cat "$T/ed.pid")" 2>/dev/null; rm -f "$T/ed.pid"; fi
}
is "end_daemon: a stop that ends 1, with no hook" "$(ed_t '')" "rig error: skyzfsd serve ended with 1 on SIGTERM: lease kept"
is "end_daemon: a stop that ends 1, the hook's verdict, and on" "$(ed_t hook)" \
    "verdict daemon_stopped: skyzfsd serve ended with 1 on SIGTERM: lease kept
went on, detached yes"
# A stop that SIGTERM does not end: the hook's verdict, then SIGKILL, and
# the course goes on (Y09, Y10 in G1 of #163's review).
is "end_daemon: a stop that hangs, the hook's verdict, killed, and on" "$(ed_t hook hang)" \
    "verdict daemon_stopped: skyzfsd serve did not end 60 s after SIGTERM: lease kept
went on, detached yes"

# A daemon that had ended before the SIGTERM ended unasked, even with 0:
# the hook's verdict, and on; with no hook, the rig's error (G-b of
# #164's review).
is "end_daemon: a daemon ended before the stop, the hook's verdict, and on" "$(ed_t hook gone)" \
    "verdict daemon_stopped: skyzfsd serve had ended before SIGTERM (status 0): lease kept
went on, detached yes"
is "end_daemon: a daemon ended before the stop, with no hook" "$(ed_t '' gone)" \
    "rig error: skyzfsd serve had ended before SIGTERM (status 0): lease kept"

# cleanup: a command that failed under errexit in a course with a hook:
# the hook judges, once, and its 1 is the status (D10); with no hook or
# no failure, nothing.
cl() { # HOOK_RC|none fail|ok
    (
        OUT=$T/cl; rm -rf "${OUT:?}"; mkdir -p "$OUT"; : > "$OUT/recordings.tsv"; NAME=arare
        STRAY=""; HOLD_PID=""; WATCH_PID=""; WRITER=""; AGENT_PID=""; S3_IS_DOWN=""; SERVE_PID=""; LOWER_UP=""
        VLOOP=""; LLOOP=""; MLOOP=""; SLOOP=""; MH_SAVED=""; NOFLUSH=$T/none/x; POOL=skzcut
        # The export under lim, never timeout(1), which waits for a
        # child that cannot be killed (run 45828678468).
        timeout() { echo "timeout $*" >> "$T/calls"; }
        lim() { echo "lim $*" >> "$T/calls"; }
        dmsetup() { :; }; log() { :; }
        hook() { echo "hook: $1" >> "$T/calls"; return "$HOOK_RC"; }
        SERVER=${CL_SERVER:-}; VOL=${CL_VOL:-}; RUN=$T/run; SKYZFSD=skyzfsd
        eval "$(fnc cleanup)"
        trap cleanup EXIT
        set -e
        [ "$1" = none ] || ON_RIG_ERROR=hook
        HOOK_RC=$1
        [ "$2" != ok ] || ON_RIG_ERROR=""
        [ "$2" != fail ] || false
        exit 0
    )
    echo "$? $(tr '\n' ';' 2>/dev/null < "$T/calls")$(cut -f2,3 "$T/cl/recordings.tsv" | tr '\n\t' ';,')"
    rm -f "${T:?}/calls"
}
is "cleanup: errexit with a hook that finds a verdict" "$(cl 1 fail)" \
    "1 hook: a command failed (status 1); the job log names it;lim 120 zpool export -f skzcut;rig error,a command failed (status 1); the job log names it;"
is "cleanup: errexit with a hook that finds none" "$(cl 0 fail)" \
    "2 hook: a command failed (status 1); the job log names it;lim 120 zpool export -f skzcut;rig error,a command failed (status 1); the job log names it;"
is "cleanup: errexit with no hook" "$(cl none fail)" "1 lim 120 zpool export -f skzcut;"
is "cleanup: an end with no failure" "$(cl 1 ok)" "0 lim 120 zpool export -f skzcut;"
# A daemon that had ended by the cleanup: its volume's device deleted
# all the same, under lim; none where the volume is gone (#164's review).
mkdir -p "$T/clvol"
is "cleanup: the device deleted with no daemon serving" "$(CL_SERVER=daemon CL_VOL=$T/clvol cl none ok)" \
    "0 lim 120 zpool export -f skzcut;lim 60 skyzfsd detach --dir $T/clvol --run-dir $T/run;"
is "cleanup: no volume, no detach" "$(CL_SERVER=daemon CL_VOL=$T/clgone cl none ok)" "0 lim 120 zpool export -f skzcut;"
is "cleanup: not through the daemon, no detach" "$(CL_VOL=$T/clvol cl none ok)" "0 lim 120 zpool export -f skzcut;"

# end: the hooks cleared once the course is recorded, before its check
# (the nit on ON_RIG_ERROR).
is "end: the hooks cleared before the check" \
    "$(fnc end | grep -n 'ON_RIG_ERROR=""; ON_DAEMON_WRONG=""\|\$CHECK "\$NAME"' | cut -d: -f1 | tr '\n' ' ' | awk '{ print ($1 < $2) }')" 1

# agent_judged: the agent, the store, the writer and the pool put away
# before the check (D7); agent_quiet's steps, none of them fatal.
judged_order() {
    (
        OUT=$T/jo; RDIR=$T/jo; CDIR=""; NAME=arare; VDEV_MB=1; rm -rf "${OUT:?}"; mkdir -p "$OUT"
        agent_quiet() { echo quiet >> "$T/calls"; }
        chk() { echo check >> "$T/calls"; return 1; }
        CHECK=chk
        eval "$(fn agent_judged)"
        agent_judged "zfs failed"
        echo "$? $(tr '\n' ';' < "$T/calls") $(cat "$RDIR/rig-error.tsv")"
    )
    rm -f "${T:?}/calls"
}
is "agent_judged: quiet, then the check; no case under way named none" "$(judged_order)" "$(printf '1 quiet;check; none\tzfs failed')"
eval "$(fn agent_quiet)"
quiet_t() { # EXPORT_RC [serving|ended|none [SCEN [CASE [killed]]]]
    (
        case ${2:-serving} in
            serving) sleep 30 > /dev/null 2>&1 & SERVE_PID=$! ;;
            # Ended by itself with 3, not yet waited for.
            ended) ( exit 3 ) & SERVE_PID=$!
                   while kill -0 "$SERVE_PID" 2>/dev/null; do sleep 0.05; done ;;
            none) SERVE_PID="" ;;
        esac
        SCEN=${3:-}; SERVE_K=2
        local serve=$SERVE_PID
        RDIR=$T/q2; WORK=$T/q2w; rm -rf "${RDIR:?}" "${WORK:?}"; mkdir -p "$RDIR" "$WORK"; touch "$WORK/watch.pause"; POOL=skzcut; NAME=arare
        sleep 30 > /dev/null 2>&1 & AGENT_PID=$!; kill -STOP "$AGENT_PID"; S3_IS_DOWN=1; S3_UP="echo up >> $T/calls"
        local agent=$AGENT_PID
        # Its output away from the command substitution, which would
        # otherwise wait for a writer left running.
        bash -c 'for _ in $(seq 1 600); do [ ! -e "$1" ] || exit 0; sleep 0.1; done' _ "$WORK/writer.stop" > /dev/null 2>&1 & WRITER=$!
        session_end() { echo session_end >> "$T/calls"; }
        lim() { echo "lim $*" >> "$T/calls"; return "$EXPORT_RC"; }
        log() { echo "log $*" >> "$T/calls"; }
        CDIR=""
        if [ -n "${4:-}" ]; then CDIR=$RDIR/cases/$4; mkdir -p "$CDIR"; fi
        [ "${5:-}" != killed ] || printf 'killed_rc\t3\nserve\t1\n' > "$CDIR/kill.tsv"
        printf 'line 1\nended badly\n' > "$RDIR/serve-2.log"
        EXPORT_RC=$1 agent_quiet
        local rc=$?
        kill -0 "$agent" 2>/dev/null && { kill -9 "$agent"; AGENT_PID="$agent left"; }
        [ -z "$serve" ] || { kill -9 "$serve"; wait "$serve"; } 2>/dev/null || true
        echo "$rc ${AGENT_PID:-none} ${S3_IS_DOWN:-thawed} ${WRITER:-stopped} $([ -e "$WORK/watch.pause" ] && echo paused || echo watching) $(tr '\n' ';' < "$T/calls")$(cd "$RDIR" && for v in cases/*/verdicts.tsv; do [ ! -e "$v" ] || printf '|%s:%s' "$v" "$(tr '\t\n' ',;' < "$v")"; done)${SERVE_PID:+ pid kept}"
        touch "$WORK/writer.stop"
    )
    rm -f "${T:?}/calls"
}
is "agent_quiet: everything put away, the pool exported" "$(quiet_t 0)" \
    "0 none thawed stopped watching up;session_end;lim 120 zpool export skzcut;log arare: skzcut exported for the check after the rig's error; pid kept"
is "agent_quiet: a pool that does not export is said, not fatal" "$(quiet_t 1)" \
    "0 none thawed stopped watching up;session_end;lim 120 zpool export skzcut;log arare: skzcut did not export for the check: ; pid kept"
# With no daemon serving (the rig's error came while it was down), no
# export is tried: it would wait its limit on a device with no server
# (run 45828678468).
is "agent_quiet: no export with no daemon" "$(quiet_t 0 none)" \
    "0 none thawed stopped watching up;session_end;log arare: skzcut not exported for the check: no daemon serves its device;"
# A daemon that had ended by itself: a verdict with its status, in the
# case under way or the course's (G5 of #163's review), and no export;
# in a dhook case, said only.
is "agent_quiet: the daemon ended by itself, outside a case" "$(quiet_t 0 ended)" \
    "0 none thawed stopped watching up;session_end;log arare: case course: verdict daemon_serving: skyzfsd serve 2 had ended (status 3) with no kill of the rig's, seen after the rig's error: line 1;ended badly;log arare: skzcut not exported for the check: no daemon serves its device;|cases/course/verdicts.tsv:daemon_serving,skyzfsd serve 2 had ended (status 3) with no kill of the rig's, seen after the rig's error: line 1 ended badly;"
is "agent_quiet: the daemon ended by itself, in a case" "$(quiet_t 0 ended restart r1-k)" \
    "0 none thawed stopped watching up;session_end;log arare: case r1-k: verdict daemon_serving: skyzfsd serve 2 had ended (status 3) with no kill of the rig's, seen after the rig's error: line 1;ended badly;log arare: skzcut not exported for the check: no daemon serves its device;|cases/r1-k/verdicts.tsv:daemon_serving,skyzfsd serve 2 had ended (status 3) with no kill of the rig's, seen after the rig's error: line 1 ended badly;"
is "agent_quiet: the daemon ended in a dhook case, said only" "$(quiet_t 0 ended dhook h1)" \
    "0 none thawed stopped watching up;session_end;log arare: skyzfsd serve 2 had ended (status 3) in a dhook case, after the rig's own error;log arare: skzcut not exported for the check: no daemon serves its device;"
# After the hook's end is recorded, the instance served again has no
# hook: a verdict (G-a of #164's review).
is "agent_quiet: dhook's exemption only until the hook's end" "$(quiet_t 0 ended dhook h1 killed)" \
    "0 none thawed stopped watching up;session_end;log arare: case h1: verdict daemon_serving: skyzfsd serve 2 had ended (status 3) with no kill of the rig's, seen after the rig's error: line 1;ended badly;log arare: skzcut not exported for the check: no daemon serves its device;|cases/h1/verdicts.tsv:daemon_serving,skyzfsd serve 2 had ended (status 3) with no kill of the rig's, seen after the rig's error: line 1 ended badly;"
is "agent_quiet: dhook's exemption only in a case" "$(quiet_t 0 ended dhook)" \
    "0 none thawed stopped watching up;session_end;log arare: case course: verdict daemon_serving: skyzfsd serve 2 had ended (status 3) with no kill of the rig's, seen after the rig's error: line 1;ended badly;log arare: skzcut not exported for the check: no daemon serves its device;|cases/course/verdicts.tsv:daemon_serving,skyzfsd serve 2 had ended (status 3) with no kill of the rig's, seen after the rig's error: line 1 ended badly;"
is "agent_quiet: a serving daemon, no verdict" "$(quiet_t 0 serving restart r1-k)" \
    "0 none thawed stopped watching up;session_end;lim 120 zpool export skzcut;log arare: skzcut exported for the check after the rig's error; pid kept"
# Its waits are bounded: an agent and a writer that SIGKILL does not end
# (a kill stub here) do not hold it; seq is cut to one try (X25, X26 in
# G1 of #163's review).
quiet_hang() {
    # shellcheck disable=SC2329
    (
        RDIR=$T/qh; WORK=$T/qhw; rm -rf "${RDIR:?}" "${WORK:?}"; mkdir -p "$RDIR" "$WORK"
        POOL=skzcut; NAME=arare; SCEN=""; CDIR=""; S3_IS_DOWN=""; SERVE_PID=""
        sleep 20 > /dev/null 2>&1 & AGENT_PID=$!; local agent=$AGENT_PID
        sleep 20 > /dev/null 2>&1 & WRITER=$!; local writer=$WRITER
        kill() { [ "$1" = -9 ] || builtin kill "$@"; }
        # One try each, the bounds asked for kept (N17 of #164's review).
        seq() { echo "$*" >> "$T/qh.seq"; echo 1; }
        session_end() { :; }; log() { :; }; lim() { return 0; }
        local t0=$SECONDS took
        agent_quiet
        took=$((SECONDS - t0))
        builtin kill -9 "$agent" "$writer" 2>/dev/null; wait "$agent" "$writer" 2>/dev/null
        if [ "$took" -lt 8 ]; then echo "bounded ${AGENT_PID:-none} ${WRITER:-stopped}"; else echo "waited $took s"; fi
    )
}
rm -f "$T/qh.seq"
is "agent_quiet: an agent and a writer that do not end do not hold it" "$(quiet_hang)" "bounded none stopped"
# The bounds: 20 tries of 0.25 s for the agent, 120 of 0.5 s for the
# writer.
is "agent_quiet: its bounds" "$(tr '\n' ';' < "$T/qh.seq")" "1 20;1 120;"

[ "$fails" = 0 ] && echo "all passed"
exit $((fails > 0))
