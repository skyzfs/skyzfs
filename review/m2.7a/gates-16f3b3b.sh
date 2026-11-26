#!/bin/bash
# Gates for PR 167 at 16f3b3b: the delta touches skyzfs-agent only.
S=$SCRATCH
cd $S/wt/pr167 || exit 9
git checkout -q --detach 16f3b3b8fd94dd33dc6adbd1f7e18f9b19f0ff02 || exit 9
export CARGO_TARGET_DIR=$S/t167 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
OUT=$S/r167/gates2.out
: > $OUT
echo "head $(git rev-parse HEAD)" >> $OUT
rustc --version >> $OUT
cargo fmt --all -- --check > $S/r167/g2-fmt.log 2>&1; echo "fmt rc=$?" >> $OUT
for c in skyzfs-agent skyzfs-agent-cli; do
  echo "tree $c: $(cargo tree -p $c -e normal,build --prefix none 2>/dev/null | grep -c -E '^(skyzfsd|skyzfs-ublk|skyzfs-ublk-sys|skyzfs-local|skyzfs-blk) ') of the featured crates" >> $OUT
done
rm -rf $S/t167
for fs in "" "skyzfs-agent-cli/test-hooks"; do
  tag=${fs:-plain}; tag=${tag//\//_}; tag=${tag//,/+}
  F=(); [ -n "$fs" ] && F=(--features "$fs")
  cargo clippy --workspace --all-targets "${F[@]}" -- -D warnings > $S/r167/g2-clippy-$tag.log 2>&1; echo "clippy $tag rc=$?" >> $OUT
  cargo test -p skyzfs-agent -p skyzfs-agent-cli "${F[@]}" > $S/r167/g2-test-$tag.log 2>&1; rc=$?
  p=$(grep -E '^test result:' $S/r167/g2-test-$tag.log | awk '{p+=$4; f+=$6; i+=$8} END {print p" passed "f" failed "i" ignored"}')
  echo "test $tag rc=$rc $p" >> $OUT
  RUSTDOCFLAGS='-D warnings' cargo doc -p skyzfs-agent -p skyzfs-agent-cli --no-deps "${F[@]}" > $S/r167/g2-doc-$tag.log 2>&1; echo "doc $tag rc=$?" >> $OUT
  df -h / | tail -1 >> $OUT
done
# The host tests twenty times (the flaky one of eedd83e).
fails=0
for i in $(seq 1 20); do
  cargo test -p skyzfs-agent --lib host:: > $S/r167/g2-host.log 2>&1 || fails=$((fails+1))
done
echo "host tests x20: $fails failed runs" >> $OUT
echo done >> $OUT
