#!/bin/bash
# Gates for PR 167 at its head, in the scratch worktree; the target is cleaned between sets.
S=$SCRATCH
cd $S/wt/pr167 || exit 9
export CARGO_TARGET_DIR=$S/t167 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
OUT=$S/r167/gates.out
: > $OUT
echo "head $(git rev-parse HEAD)" >> $OUT
rustc --version >> $OUT
cargo fmt --all -- --check > $S/r167/g-fmt.log 2>&1; echo "fmt rc=$?" >> $OUT
for fs in "" "skyzfsd/test-hooks,skyzfs-local/test-hooks" "skyzfs-ublk-sys/control-test-program" "skyzfs-agent-cli/test-hooks"; do
  rm -rf $S/t167
  tag=${fs:-plain}; tag=${tag//\//_}; tag=${tag//,/+}
  F=(); [ -n "$fs" ] && F=(--features "$fs")
  cargo clippy --workspace --all-targets "${F[@]}" -- -D warnings > $S/r167/g-clippy-$tag.log 2>&1; echo "clippy $tag rc=$?" >> $OUT
  cargo test --workspace "${F[@]}" > $S/r167/g-test-$tag.log 2>&1; rc=$?
  p=$(grep -E '^test result:' $S/r167/g-test-$tag.log | awk '{p+=$4; f+=$6; i+=$8} END {print p" passed "f" failed "i" ignored"}')
  echo "test $tag rc=$rc $p" >> $OUT
  RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps "${F[@]}" > $S/r167/g-doc-$tag.log 2>&1; echo "doc $tag rc=$?" >> $OUT
  df -h / | tail -1 >> $OUT
done
rm -rf $S/t167
echo done >> $OUT
