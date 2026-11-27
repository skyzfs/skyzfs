#!/bin/bash
# Gates for PR 175 at its head, in the scratch worktree; the target is cleaned between sets.
S=${SCRATCH:?set SCRATCH to a scratch directory with a worktree of the head at $SCRATCH/r175/head}
cd $S/r175/head || exit 9
export CARGO_TARGET_DIR=$S/t175 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
OUT=$S/gates175.out
: > $OUT
echo "head $(git rev-parse HEAD)" >> $OUT
rustc --version >> $OUT
cargo fmt --all -- --check > $S/g175-fmt.log 2>&1; echo "fmt rc=$?" >> $OUT
for fs in "" "skyzfsd/test-hooks,skyzfs-local/test-hooks" "skyzfs-ublk-sys/control-test-program" "skyzfs-agent-cli/test-hooks"; do
  rm -rf $S/t175
  tag=${fs:-plain}; tag=${tag//\//_}; tag=${tag//,/+}
  F=(); [ -n "$fs" ] && F=(--features "$fs")
  cargo clippy --workspace --all-targets "${F[@]}" -- -D warnings > $S/g175-clippy-$tag.log 2>&1; echo "clippy $tag rc=$?" >> $OUT
  cargo test --workspace "${F[@]}" > $S/g175-test-$tag.log 2>&1; rc=$?
  p=$(grep -E '^test result:' $S/g175-test-$tag.log | awk '{p+=$4; f+=$6; i+=$8} END {print p" passed "f" failed "i" ignored"}')
  echo "test $tag rc=$rc $p" >> $OUT
  RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps "${F[@]}" > $S/g175-doc-$tag.log 2>&1; echo "doc $tag rc=$?" >> $OUT
  df -h / | tail -1 >> $OUT
done
echo done >> $OUT
