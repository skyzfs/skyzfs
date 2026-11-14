#!/bin/bash
# Tests of the daemon rig's refused-commit courses (check e, M1.T35):
# the rig's own rc_course and check_e_refused, run against a made-up
# server that does what `serve --refuse-one-commit` of a test-hooks
# build does (#83) and against servers wrong on purpose. No kernel, no
# ublk and no daemon are needed.
# Run: bash tests/kernel/test_daemon_refused.sh
# The rig's functions are cut from it and eval'ed: they read the
# variables set here and call the functions made up here.
# shellcheck disable=SC2034,SC2329
HERE=$(cd "$(dirname "$0")" && pwd)
RIG=$HERE/daemon-rig.sh
# shellcheck source=tests/lib/lim.sh
. "$HERE/../lib/lim.sh"
# shellcheck source=tests/kernel/looks.sh
. "$HERE/looks.sh"
if ! T=$(mktemp -d) || [ -z "$T" ] || [ ! -d "$T" ]; then echo "no temporary directory"; exit 2; fi
# The made-up server and client wait for this file: none outlives the test.
trap 'touch "$T/deleted"; sleep 0.5; rm -rf "${T:?}"' EXIT
fails=0
is() { # name got want
    if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got '$2', want '$3'"; fails=$((fails + 1)); fi
}
has() { # name got part
    case $2 in *"$3"*) echo "ok   $1" ;; *) echo "FAIL $1: '$3' not in '$2'"; fails=$((fails + 1)) ;; esac
}

# The rig's constants and functions, as the rig has them. A one-line
# function is cut at its own line.
eval "$(sed -n '/^EXIT_FAILED=/p; /^HOLD_AT=/p; /^RC_AT=/p; /^RC_PINS=/p' "$RIG")"
for f in rec pin now_ms serve_wait job_end rc_pin rc_cleanup rc_course check_e_refused; do
    eval "$(sed -n "/^$f() .*}\$/{p;q}; /^$f() {/,/^}\$/p" "$RIG")"
    declare -F "$f" > /dev/null || { echo "FAIL the rig has no function $f"; exit 2; }
done
[ "$RC_AT" -gt 0 ] && [ -n "$RC_PINS" ] || { echo "FAIL the rig has no RC_AT or RC_PINS"; exit 2; }

OUT=$T/out; WORK=$T/work; mkdir -p "$OUT" "$WORK"
R=$OUT/results.tsv; : > "$R"
HOLD_S=300; SKYZFSD=skyzfsd
export T

# The made-up server. SCEN says how it goes wrong; `ok` is what #83
# says the daemon does. The device's state is $T/state; a request is a
# file $T/req.ID with its kind, answered by $T/done.ID (`lost`: answered
# and not carried out).
FS=$T/server
cat > "$FS" <<'FSEOF'
#!/bin/bash
dir=$1; shift; refuse=""; hold=""
while [ $# -gt 0 ]; do case $1 in --refuse-one-commit) refuse=${2%%:*}; shift ;; --hold-one-request) hold=1; shift ;; esac; shift; done
echo live > "$T/state"; rm -f "$T/held"
held_id=""; refused=""
ended_8() {
    echo quiesced > "$T/state"
    [ "$SCEN" = noline ] || echo "skyzfsd: the kernel refused serve's answer on tag 0 of device 9 (Invalid argument (os error 22)): nothing more is answered on this queue" >&2
    [ "$SCEN" != live ] || echo live > "$T/state"
    [ "$SCEN" != exit3 ] || exit 3
    exit 8
}
on_term() {
    if [ -n "$held_id" ]; then
        if [ "$SCEN" = stop0 ]; then echo quiesced > "$T/state"; exit 0; fi
        ended_8
    fi
    [ "$SCEN" = homeless ] || cp --sparse=always "$T/img" "$dir/home.img" 2>/dev/null
    echo quiesced > "$T/state"; exit 0
}
trap on_term TERM
while :; do
    for f in "$T"/req.*; do
        case $f in *.tmp) continue ;; esac
        [ -e "$f" ] || continue
        id=${f#"$T"/req.}
        [ ! -e "$T/done.$id" ] && [ "$id" != "$held_id" ] || continue
        k=$(cat "$f")
        if [ -n "$refuse" ] && [ "$k" = "$refuse" ] && [ -z "$refused" ]; then
            refused=1
            if [ -n "$hold" ]; then held_id=$id; touch "$T/held"; continue; fi
            [ "$SCEN" != answers ] || echo done > "$T/done.$id"
            ended_8
        fi
        if [ -z "$refuse" ] && { [ "$SCEN/$k" = lost/write ] || [ "$SCEN/$k" = lostread/read ]; }; then echo lost > "$T/done.$id"; else echo done > "$T/done.$id"; fi
    done
    sleep 0.05
done
FSEOF
chmod +x "$FS"
# The client's dd: a request of the device waits for its answer, then
# is carried out on $T/img; any other dd is the real one.
DD=$T/dd
cat > "$DD" <<'DDEOF'
#!/bin/bash
dev=/dev/ublkb9; args=(); kind=""; fsync=""
for a in "$@"; do
    case $a in
        "if=$dev") kind=read; args+=("if=$T/img") ;;
        "of=$dev") kind=write; args+=("of=$T/img") ;;
        iflag=*|oflag=*) ;;
        conv=*fsync*) fsync=1; args+=(conv=notrunc) ;;
        *) args+=("$a") ;;
    esac
done
[ -n "$kind" ] || exec dd "$@"
req() {
    local id=$$.$RANDOM
    echo "$1" > "$T/req.$id.tmp"; mv "$T/req.$id.tmp" "$T/req.$id"
    while [ ! -e "$T/done.$id" ]; do [ ! -e "$T/deleted" ] || exit 5; sleep 0.05; done
    [ "$(cat "$T/done.$id")" != lost ]
}
if req "$kind"; then dd "${args[@]}" || exit 1; fi
[ -z "$fsync" ] || req flush || true
exit 0
DDEOF
chmod +x "$DD"
vol_up() { local impl=$1 dir=$2; shift 2; mkdir -p "$dir"; rm -f "$T"/req.* "$T"/done.* "$T/deleted" "$T/img" "$T/held"; serve_start "$impl" "$dir" "$@"; }
serve_start() {
    local dir=$2; shift 2
    rm -f "$T/state"
    "$FS" "$dir" "$@" 2>> "$dir.serve.log" &
    SERVE_PID=$!; BDEV=/dev/ublkb9; SERVE_RC=""
    for _ in $(seq 1 100); do [ "$(cat "$T/state" 2>/dev/null)" = live ] && return 0; sleep 0.05; done
    SERVE_RC="none"; return 2
}
ublk_state() { case $(cat "$T/state" 2>/dev/null) in live) echo LIVE ;; quiesced) echo QUIESCED ;; *) echo DEAD ;; esac; }
vol_stat() { [ "$3" = held ] && { [ -e "$T/held" ] && echo 1 || echo 0; }; }
# A new volume is a new device: vol_up starts with nothing of the last.
vol_detach() { touch "$T/deleted"; }
course() { :; }
step() { :; }

course_of() { # scenario name kind [hold] -> RC_V, RCT_WHY
    SCEN=$1; export SCEN; shift
    rm -f "$T"/req.* "$T"/done.* "$T/deleted" "$T/img" "$T/held"
    rc_course "$@" > /dev/null 2>&1; RC_V=$?
}

for k in read write flush; do
    course_of ok "t-$k" "$k"
    is "a refused $k, then a take-over: PASS" "$RC_V" 0
    has "  serve's exit and line" "$RCT_WHY" "serve ended with exit 8 after"
    has "  and its words" "$RCT_WHY" "saying: skyzfsd: the kernel refused serve's answer on tag 0"
    has "  the device waited" "$RCT_WHY" "the device then read QUIESCED; the client still waited 2 s after"
    has "  the client ended after the take-over" "$RCT_WHY" "after the take-over the client exited 0"
    has "  the stop and the home image" "$RCT_WHY" "the next stop: exit 0; the home image's block: same"
done
course_of ok t-read2 read
has "the read returns what was written" "$RCT_WHY" "the block read back through the device: same, and the client read it"

course_of ok t-stop write hold
is "a held write refused at a stop: PASS" "$RC_V" 0
has "  says it was stopped while held" "$RCT_WHY" "SIGTERM while the write was held; serve ended with exit 8"
course_of stop0 t-stop0 write hold
is "the negative control: a stop that ends 0 fails" "$RC_V" 1
has "  and says so" "$RCT_WHY" "serve ended with exit 0"

course_of answers t-ans write
is "a refused request that the server answered all the same fails" "$RC_V" 1
has "  the client did not wait" "$RCT_WHY" "the client had ended (0"
course_of noline t-noline write
is "an 8 with no line fails" "$RC_V" 1
course_of live t-live write
is "a device that does not wait fails" "$RC_V" 1
has "  says what it read" "$RCT_WHY" "the device then read LIVE"
course_of lost t-lost write
is "a take-over that loses the write fails" "$RC_V" 1
has "  the block differs" "$RCT_WHY" "read back through the device: DIFFERENT"
course_of lostread t-lostread read
is "a take-over that loses the read fails" "$RC_V" 1
has "  the client read something else" "$RCT_WHY" "the client read SOMETHING ELSE"
course_of exit3 t-exit3 write
is "the line with another exit status fails" "$RC_V" 1
has "  says which" "$RCT_WHY" "serve ended with exit 3"
course_of homeless t-home write
is "a write not in the home image fails" "$RC_V" 1
has "  says so" "$RCT_WHY" "the home image's block: DIFFERENT"

# A build without the hook: serve does not start. The rig's error.
vol_up() { SERVE_RC=2; echo "skyzfsd: unknown option --refuse-one-commit" >> "$2.serve.log"; return 1; }
course_of ok t-old write
is "a build without the hook is the rig's error" "$RC_V" 2
has "  and says why" "$RCT_WHY" "unknown option --refuse-one-commit"
vol_up() { local impl=$1 dir=$2; shift 2; mkdir -p "$dir"; rm -f "$T"/req.* "$T"/done.* "$T/deleted" "$T/img" "$T/held"; serve_start "$impl" "$dir" "$@"; }

# The check's wiring: four pins on the daemon, WAITING on the stand-in.
: > "$R"; IMPL=daemon; SCEN=ok; export SCEN
rm -f "$T"/req.* "$T"/done.* "$T/deleted" "$T/img"
check_e_refused > /dev/null 2>&1
for p in $RC_PINS; do is "the daemon: pin_$p" "$(awk -F'\t' -v k="pin_$p" '$1 == k { split($2, a, ":"); print a[1] }' "$R")" PASS; done
: > "$R"; IMPL=reference
check_e_refused > /dev/null 2>&1
for p in $RC_PINS; do is "the stand-in: pin_$p" "$(awk -F'\t' -v k="pin_$p" '$1 == k { split($2, a, ":"); print a[1] }' "$R")" WAITING; done
is "four pins" "$(wc -w <<< "$RC_PINS" | tr -d ' ')" 4

if [ "$fails" = 0 ]; then echo "all passed"; else echo "$fails failed"; exit 1; fi
