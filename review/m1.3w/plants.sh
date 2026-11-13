#!/bin/bash
# plants.sh PLANT_PY NAME...: run in a worktree at the pull request's head.
# Each plant puts a defect in the store's time-keeping, runs the two timing
# tests and restores the tree. The line that fails is printed.
set -u
plant=$1; shift
F=crates/skyzfs-local/src/store.rs
T1=a_flush_that_arrives_during_a_step_is_counted_with_what_was_left_of_the_step
T2=a_read_waits_for_the_reader_and_for_nothing_of_the_worker
for name in "$@"; do
  git checkout -q -- crates
  case $name in
    p1) # a look counts an idle thread's wait from the store's start
      python3 "$plant" $F '*idle = idle.saturating_add(ns(since, now));' '*idle = idle.saturating_add(ns(self.2, now));' ;;
    p2) # a look counts 100 microseconds too much of an idle thread's wait
      python3 "$plant" $F '*idle = idle.saturating_add(ns(since, now));' '*idle = idle.saturating_add(ns(since, now) + 100_000);' ;;
    p3) # the worker's account holds the home image's sync twice
      python3 "$plant" $F 'timing.worker[activity] = timing.worker[activity].saturating_add(ns);' 'timing.worker[activity] = timing.worker[activity].saturating_add(if activity == Activity::STEP_HOME_SYNC { ns * 2 } else { ns });' ;;
    p4) # the step's part for the home image's sync is counted double (where a served store hands the sync over)
      python3 "$plant" $F 'self.timing.step_home_sync.add(began - written_at);' 'self.timing.step_home_sync.add((began - written_at) * 2);' ;;
    *) echo "unknown plant $name"; exit 2 ;;
  esac || { echo "$name: the plant did not apply"; git checkout -q -- crates; continue; }
  echo "== $name"
  cargo test -p skyzfs-local --lib -- $T1 $T2 2>&1 | grep -E "^test |panicked at|^error" | sed -E 's/thread .* panicked at/panicked at/' | cut -c1-140
  git checkout -q -- crates
done
