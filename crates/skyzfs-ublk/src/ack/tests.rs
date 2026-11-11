//! The rules of the record of what was acknowledged (design §4.2),
//! each with the defect that shows the test would notice.

use super::*;

type Outcome = Result<(), String>;

const STORE: [u8; 16] = *b"a store identity";
const OTHER: [u8; 16] = *b"another identity";
const BOOT: [u8; 16] = *b"this boot's id..";
const LATER: [u8; 16] = *b"the next boot id";

fn holds(scenario: fn(u32) -> Outcome) {
    if let Err(why) = scenario(0) {
        panic!("{why}");
    }
}

/// The scenario holds for the record as it is and fails with the
/// defect.
fn caught(scenario: fn(u32) -> Outcome, mutant: u32) {
    holds(scenario);
    assert!(
        scenario(mutant).is_err(),
        "the defect {mutant:#x} was not noticed"
    );
}

fn text<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

fn open(path: &Path, identity: [u8; 16], boot: [u8; 16], mutants: u32) -> Result<AckFile, String> {
    AckFile::open_with(path, identity, boot, Mutants(mutants)).map_err(text)
}

fn stored(path: &Path) -> Result<Option<u64>, String> {
    Ok(read(path)
        .map_err(text)?
        .and_then(|s| s.position_for(&STORE, &BOOT)))
}

fn expect<T: PartialEq + std::fmt::Debug>(got: T, want: T, what: &str) -> Outcome {
    if got == want {
        Ok(())
    } else {
        Err(format!("{what}: expected {want:?}, got {got:?}"))
    }
}

/// The stored position never moves back: a lower or equal position is
/// not written, through one handle and through two queues that share
/// the record and store out of order. Mutant: any position is written.
#[test]
fn the_position_never_moves_back() {
    fn scenario(mutants: u32) -> Outcome {
        let dir = tempfile::tempdir().map_err(text)?;
        let path = dir.path().join("volume.ack");
        let mut file = open(&path, STORE, BOOT, mutants)?;
        expect(stored(&path)?, None, "a new record")?;
        for (position, want) in [
            (4096, 4096),
            (8192, 8192),
            (4096, 8192),
            (8192, 8192),
            (0, 8192),
        ] {
            file.advance(position).map_err(text)?;
            expect(stored(&path)?, Some(want), "after a store")?;
            expect(file.position(), Some(want), "held")?;
        }
        // Two queues: the one with the lower position stores last.
        let (mut first, mut second) = {
            let shared = SharedAck::new(file);
            (shared.clone(), shared)
        };
        second.advance(20_000).map_err(text)?;
        first.advance(12_288).map_err(text)?;
        expect(stored(&path)?, Some(20_000), "two queues out of order")
    }
    caught(scenario, Mutants::MOVES_BACK);
}

/// A take-over keeps what the instance before it acknowledged: opening
/// the record again for the same store in the same boot leaves the
/// position, on the file and in the handle, before the new instance
/// has acknowledged anything, and a position below it is still not
/// written. Mutant: opening empties the record.
#[test]
fn a_take_over_does_not_forget_what_was_acknowledged() {
    fn scenario(mutants: u32) -> Outcome {
        let dir = tempfile::tempdir().map_err(text)?;
        let path = dir.path().join("volume.ack");
        let mut file = open(&path, STORE, BOOT, mutants)?;
        file.advance(70_000).map_err(text)?;
        drop(file);
        let mut again = open(&path, STORE, BOOT, mutants)?;
        expect(stored(&path)?, Some(70_000), "after the take-over")?;
        expect(again.position(), Some(70_000), "held by the new instance")?;
        again.advance(100).map_err(text)?;
        expect(
            stored(&path)?,
            Some(70_000),
            "a lower position after the take-over",
        )
    }
    caught(scenario, Mutants::OPEN_RESETS);
}

/// The record belongs to one store in one boot: read for another store
/// or in another boot it holds no position, and opening it for another
/// store, or in another boot, starts an empty record. `reset` empties
/// it for the same store and boot.
#[test]
fn a_record_of_another_store_or_boot_counts_as_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("volume.ack");
    assert_eq!(read(&path).unwrap(), None);
    let mut file = AckFile::open(&path, STORE, BOOT).unwrap();
    file.advance(5000).unwrap();
    let record = read(&path).unwrap().unwrap();
    assert_eq!(record.position_for(&STORE, &BOOT), Some(5000));
    assert_eq!(record.position_for(&OTHER, &BOOT), None);
    assert_eq!(record.position_for(&STORE, &LATER), None);
    for (identity, boot) in [(OTHER, BOOT), (STORE, LATER)] {
        let opened = AckFile::open(&path, identity, boot).unwrap();
        assert_eq!(opened.position(), None);
        let record = read(&path).unwrap().unwrap();
        assert_eq!(
            (record.identity, record.boot, record.position),
            (identity, boot, None)
        );
        // Put the first record back for the next round.
        drop(opened);
        let mut file = AckFile::open(&path, STORE, BOOT).unwrap();
        file.advance(5000).unwrap();
    }
    let emptied = AckFile::reset(&path, STORE, BOOT).unwrap();
    assert_eq!(emptied.position(), None);
    assert_eq!(read(&path).unwrap().unwrap().position, None);
}

/// A kill leaves the old value or the new one: the file has its whole
/// length from the moment it has its name, a store changes exactly the
/// eight bytes of the position, and a handle that was open when another
/// record took the place does not write into the new one. Mutant: a new
/// record is written in place over the old one, so the name holds, for
/// a moment, a file that is empty.
#[test]
fn the_record_is_whole_whenever_it_has_its_name() {
    fn scenario(mutants: u32) -> Outcome {
        let dir = tempfile::tempdir().map_err(text)?;
        let path = dir.path().join("volume.ack");
        let mut file = open(&path, STORE, BOOT, mutants)?;
        file.advance(1 << 40).map_err(text)?;
        let before = fs::read(&path).map_err(text)?;
        expect(before.len(), LEN, "the length")?;
        // Another store takes the place while the old handle is open.
        let other = open(&path, OTHER, BOOT, mutants)?;
        file.advance(1 << 41).map_err(text)?;
        let record = read(&path).map_err(text)?.ok_or("no record")?;
        expect(
            (record.identity, record.position),
            (OTHER, None),
            "the new record, after the old handle stored",
        )?;
        drop(other);
        // A store changes the position's bytes and nothing else.
        let mut file = open(&path, OTHER, BOOT, mutants)?;
        let empty = fs::read(&path).map_err(text)?;
        file.advance(0x0102_0304_0506_0708).map_err(text)?;
        let after = fs::read(&path).map_err(text)?;
        expect(&after[..40], &empty[..40], "the bytes before the position")?;
        expect(&after[48..], &empty[48..], "the bytes behind the position")?;
        expect(
            &after[40..48],
            &[8, 7, 6, 5, 4, 3, 2, 1][..],
            "the position",
        )?;
        // Nothing is left beside it.
        let names: Vec<_> = fs::read_dir(dir.path())
            .map_err(text)?
            .filter_map(|e| e.ok().map(|e| e.file_name()))
            .collect();
        expect(names.len(), 1, "files in the directory")
    }
    caught(scenario, Mutants::CREATE_IN_PLACE);
}

/// A file that is there and is not a whole record is an error, for a
/// read and for an open: it cannot say that nothing was acknowledged.
/// And the position that stands for "none" cannot be stored.
#[test]
fn a_file_that_is_not_a_record_is_an_error_and_not_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("volume.ack");
    let mut file = AckFile::open(&path, STORE, BOOT).unwrap();
    file.advance(9).unwrap();
    assert!(file.advance(u64::MAX).is_err());
    let good = fs::read(&path).unwrap();
    let mut wrong_magic = good.clone();
    wrong_magic[0] ^= 1;
    let mut wrong_tail = good.clone();
    wrong_tail[63] = 1;
    for bad in [
        &good[..63],
        &[][..],
        &wrong_magic[..],
        &wrong_tail[..],
        &[good.clone(), vec![0]].concat()[..],
    ] {
        fs::write(&path, bad).unwrap();
        assert_eq!(read(&path).unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert!(AckFile::open(&path, STORE, BOOT).is_err());
    }
    fs::write(&path, &good).unwrap();
    assert_eq!(read(&path).unwrap().unwrap().position, Some(9));
}
