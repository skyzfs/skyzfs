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
    assert_eq!(read(&version, shape()), Err(Distrust::Format));
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
