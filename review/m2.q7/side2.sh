#!/bin/bash
# Side runs in the second worktree: the connect's tries (head, CE, CF,
# and the head under CPU load), then the lease's conformance over a
# local S3 endpoint (moto), on the head and with O1 planted.
S=$1
W=$S/head2
O=$S/q7
export CARGO_TARGET_DIR=$S/q7/target2 CARGO_INCREMENTAL=0
cd $W
CTT=bins/skyzfsd/src/control/tests.rs
CT=bins/skyzfsd/src/control.rs
anchor='    /// The errors of `accept` after which the channel'"'"'s thread takes the'
python3 - "$W/$CTT" "$O/scenario_tries.rs" "$anchor" <<'PY'
import sys
p, s, a = sys.argv[1], sys.argv[2], sys.argv[3]
t = open(p).read(); assert t.count(a) == 1
open(p, "w").write(t.replace(a, open(s).read() + a))
PY
run_tries() {
  timeout 1200 cargo test -p skyzfsd --lib -- review_q7_tries --nocapture > "$O/tries_$1.out" 2>&1; echo "exit $?" >> "$O/tries_$1.out"
}
run_tries head
# Under load: four busy loops beside the test and the PR's own tests, 10 runs.
for i in 1 2 3 4; do (timeout 900 sh -c 'while :; do :; done' &) ; done
run_tries head_loaded
for i in $(seq 1 10); do
  timeout 600 cargo test -p skyzfsd --lib -- control::tests::socket:: >> "$O/socket_loaded.out" 2>&1; echo "exit $?" >> "$O/socket_loaded.out"
done
pkill -f "while :; do :; done"
plant() { python3 - "$W/$CT" "$1" "$2" <<'PY'
import sys
p, old, new = sys.argv[1:4]
t = open(p).read(); assert t.count(old) == 1, old
open(p, "w").write(t.replace(old, new))
PY
}
plant '                pause = (pause * 2).min(CONNECT_PAUSE);' '                pause = (pause * 2).min(CONNECT_PAUSE * 1000);'
run_tries CE
git checkout -- $CT
plant '    let mut pause = Duration::from_millis(1);' '    let mut pause = CONNECT_PAUSE;'
run_tries CF
git checkout -- $CT
git checkout -- $CTT
# The lease over a local S3 endpoint.
$S/motoenv/bin/python -m moto.server -p 5077 > "$O/moto.log" 2>&1 &
MOTO=$!
for i in $(seq 1 30); do curl -s -o /dev/null --noproxy '*' http://127.0.0.1:5077 && break; sleep 1; done
s3() {
  NO_PROXY=127.0.0.1,localhost no_proxy=127.0.0.1,localhost SKYNBD_S3_TEST_ENDPOINT=http://127.0.0.1:5077 AWS_ACCESS_KEY_ID=test AWS_SECRET_ACCESS_KEY=test \
    timeout 1200 cargo test -p skynbd-refs --features skynbd-store/s3 --test object_refs_s3 -- --nocapture --test-threads 1 > "$O/s3_$1.out" 2>&1; echo "exit $?" >> "$O/s3_$1.out"
}
s3 head
OBJ=crates/skynbd-refs/src/object_store.rs
python3 - "$W/$OBJ" <<'PY'
import sys
p = sys.argv[1]
old = "            let (write, outcome) = lease::renew(current.as_ref(), &holder, epoch, ttl_ms, clock());\n"
new = ("            let me = Lease { holder: holder.clone(), epoch, expires_at_ms: 0 };\n"
       "            let _ = &current;\n"
       "            let (write, outcome) = lease::renew(Some(&me), &holder, epoch, ttl_ms, clock());\n")
t = open(p).read(); assert t.count(old) == 1
open(p, "w").write(t.replace(old, new))
PY
s3 O1
git checkout -- $OBJ
kill $MOTO
# The uploader's suite with the header synced, as the daemon now runs a volume that uploads.
UC=crates/skyzfs-upload/tests/common/mod.rs
python3 - "$W/$UC" <<'PY2'
import sys
p = sys.argv[1]
t = open(p).read(); old = "                intent_header_sync: false,\n"; assert t.count(old) == 1
open(p, "w").write(t.replace(old, "                intent_header_sync: true,\n"))
PY2
timeout 2400 cargo test -p skyzfs-upload --tests > "$O/upload_synced.out" 2>&1; echo "exit $?" >> "$O/upload_synced.out"
git checkout -- $UC
git status --short > "$O/side2_tree.out"; echo done >> "$O/side2_tree.out"
