#!/bin/bash
# Gates for PR 170 at bb03454, in a scratch worktree; the target is cleaned
# between feature sets. Then the host and agent tests twenty times, and a
# debug build of the binary for the signal driver (live.sh).
S=$SCRATCH
cd $S/pr170 || exit 9
git checkout -q --detach bb034546b34be39e665cd869bb6a8de220b61e85 || exit 9
export CARGO_TARGET_DIR=$S/t170 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
R=$S/r170
OUT=$R/gates.out
: > $OUT
echo "head $(git rev-parse HEAD)" >> $OUT
rustc --version >> $OUT
cargo fmt --all -- --check > $R/fmt.log 2>&1; echo "fmt rc=$?" >> $OUT
for fs in "" "skyzfsd/test-hooks,skyzfs-local/test-hooks" "skyzfs-ublk-sys/control-test-program" "skyzfs-agent-cli/test-hooks"; do
  rm -rf $S/t170
  tag=${fs:-plain}; tag=${tag//\//_}; tag=${tag//,/+}
  F=(); [ -n "$fs" ] && F=(--features "$fs")
  cargo clippy --workspace --all-targets "${F[@]}" -- -D warnings > $R/clippy-$tag.log 2>&1; echo "clippy $tag rc=$?" >> $OUT
  cargo test --workspace "${F[@]}" > $R/test-$tag.log 2>&1; rc=$?
  p=$(grep -E '^test result:' $R/test-$tag.log | awk '{p+=$4; f+=$6; i+=$8} END {print p" passed "f" failed "i" ignored"}')
  echo "test $tag rc=$rc $p" >> $OUT
  RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps "${F[@]}" > $R/doc-$tag.log 2>&1; echo "doc workspace $tag rc=$?" >> $OUT
  if [ -z "$fs" ]; then
    fails=0
    for i in $(seq 1 20); do
      cargo test -p skyzfs-agent -p skyzfs-agent-cli > $R/agent-x20.log 2>&1 || { fails=$((fails+1)); cp $R/agent-x20.log $R/agent-fail-$i.log; }
    done
    echo "agent crates' tests x20: $fails failed runs (last: $(grep -E '^test result:' $R/agent-x20.log | awk '{p+=$4; f+=$6} END {print p" passed "f" failed"}'))" >> $OUT
    fails=0
    for i in $(seq 1 20); do
      cargo test -p skyzfs-agent --lib host:: > $R/host-x20.log 2>&1 || { fails=$((fails+1)); cp $R/host-x20.log $R/host-fail-$i.log; }
    done
    echo "host tests x20: $fails failed runs ($(grep -E '^test result:' $R/host-x20.log))" >> $OUT
    cargo build -p skyzfs-agent-cli > $R/build.log 2>&1; echo "build bin rc=$?" >> $OUT
    mkdir -p $R/bin && cp $S/t170/debug/skyzfs-agent $R/bin/skyzfs-agent
  fi
  df -h / | tail -1 >> $OUT
done
rm -rf $S/t170
echo done >> $OUT
