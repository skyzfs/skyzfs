#!/bin/bash
# Gates for PR 179 at its head, in a scratch worktree of the head.
# The target is cleaned between feature sets (disk space).
S=${SCRATCH:?set SCRATCH to a scratch directory with a worktree of the head at $SCRATCH/r179/head}
cd $S/r179/head || exit 9
export CARGO_TARGET_DIR=$S/t179 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
L=$S/r179/logs
OUT=$S/r179/notes/review/m2.7b2/gates.out
: > $OUT
echo "head $(git rev-parse HEAD)" >> $OUT
rustc --version >> $OUT
cargo fmt --all -- --check > $L/g-fmt.log 2>&1; echo "fmt rc=$?" >> $OUT
first=1
for fs in "" "skyzfsd/test-hooks,skyzfs-local/test-hooks" "skyzfs-ublk-sys/control-test-program" "skyzfs-agent-cli/test-hooks"; do
  [ $first = 1 ] || rm -rf $S/t179
  first=0
  tag=${fs:-plain}; tag=${tag//\//_}; tag=${tag//,/+}
  F=(); [ -n "$fs" ] && F=(--features "$fs")
  cargo clippy --workspace --all-targets "${F[@]}" -- -D warnings > $L/g-clippy-$tag.log 2>&1; echo "clippy $tag rc=$?" >> $OUT
  cargo test --workspace "${F[@]}" > $L/g-test-$tag.log 2>&1; rc=$?
  p=$(grep -E '^test result:' $L/g-test-$tag.log | awk '{p+=$4; f+=$6; i+=$8} END {print p" passed "f" failed "i" ignored"}')
  echo "test $tag rc=$rc $p" >> $OUT
  RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps "${F[@]}" > $L/g-doc-$tag.log 2>&1; echo "doc $tag rc=$?" >> $OUT
  df -h / | tail -1 >> $OUT
done
# The agent's crates' tests ten times more (timing-sensitive program tests).
for i in $(seq 1 10); do
  cargo test -p skyzfs-agent -p skyzfs-agent-cli --features skyzfs-agent-cli/test-hooks > $L/g-agent-rep-$i.log 2>&1
  echo "agent repeat $i rc=$? $(grep -E '^test result:' $L/g-agent-rep-$i.log | awk '{p+=$4; f+=$6} END {print p" passed "f" failed"}')" >> $OUT
done
echo done >> $OUT
