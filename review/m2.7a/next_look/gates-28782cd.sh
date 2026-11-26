#!/bin/bash
# Gates for PR 167 at 28782cd, in a scratch worktree; the target is cleaned
# between feature sets. Then the host tests twenty times, and a debug build
# of the binary for the signal driver (live.sh).
S=$SCRATCH
cd $S/wt/pr167 || exit 9
git checkout -q --detach 28782cddb8423a5a05c9d34a60266e0075e32c8d || exit 9
export CARGO_TARGET_DIR=$S/t167 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
OUT=$S/r167/nl-gates.out
: > $OUT
echo "head $(git rev-parse HEAD)" >> $OUT
rustc --version >> $OUT
cargo fmt --all -- --check > $S/r167/nl-fmt.log 2>&1; echo "fmt rc=$?" >> $OUT
for fs in "" "skyzfsd/test-hooks,skyzfs-local/test-hooks" "skyzfs-ublk-sys/control-test-program" "skyzfs-agent-cli/test-hooks"; do
  rm -rf $S/t167
  tag=${fs:-plain}; tag=${tag//\//_}; tag=${tag//,/+}
  F=(); [ -n "$fs" ] && F=(--features "$fs")
  cargo clippy --workspace --all-targets "${F[@]}" -- -D warnings > $S/r167/nl-clippy-$tag.log 2>&1; echo "clippy $tag rc=$?" >> $OUT
  cargo test --workspace "${F[@]}" > $S/r167/nl-test-$tag.log 2>&1; rc=$?
  p=$(grep -E '^test result:' $S/r167/nl-test-$tag.log | awk '{p+=$4; f+=$6; i+=$8} END {print p" passed "f" failed "i" ignored"}')
  echo "test $tag rc=$rc $p" >> $OUT
  RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps "${F[@]}" > $S/r167/nl-doc-$tag.log 2>&1; echo "doc workspace $tag rc=$?" >> $OUT
  RUSTDOCFLAGS='-D warnings' cargo doc -p skyzfs-agent -p skyzfs-agent-cli --no-deps "${F[@]}" > $S/r167/nl-doca-$tag.log 2>&1; echo "doc agent crates $tag rc=$?" >> $OUT
  if [ -z "$fs" ]; then
    # The host tests twenty times (the race of eedd83e), and the binary.
    fails=0
    for i in $(seq 1 20); do
      cargo test -p skyzfs-agent --lib host:: > $S/r167/nl-host.log 2>&1 || { fails=$((fails+1)); cp $S/r167/nl-host.log $S/r167/nl-host-fail-$i.log; }
    done
    echo "host tests x20: $fails failed runs ($(grep -E '^test result:' $S/r167/nl-host.log))" >> $OUT
    cargo build -p skyzfs-agent-cli > $S/r167/nl-build.log 2>&1; echo "build bin rc=$?" >> $OUT
    mkdir -p $S/r167/bin && cp $S/t167/debug/skyzfs-agent $S/r167/bin/skyzfs-agent
  fi
  df -h / | tail -1 >> $OUT
done
rm -rf $S/t167
echo done >> $OUT
