#!/bin/bash
# Gates at the fourth head (e6530f1). Its commits after adc391a
# touch crates/skyzfs-agent only, so the agent's crates are gated here
# (fmt on the workspace); adc391a had the full gates (gates.sh).
S=${SCRATCH:?set SCRATCH to a scratch directory with a worktree of the head at $SCRATCH/r179/head3}
cd $S/r179/head3 || exit 9
export CARGO_TARGET_DIR=$S/t179c CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
L=$S/r179/logs
OUT=$S/r179/notes/review/m2.7b2/gates3.out
: > $OUT
echo "head $(git rev-parse HEAD)" >> $OUT
echo "changed since adc391a: $(git diff --name-only adc391a HEAD | tr '\n' ' ')" >> $OUT
rustc --version >> $OUT
cargo fmt --all -- --check > $L/g3-fmt.log 2>&1; echo "fmt rc=$?" >> $OUT
P=(-p skyzfs-agent -p skyzfs-agent-cli)
for fs in "" "skyzfs-agent-cli/test-hooks"; do
  tag=${fs:-plain}; tag=${tag//\//_}
  F=(); [ -n "$fs" ] && F=(--features "$fs")
  cargo clippy "${P[@]}" --all-targets "${F[@]}" -- -D warnings > $L/g3-clippy-$tag.log 2>&1; echo "clippy agent $tag rc=$?" >> $OUT
  cargo test "${P[@]}" "${F[@]}" > $L/g3-test-$tag.log 2>&1; rc=$?
  p=$(grep -E '^test result:' $L/g3-test-$tag.log | awk '{p+=$4; f+=$6; i+=$8} END {print p" passed "f" failed "i" ignored"}')
  echo "test agent $tag rc=$rc $p" >> $OUT
  RUSTDOCFLAGS='-D warnings' cargo doc "${P[@]}" --no-deps "${F[@]}" > $L/g3-doc-$tag.log 2>&1; echo "doc agent $tag rc=$?" >> $OUT
done
for i in $(seq 1 3); do
  cargo test "${P[@]}" --features skyzfs-agent-cli/test-hooks > $L/g3-rep-$i.log 2>&1
  echo "agent repeat $i rc=$? $(grep -E '^test result:' $L/g3-rep-$i.log | awk '{p+=$4; f+=$6} END {print p" passed "f" failed"}')" >> $OUT
done
echo done >> $OUT
