#!/bin/bash
# Gates for PR 173 at e3fe645, in a scratch worktree (wt173n). The Rust
# delta from 3b93a83 is the agent's program.rs and its tests only, so the
# workspace-wide gates of 3b93a83 (gates-3b93a83.out) carry over; these
# rerun fmt for the workspace, and clippy, the tests and the docs of the
# agent's crates, plain and with skyzfs-agent-cli/test-hooks, then the
# whole workspace's tests plain. SCRATCH names the scratch directory.
S=$SCRATCH
cd $S/wt173n || exit 9
export CARGO_TARGET_DIR=$S/t173g CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
R=$S/m27cn/out-e
mkdir -p $R
OUT=$R/gates-e.out
: > $OUT
echo "head $(git rev-parse HEAD)" >> $OUT
rustc --version >> $OUT
cargo fmt --all -- --check > $R/fmt.log 2>&1; echo "fmt rc=$?" >> $OUT
A=(-p skyzfs-agent -p skyzfs-agent-cli)
for fs in "" "skyzfs-agent-cli/test-hooks"; do
  tag=${fs:-plain}; tag=${tag//\//_}
  F=(); [ -n "$fs" ] && F=(--features "$fs")
  cargo clippy "${A[@]}" --all-targets "${F[@]}" -- -D warnings > $R/clippy-$tag.log 2>&1; echo "clippy agent crates $tag rc=$?" >> $OUT
  cargo test "${A[@]}" -p skyzfs-control "${F[@]}" > $R/test-$tag.log 2>&1; rc=$?
  p=$(grep -E '^test result:' $R/test-$tag.log | awk '{p+=$4; f+=$6; i+=$8} END {print p" passed "f" failed "i" ignored"}')
  echo "test agent, agent-cli, control $tag rc=$rc $p" >> $OUT
  RUSTDOCFLAGS='-D warnings' cargo doc "${A[@]}" --no-deps "${F[@]}" > $R/doc-$tag.log 2>&1; echo "doc agent crates $tag rc=$?" >> $OUT
done
fails=0
for i in $(seq 1 10); do
  cargo test -p skyzfs-agent --lib > $R/agent-x10.log 2>&1 || { fails=$((fails+1)); cp $R/agent-x10.log $R/agent-fail-$i.log; }
done
echo "agent lib tests x10: $fails failed runs (last: $(grep -E '^test result:' $R/agent-x10.log | awk '{p+=$4; f+=$6} END {print p" passed "f" failed"}'))" >> $OUT
cargo test --workspace > $R/test-workspace.log 2>&1; rc=$?
p=$(grep -E '^test result:' $R/test-workspace.log | awk '{p+=$4; f+=$6; i+=$8} END {print p" passed "f" failed "i" ignored"}')
echo "test workspace plain rc=$rc $p" >> $OUT
echo done >> $OUT
