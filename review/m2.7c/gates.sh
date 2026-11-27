#!/bin/bash
# Gates for PR 173 at 29f5538, in a scratch worktree (pr173); the target
# is cleaned between feature sets. Then the agent and control crates'
# tests twenty times. SCRATCH names the scratch directory.
S=$SCRATCH
cd $S/pr173 || exit 9
git checkout -q --detach 29f5538fab63aede594935f4294a9fee0964b781 || exit 9
export CARGO_TARGET_DIR=$S/t173 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
R=$S/m27c/out
OUT=$R/gates.out
: > $OUT
echo "head $(git rev-parse HEAD)" >> $OUT
rustc --version >> $OUT
cargo fmt --all -- --check > $R/fmt.log 2>&1; echo "fmt rc=$?" >> $OUT
for fs in "" "skyzfsd/test-hooks,skyzfs-local/test-hooks" "skyzfs-ublk-sys/control-test-program" "skyzfs-agent-cli/test-hooks"; do
  rm -rf $S/t173
  tag=${fs:-plain}; tag=${tag//\//_}; tag=${tag//,/+}
  F=(); [ -n "$fs" ] && F=(--features "$fs")
  cargo clippy --workspace --all-targets "${F[@]}" -- -D warnings > $R/clippy-$tag.log 2>&1; echo "clippy $tag rc=$?" >> $OUT
  cargo test --workspace "${F[@]}" > $R/test-$tag.log 2>&1; rc=$?
  p=$(grep -E '^test result:' $R/test-$tag.log | awk '{p+=$4; f+=$6; i+=$8} END {print p" passed "f" failed "i" ignored"}')
  echo "test $tag rc=$rc $p" >> $OUT
  RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps "${F[@]}" > $R/doc-$tag.log 2>&1; echo "doc workspace $tag rc=$?" >> $OUT
  if [ -z "$fs" ]; then
    for c in skyzfs-agent skyzfs-control skyzfs-ublk; do
      grep -E "^test result:|Running .*(${c//-/_})" $R/test-$tag.log | head -0 >/dev/null
    done
    fails=0
    for i in $(seq 1 20); do
      cargo test -p skyzfs-agent -p skyzfs-agent-cli -p skyzfs-control > $R/agent-x20.log 2>&1 || { fails=$((fails+1)); cp $R/agent-x20.log $R/agent-fail-$i.log; }
    done
    echo "agent, agent-cli and control tests x20: $fails failed runs (last: $(grep -E '^test result:' $R/agent-x20.log | awk '{p+=$4; f+=$6} END {print p" passed "f" failed"}'))" >> $OUT
  fi
  df -h / | tail -1 >> $OUT
done
rm -rf $S/t173
echo done >> $OUT
