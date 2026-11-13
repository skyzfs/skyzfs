#!/bin/bash
# Plants and the probe for the review of M1.3v (skyzfs/skyzfs#47) at cbeac1d.
# Run from a clean checkout of that commit: plants.sh <dir of plant.py>
set -u
P="python3 $1/plant.py"
W=crates/skyzfs-ublk-sys/src/watch.rs
T=crates/skyzfs-local/src/inplace/tests.rs
echo "=== q1: mapped_in rounds to 4096 whatever page it is given (the test fails)"
$P $W '    let from = start / page * page;
    let bytes = (start + len).div_ceil(page) * page - from;' '    let from = start / 4096 * 4096;
    let bytes = (start + len).div_ceil(4096) * 4096 - from;
    let _ = page;' && cargo test -p skyzfs-ublk-sys --lib 2>&1 | grep -E '^test result|FAILED'; git checkout -q -- crates
echo "=== q2: mapped() gives 4096, the defect as it was (all pass on amd64)"
$P $W '    mapped_in(start, len, page_size())
}' '    mapped_in(start, len, 4096)
}' && cargo test -p skyzfs-ublk-sys --lib 2>&1 | grep -E '^test result|FAILED'; git checkout -q -- crates
echo "=== q3: the simulated disk does not widen a write to the page (the garbage test fails)"
$P $T '    disk.set_write_unit(page);
    disk.set_page_size(page);
    disk.set_block_size(Some(page));
    disk.set_sync_delay(sync_delay);' '    disk.set_write_unit(4096);
    disk.set_page_size(page);
    disk.set_block_size(Some(4096));
    disk.set_sync_delay(sync_delay);' && cargo test -p skyzfs-local --lib large_pages 2>&1 | grep -E '^test result|FAILED'; git checkout -q -- crates
echo "=== probe P: how often garbage reaches a block nothing wrote since its last flush"
cat >> $T <<'EOT'

#[test]
fn probe_p_garbage_by_page() {
    for page in [4096u64, 16 << 10, 64 << 10] {
        let ending = Ending::Cut { garbage: true };
        let lost = (1..=200)
            .filter(|seed| volatile_cache_holds_at(*seed, ending, 0, page).is_err())
            .count();
        let first = (1..=200).find_map(|seed| volatile_cache_holds_at(seed, ending, 0, page).err());
        eprintln!("probe P: page {page}: {lost} of 200 seeds; first: {first:?}");
    }
}
EOT
cargo test -p skyzfs-local --lib probe_p -- --nocapture 2>&1 | grep -oE 'probe P:.*|test result.*'; git checkout -q -- crates
