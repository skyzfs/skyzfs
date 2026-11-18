//! The intent map's own rules, without a store: its format, its trust,
//! and what it holds durable at each step.

use super::*;

const ID: [u8; 16] = [7; 16];
const T1: [u8; 16] = [1; 16];
const T2: [u8; 16] = [2; 16];

/// A volume of 1000 regions of 64 blocks, the last one short.
fn shape() -> Shape {
    Shape {
        identity: ID,
        region_blocks: 64,
        blocks: 999 * 64 + 10,
    }
}

/// A file as `map`'s pending writes leave it, over `file`.
fn write_out(map: &mut IntentMap, file: &mut Vec<u8>) {
    let len = map.shape().file_len() as usize;
    if file.len() < len {
        file.resize(len, 0);
    }
    for (at, image) in map.write_begin() {
        file[at as usize..at as usize + PAGE].copy_from_slice(&image);
    }
}

/// `map` written out whole and synced.
fn settled(map: &mut IntentMap) -> Vec<u8> {
    let mut file = Vec::new();
    write_out(map, &mut file);
    map.sync_begin();
    map.synced(true);
    file
}

const BASE: Base = Base {
    state: [5; 16],
    number: 3,
    group: 30,
};

#[test]
fn a_shape_counts_its_regions_and_pages() {
    let shape = shape();
    assert_eq!(shape.regions(), 1000);
    assert_eq!(shape.bit_pages(), 1);
    assert_eq!(shape.file_len(), 2 * PAGE as u64);
    assert_eq!(shape.blocks_of(999), (999 * 64, 10));
    assert_eq!(shape.regions_of(0, 1), 0..1);
    assert_eq!(shape.regions_of(63, 2), 0..2);
    assert_eq!(shape.regions_of(64, 64), 1..2);
    assert_eq!(shape.regions_of(5, 0), 0..0);
    // A range past the volume's end touches what lies within it.
    assert_eq!(shape.regions_of(999 * 64, 1000), 999..1000);
    let large = Shape {
        identity: ID,
        region_blocks: 64,
        blocks: 64 * (PER_PAGE + 1),
    };
    assert_eq!(large.bit_pages(), 2);
}

/// The count of the regions named is the length of their list, on a
/// map of two bit pages whose last word in each page is half used, with
/// every region named, none, and some.
#[test]
fn the_regions_named_are_counted_as_listed() {
    let shape = Shape {
        identity: ID,
        region_blocks: 64,
        blocks: 64 * (PER_PAGE + 100) - 3,
    };
    let mut map = IntentMap::new(shape, T1, Mutants::default());
    assert_eq!(map.named_count(), shape.regions());
    assert_eq!(map.named_count(), map.named().len() as u64);
    // Nothing changed anywhere: every bit is cleared.
    map.clear(|_, _| false);
    assert_eq!(map.named_count(), 0);
    let mut seed = 0x5eed_u64;
    for _ in 0..500 {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let region = (seed >> 33) % shape.regions();
        map.set(region * 64, 1);
    }
    for region in [PER_PAGE - 1, PER_PAGE, shape.regions() - 1] {
        map.set(region * 64, 1);
    }
    assert_eq!(map.named_count(), map.named().len() as u64);
}

/// A new map names every region, and what it writes reads back as it
/// was: the bits, the token and the base.
#[test]
fn a_map_reads_back_as_written() {
    let mut map = IntentMap::new(shape(), T1, Mutants::default());
    assert_eq!(map.named().len(), 1000, "a new map names every region");
    let mut file = settled(&mut map);
    map.commit(BASE);
    write_out(&mut map, &mut file);
    let cleared = map.clear(|first, _| first == 5 * 64);
    assert_eq!(cleared, 999);
    write_out(&mut map, &mut file);
    let read = IntentMap::read(&file, shape(), Mutants::default()).expect("whole");
    assert_eq!(read.named(), [5]);
    assert_eq!(read.base(), Some(BASE));
    assert_eq!(read.trust(Some(T1)), Ok(()));
}

/// A page torn by a power cut, or one of another place, is seen, and
/// so are another format, volume or shape.
#[test]
fn a_map_that_is_not_whole_or_not_this_volumes_is_not_taken_up() {
    let mut map = IntentMap::new(shape(), T1, Mutants::default());
    let file = settled(&mut map);
    let read = |bytes: &[u8], shape: Shape| {
        IntentMap::read(bytes, shape, Mutants::default()).map(|m| m.named().len())
    };
    assert_eq!(read(&file, shape()), Ok(1000));
    for (at, why) in [(20, Distrust::Torn(0)), (PAGE + 100, Distrust::Torn(1))] {
        let mut torn = file.clone();
        torn[at] ^= 1;
        assert_eq!(read(&torn, shape()), Err(why));
    }
    let mut moved = file.clone();
    moved[PAGE..PAGE + 8].copy_from_slice(&2u64.to_le_bytes());
    seal(&mut moved[PAGE..2 * PAGE]);
    assert_eq!(read(&moved, shape()), Err(Distrust::Torn(1)));
    let mut magic = file.clone();
    magic[0] ^= 1;
    assert_eq!(read(&magic, shape()), Err(Distrust::Format));
    let mut version = file.clone();
    version[8] = 9;
    seal(&mut version[..PAGE]);
    assert_eq!(read(&version, shape()), Err(Distrust::Version(9)));
    // Version 1's base named no cut state (M2.4): it is read no more,
    // and refused for its version before its checksum is looked at.
    version[8] = 1;
    seal(&mut version[..PAGE]);
    assert_eq!(read(&version, shape()), Err(Distrust::Version(1)));
    version[20] ^= 1;
    assert_eq!(read(&version, shape()), Err(Distrust::Version(1)));
    let other = |change: &dyn Fn(&mut Shape)| {
        let mut shape = shape();
        change(&mut shape);
        read(&file, shape)
    };
    assert_eq!(other(&|s| s.identity = [8; 16]), Err(Distrust::Identity));
    assert_eq!(other(&|s| s.region_blocks = 128), Err(Distrust::Shape));
    assert_eq!(other(&|s| s.blocks += 64), Err(Distrust::Shape));
    assert_eq!(read(&file[..PAGE], shape()), Err(Distrust::Shape));
    assert_eq!(read(&file[..100], shape()), Err(Distrust::Format));
    // The defect takes a torn bit page up as it reads.
    let mut torn = file.clone();
    torn[PAGE + 100] = 0;
    assert_eq!(
        IntentMap::read(&torn, shape(), Mutants(Mutants::INTENT_TORN_TRUSTED))
            .map(|m| m.named().len()),
        Ok(1000 - 8)
    );
}

/// A map is taken up only by the start after the one that kept it, and
/// only with a base.
#[test]
fn a_map_is_trusted_only_under_its_token_and_with_a_base() {
    let mut map = IntentMap::new(shape(), T1, Mutants::default());
    assert_eq!(map.trust(Some(T1)), Err(Distrust::NoBase));
    map.commit(BASE);
    assert_eq!(map.trust(Some(T1)), Ok(()));
    assert_eq!(map.trust(Some(T2)), Err(Distrust::Token));
    assert_eq!(map.trust(None), Err(Distrust::Token));
    map.kept_by(T2);
    assert_eq!(map.trust(Some(T2)), Ok(()));
    let mut file = Vec::new();
    write_out(&mut map, &mut file);
    let read = IntentMap::read(&file, shape(), Mutants::default()).expect("whole");
    assert_eq!(read.trust(Some(T2)), Ok(()));
    let unchecked =
        IntentMap::read(&file, shape(), Mutants(Mutants::INTENT_TOKEN_UNCHECKED)).expect("whole");
    assert_eq!(unchecked.trust(Some(T1)), Ok(()), "the defect");
}

/// A bit is durable only once a sync that began after its write has
/// ended well: a write into its region waits until then.
#[test]
fn a_bit_covers_a_write_only_once_its_write_is_synced() {
    let mut map = IntentMap::new(shape(), T1, Mutants::default());
    assert!(
        !map.covers(0, 1),
        "nothing is durable before the first sync"
    );
    let mut file = settled(&mut map);
    assert!(map.covers(0, 999 * 64 + 10));
    map.commit(BASE);
    write_out(&mut map, &mut file);
    map.clear(|_, _| false);
    assert!(!map.covers(5 * 64, 1), "a clear covers no write");
    // Set again before its page is written, the bit was never off the
    // disk: it covers at once, and the page is written with it.
    map.set(4 * 64, 1);
    assert!(map.covers(4 * 64, 64));
    let image = map.write_begin();
    assert_eq!(image.len(), 1);
    map.sync_begin();
    map.synced(true);
    assert!(map.covers(4 * 64, 64));
    assert!(!map.covers(5 * 64, 1), "written cleared");
    map.set(5 * 64 + 3, 70);
    assert!(map.has_dirty());
    assert!(!map.covers(5 * 64, 1), "set, not written");
    write_out(&mut map, &mut file);
    assert!(!map.covers(5 * 64, 1), "written, not synced");
    map.sync_begin();
    // A failed sync makes nothing durable.
    map.synced(false);
    assert!(!map.covers(5 * 64, 1));
    map.set(5 * 64, 1);
    write_out(&mut map, &mut file);
    map.sync_begin();
    map.synced(true);
    assert!(map.covers(5 * 64, 64));
    assert!(map.covers(6 * 64, 64));
    assert!(!map.covers(7 * 64, 1));
    // Region 4's page was written in place of the file's here: the
    // file holds what was written before, and this write's page.
    let mut map_file = file.clone();
    map.set(0, 1);
    write_out(&mut map, &mut map_file);
    let read = IntentMap::read(&map_file, shape(), Mutants::default()).expect("whole");
    assert_eq!(read.named(), [0, 4, 5, 6]);
    // The defect lets every write begin.
    let defect = IntentMap::new(shape(), T1, Mutants(Mutants::INTENT_WRITE_UNCOVERED));
    assert!(defect.covers(0, 1));
}

/// A page written again with a bit cleared while a sync of an earlier
/// write runs: the disk may hold either write, so the bit is not
/// durable when that sync ends; a bit both writes set is.
#[test]
fn a_bit_cleared_by_a_later_write_is_not_made_durable_by_an_earlier_sync() {
    let mut map = IntentMap::new(shape(), T1, Mutants::default());
    let mut file = settled(&mut map);
    map.commit(BASE);
    write_out(&mut map, &mut file);
    map.clear(|first, _| first < 2 * 64);
    write_out(&mut map, &mut file);
    map.sync_begin();
    map.synced(true);
    assert_eq!(map.named(), [0, 1]);
    map.set(2 * 64, 1);
    write_out(&mut map, &mut file);
    map.sync_begin();
    // While that sync runs, region 2 is let go and the page written.
    map.clear(|first, _| first < 64);
    write_out(&mut map, &mut file);
    map.synced(true);
    assert!(map.covers(0, 64), "set in both writes");
    assert!(!map.covers(64, 1), "cleared in the later write");
    assert!(!map.covers(2 * 64, 1), "cleared in the later write");
    // Set again, region 2 is not durable: its page is to be written and
    // synced first. (Asking `covers` alone would not show it: the bit is
    // clear in memory whatever the sync made durable.)
    map.set(2 * 64, 1);
    assert!(
        !map.covers(2 * 64, 1),
        "the sync covered only the first write"
    );
    assert!(map.has_dirty(), "the page is to be written");
}

/// The review's D1 of #105: a page written with region 2's bit set, a
/// sync begun, then while it runs the page written with the bit
/// cleared (a clear after a commit) and once more with it set (a write
/// into region 2). The disk may keep the clearing write and lose the
/// last, and the sync covered only the first: region 2 is not durable
/// when the sync ends, and a write there must wait for the next one.
/// The defect (`INTENT_SYNC_UNNARROWED`) calls it durable, and so did
/// the first draft, which looked at the last write alone.
#[test]
fn a_bit_cleared_and_set_again_while_one_sync_runs_is_not_made_durable_by_it() {
    for mutants in [0, Mutants::INTENT_SYNC_UNNARROWED] {
        let durable = a_bit_cleared_and_set_again_while_one_sync_runs(Mutants(mutants));
        assert_eq!(durable, mutants != 0, "mutants {mutants:#x}");
    }
}

/// Whether region 2 is called durable in D1's case, under `mutants`.
fn a_bit_cleared_and_set_again_while_one_sync_runs(mutants: Mutants) -> bool {
    let mut map = IntentMap::new(shape(), T1, mutants);
    let mut file = settled(&mut map);
    map.commit(BASE);
    map.clear(|first, _| first < 3 * 64);
    // The page, with region 2 named, written and not yet synced; then
    // a sync begins, which covers that write.
    write_out(&mut map, &mut file);
    map.sync_begin();
    // While that sync runs: region 2 let go, its page written ...
    map.clear(|first, _| first < 2 * 64);
    write_out(&mut map, &mut file);
    // ... then a write into region 2 sets it again, and the page is
    // written again.
    map.set(2 * 64, 1);
    write_out(&mut map, &mut file);
    map.synced(true);
    assert!(map.covers(0, 2 * 64), "set in every write");
    let durable = map.covers(2 * 64, 1);
    // The next sync covers the last write, and makes it durable.
    map.set(2 * 64, 1);
    write_out(&mut map, &mut file);
    map.sync_begin();
    map.synced(true);
    assert!(map.covers(2 * 64, 1));
    durable
}

/// A sync covers only the writes issued before it began: a bit set in a
/// write while it runs is not durable when it ends.
#[test]
fn a_sync_covers_only_the_writes_issued_before_it_began() {
    let mut map = IntentMap::new(shape(), T1, Mutants::default());
    let mut file = settled(&mut map);
    map.commit(BASE);
    map.clear(|_, _| false);
    write_out(&mut map, &mut file);
    map.sync_begin();
    map.synced(true);
    assert_eq!(map.named(), Vec::<u64>::new());
    map.set(0, 1);
    write_out(&mut map, &mut file);
    map.sync_begin();
    map.set(64, 1);
    write_out(&mut map, &mut file);
    map.synced(true);
    assert!(map.covers(0, 1), "written before the sync began");
    assert!(!map.covers(64, 1), "written while it ran");
    map.sync_begin();
    map.synced(true);
    assert!(map.covers(64, 1), "covered by the next");
}

/// A start that keeps a map it took up writes its header with its own
/// token, so that the start after it finds that token.
#[test]
fn a_map_kept_by_a_start_has_its_header_written_with_the_new_token() {
    let mut map = IntentMap::new(shape(), T1, Mutants::default());
    let mut file = settled(&mut map);
    map.commit(BASE);
    write_out(&mut map, &mut file);
    let mut taken = IntentMap::read(&file, shape(), Mutants::default()).expect("whole");
    assert_eq!(taken.trust(Some(T1)), Ok(()));
    assert!(!taken.has_dirty());
    taken.kept_by(T2);
    write_out(&mut taken, &mut file);
    let again = IntentMap::read(&file, shape(), Mutants::default()).expect("whole");
    assert_eq!(again.trust(Some(T2)), Ok(()));
    assert_eq!(again.trust(Some(T1)), Err(Distrust::Token));
}

/// A token is compared whole: one that differs in its last byte alone
/// is another start's.
#[test]
fn a_token_that_differs_in_its_last_byte_is_another_starts() {
    let mut map = IntentMap::new(shape(), T1, Mutants::default());
    settled(&mut map);
    map.commit(BASE);
    let mut other = T1;
    other[15] ^= 1;
    assert_eq!(map.trust(Some(other)), Err(Distrust::Token));
    assert_eq!(map.trust(Some(T1)), Ok(()));
}

/// A clear names the regions in which something changed after the
/// base's instant; the defect clears them too.
#[test]
fn a_bit_is_kept_where_its_region_changed_after_the_base() {
    for (mutants, kept) in [(0, vec![3, 4]), (Mutants::INTENT_CLEARED_CHANGED, vec![])] {
        let mut map = IntentMap::new(shape(), T1, Mutants(mutants));
        settled(&mut map);
        map.commit(BASE);
        let changed = |first: u64, count: u64| first < 5 * 64 && first + count > 3 * 64;
        map.clear(changed);
        assert_eq!(map.named(), kept, "mutants {mutants:#x}");
    }
}
