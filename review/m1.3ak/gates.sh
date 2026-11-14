set -u
cd /home/user/skyzfs-pr91
export CARGO_TARGET_DIR=/home/user/target-pr73
rustc --version
echo "=== fmt"; cargo fmt --all --check >/dev/null 2>&1; echo "rc $?"
echo "=== clippy workspace"; cargo clippy --workspace --all-targets -- -D warnings > $1/clippy.log 2>&1; echo "rc $?"
echo "=== clippy test-hooks"; cargo clippy --workspace --all-targets --features skyzfsd/test-hooks,skyzfs-local/test-hooks -- -D warnings > $1/clippy-hooks.log 2>&1; echo "rc $?"
echo "=== tests skyzfsd plain"; cargo test -p skyzfsd > $1/tests-plain.log 2>&1; echo "rc $?"
grep "^test result:" $1/tests-plain.log | awk '{p+=$4; f+=$6; n++} END {print "binaries", n, "passed", p, "failed", f}'
echo "=== tests skyzfsd test-hooks"; cargo test -p skyzfsd --features test-hooks > $1/tests-hooks.log 2>&1; echo "rc $?"
grep "^test result:" $1/tests-hooks.log | awk '{p+=$4; f+=$6; n++} END {print "binaries", n, "passed", p, "failed", f}'
grep -c "the_give_ups_commands_share_one_limit\|a_start_given_up_ends_within_one_limit\|a_delete_with_no_answer_gives_9_and_a_line" $1/tests-plain.log $1/tests-hooks.log
echo "=== repeats of the three new tests: 30 alone, then 30 with every CPU busy"
F="serve::tests::the_give_ups serve::tests::a_start_given_up_ends serve::tests::a_delete_with_no_answer_gives_9"
cargo test -q -p skyzfsd --lib --no-run >/dev/null 2>&1
for i in $(seq 1 30); do cargo test -q -p skyzfsd --lib -- $F 2>&1 | grep -q "3 passed" && echo ok || echo FAIL; done | sort | uniq -c
echo "nproc $(nproc)"
pids=""
for c in $(seq 1 $(nproc)); do timeout 900 sh -c 'while :; do :; done' & pids="$pids $!"; done
for i in $(seq 1 30); do cargo test -q -p skyzfsd --lib -- $F > $1/loaded-$i.log 2>&1; grep -q "3 passed" $1/loaded-$i.log && echo ok || echo FAIL; done | sort | uniq -c
kill $pids 2>/dev/null; wait 2>/dev/null
grep -h "took" $1/loaded-*.log | head -5
echo "=== plants"
python3 -I $1/plants.py
