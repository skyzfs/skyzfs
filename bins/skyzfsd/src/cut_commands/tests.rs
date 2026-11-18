//! The commands about held cuts: the protocol over a stand-in store,
//! and the whole of it over the in-place store, on a simulated disk,
//! holding two cuts.

use std::collections::BTreeMap;
use std::time::Duration;

use skyzfs_blk::{Base, BlockTarget, Completion, Op, Request, RequestId, BLOCK_USIZE};
use skyzfs_local::inplace::{
    self, CutRegion, HoldCuts, InPlace, MemRegion, OpenOptions, Opened, VolumeConfig,
};
use skyzfs_local::sim::SimDisk;
use skyzfs_uberblock::{seal, ByteOrder, Labels, MAGIC};

use super::*;

/// A store that holds the cuts it is given. Block `b` of cut `n` reads
/// as the byte `n * 16 + b % 16`. `let_go_after` reads, the cut being
/// read is let go; `grows_after` reads, its map gains a block.
#[derive(Default)]
struct Stand {
    off: bool,
    cuts: Vec<Held>,
    maps: BTreeMap<u64, Vec<u64>>,
    reads: usize,
    let_go_after: Option<usize>,
    grows_after: Option<usize>,
    released: Vec<u64>,
}

fn content(number: u64, block: u64) -> u8 {
    (number * 16 + block % 16) as u8
}

impl Stand {
    fn holding(cuts: &[(u64, CutPhase, &[u64])]) -> Self {
        let mut stand = Stand::default();
        for &(number, phase, map) in cuts {
            stand.cuts.push(Held {
                number,
                group: number + 100,
                phase,
                itself: number % 2 == 1,
                blocks: map.len() as u64,
            });
            stand.maps.insert(number, map.to_vec());
        }
        stand
    }
}

impl Cuts for Stand {
    fn holds_cuts(&self) -> bool {
        !self.off
    }

    fn held(&self) -> Vec<Held> {
        self.cuts.clone()
    }

    fn map(&self, number: u64) -> Vec<u64> {
        self.maps.get(&number).cloned().unwrap_or_default()
    }

    fn read(&mut self, number: u64, block: u64, buf: &mut [u8]) -> io::Result<bool> {
        self.reads += 1;
        if self.let_go_after == Some(self.reads) {
            self.cuts.retain(|cut| cut.number != number);
            self.maps.remove(&number);
        }
        if self.grows_after == Some(self.reads) {
            if let Some(map) = self.maps.get_mut(&number) {
                map.push(1 << 20);
            }
        }
        let Some(map) = self.maps.get(&number) else {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no such held cut"));
        };
        if !map.contains(&block) {
            return Ok(false);
        }
        buf.fill(content(number, block));
        Ok(true)
    }

    fn release(&mut self, number: u64) -> bool {
        let uploading = self
            .cuts
            .iter()
            .any(|cut| cut.number == number && cut.phase == CutPhase::Uploading);
        if uploading {
            self.cuts.retain(|cut| cut.number != number);
            self.released.push(number);
        }
        uploading
    }
}

/// A run directory, its commands, and a file of `blocks` blocks of
/// 0xEE for a write to go into.
struct Rig {
    dir: tempfile::TempDir,
    commands: Commands,
}

impl Rig {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a directory");
        let commands = Commands::new(dir.path().to_owned());
        Self { dir, commands }
    }

    fn image(&self, blocks: usize) -> PathBuf {
        let path = self.dir.path().join("uploaded.img");
        std::fs::write(&path, vec![0xEE; blocks * BLOCK_USIZE]).expect("the image");
        path
    }

    /// Send `line` as the rig sends it: the file, then the signal.
    fn send(&mut self, line: &str, cuts: &mut impl Cuts) -> bool {
        std::fs::write(self.dir.path().join(COMMAND), format!("{line}\n")).expect("the command");
        self.commands.turn(true, cuts)
    }

    /// Turns with no signal until no write is under way: how many.
    fn finish(&mut self, cuts: &mut impl Cuts) -> usize {
        let mut turns = 0;
        while self.commands.turn(false, cuts) {
            turns += 1;
            assert!(turns < 1 << 16, "the write never ends");
        }
        turns + 1
    }

    fn reply(&self) -> String {
        std::fs::read_to_string(self.dir.path().join(REPLY)).unwrap_or_default()
    }

    fn reply_gone(&self) {
        let _ = std::fs::remove_file(self.dir.path().join(REPLY));
    }
}

#[test]
fn a_line_is_a_token_and_a_command() {
    let cases: [(&str, &str, Result<Command, &str>); 13] = [
        ("7 held", "7", Ok(Command::Held)),
        ("  x9   held  ", "x9", Ok(Command::Held)),
        ("3 release 12", "3", Ok(Command::Release { number: 12 })),
        (
            "4 write 2 /tmp/an image",
            "4",
            Ok(Command::Write {
                number: 2,
                path: PathBuf::from("/tmp/an image"),
            }),
        ),
        ("", "-", Err("an empty command")),
        ("5", "5", Err("no command after the token")),
        ("5 hold", "5", Err("not a command: \"hold\"")),
        ("5 held 1", "5", Err("held takes nothing")),
        ("5 release", "5", Err("release takes a cut's number")),
        ("5 release 1 2", "5", Err("release takes a cut's number")),
        ("5 release one", "5", Err("not a cut's number: \"one\"")),
        (
            "5 write 1 relative.img",
            "5",
            Err("not an absolute path: relative.img"),
        ),
        (
            "5 write 1",
            "5",
            Err("write takes a cut's number and a path"),
        ),
    ];
    for (line, token, command) in cases {
        let (got_token, got) = parse(line);
        assert_eq!(got_token, token, "{line:?}");
        assert_eq!(got, command.map_err(str::to_owned), "{line:?}");
    }
}

#[test]
fn held_lists_every_held_cut_oldest_first_then_ok() {
    let mut rig = Rig::new();
    let mut stand = Stand::holding(&[
        (3, CutPhase::Uploading, &[1, 2, 3]),
        (4, CutPhase::Waiting, &[2]),
        (5, CutPhase::Flushing, &[]),
    ]);
    assert!(!rig.send("t1 held", &mut stand));
    assert_eq!(
        rig.reply(),
        "3 103 uploading itself 3\n4 104 waiting rewind 1\n5 105 flushing itself 0\nt1 ok\n"
    );
    assert!(
        !rig.dir.path().join(REPLY_TEMP).exists(),
        "the reply is renamed into place"
    );
    rig.send("t2 held", &mut Stand::default());
    assert_eq!(rig.reply(), "t2 ok\n");
}

#[test]
fn every_command_is_refused_while_held_cuts_are_off() {
    let mut rig = Rig::new();
    let mut stand = Stand {
        off: true,
        ..Stand::holding(&[(1, CutPhase::Uploading, &[0])])
    };
    let image = rig.image(1);
    for line in [
        "a held",
        "b release 1",
        &format!("c write 1 {}", image.display()),
    ] {
        rig.send(line, &mut stand);
        let token = &line[..1];
        assert_eq!(
            rig.reply(),
            format!(
                "{token} refused: held cuts are off: the daemon was started without --hold-cuts\n"
            )
        );
    }
    assert!(stand.released.is_empty());
}

#[test]
fn a_line_that_is_no_command_is_refused_with_its_token() {
    let mut rig = Rig::new();
    let mut stand = Stand::default();
    rig.send("9 upload 1", &mut stand);
    assert_eq!(rig.reply(), "9 refused: not a command: \"upload\"\n");
    rig.send("", &mut stand);
    assert_eq!(rig.reply(), "- refused: an empty command\n");
    // No command file at all.
    std::fs::remove_file(rig.dir.path().join(COMMAND)).expect("remove");
    rig.commands.turn(true, &mut stand);
    assert!(rig.reply().starts_with("- refused: "), "{}", rig.reply());
    assert!(rig.reply().contains(COMMAND), "{}", rig.reply());
}

#[test]
fn release_lets_only_the_cut_being_uploaded_go() {
    let mut rig = Rig::new();
    let mut stand = Stand::holding(&[(3, CutPhase::Uploading, &[1]), (4, CutPhase::Waiting, &[2])]);
    rig.send("r release 4", &mut stand);
    assert_eq!(
        rig.reply(),
        "r refused: cut 4 is not the one being uploaded: the cut being uploaded is 3\n"
    );
    rig.send("s release 3", &mut stand);
    assert_eq!(rig.reply(), "s ok\n");
    assert_eq!(stand.released, [3]);
    rig.send("u release 3", &mut stand);
    assert_eq!(
        rig.reply(),
        "u refused: cut 3 is not the one being uploaded: no cut is being uploaded\n"
    );
}

/// In a daemon that uploads, `release` is refused whatever the cut, and
/// lets nothing go: the uploader lets a cut go once it is committed.
/// `held` and `write` are answered as before.
#[test]
fn release_is_refused_in_a_daemon_that_uploads() {
    let mut rig = Rig::new();
    rig.commands = Commands::new(rig.dir.path().to_owned()).uploading(true);
    let mut stand = Stand::holding(&[(3, CutPhase::Uploading, &[1])]);
    rig.send("r release 3", &mut stand);
    assert_eq!(
        rig.reply(),
        "r refused: the daemon uploads its cuts, and its uploader lets each go once it is \
         committed\n"
    );
    assert!(stand.released.is_empty());
    rig.send("s held", &mut stand);
    assert_eq!(rig.reply(), "3 103 uploading itself 1\ns ok\n");
    let image = rig.image(4);
    rig.send(&format!("t write 3 {}", image.display()), &mut stand);
    rig.finish(&mut stand);
    assert_eq!(rig.reply(), "t ok 1 blocks\n");
}

/// The write puts each block of the map at its offset, as the cut reads
/// it, leaves every other block of the file as it was, lists the map in
/// `PATH.blocks`, and answers only when it is done, a slice a turn.
#[test]
fn write_puts_the_cut_s_map_into_the_file_a_slice_a_turn() {
    let mut rig = Rig::new();
    // Blocks 0, 2, 4, ... of 700, given out of order: two slices.
    let mut map: Vec<u64> = (0..350).map(|n| 2 * n).collect();
    map.reverse();
    let mut stand = Stand::holding(&[(3, CutPhase::Uploading, &[]), (4, CutPhase::Waiting, &map)]);
    let image = rig.image(700);
    assert!(rig.send(&format!("w write 4 {}", image.display()), &mut stand));
    assert_eq!(rig.reply(), "", "no reply before the write is done");
    let turns = rig.finish(&mut stand);
    assert_eq!(
        turns, 1,
        "after the signal's turn: one more slice, which ends it"
    );
    assert_eq!(rig.reply(), "w ok 350 blocks\n");
    let written = std::fs::read(&image).expect("the image");
    assert_eq!(written.len(), 700 * BLOCK_USIZE);
    for (block, bytes) in written.chunks(BLOCK_USIZE).enumerate() {
        let want = if block % 2 == 0 {
            content(4, block as u64)
        } else {
            0xEE
        };
        assert!(bytes.iter().all(|&b| b == want), "block {block}");
    }
    let listed = std::fs::read_to_string(blocks_path(&image)).expect("the list");
    let listed: Vec<u64> = listed
        .lines()
        .map(|l| l.parse().expect("a number"))
        .collect();
    let mut ascending = map.clone();
    ascending.sort_unstable();
    assert_eq!(listed, ascending);
}

#[test]
fn a_cut_that_is_empty_is_written_at_once() {
    let mut rig = Rig::new();
    let mut stand = Stand::holding(&[(3, CutPhase::Uploading, &[])]);
    let image = rig.image(2);
    assert!(!rig.send(&format!("e write 3 {}", image.display()), &mut stand));
    assert_eq!(rig.reply(), "e ok 0 blocks\n");
    assert_eq!(
        std::fs::read_to_string(blocks_path(&image)).expect("list"),
        ""
    );
}

#[test]
fn write_refuses_a_cut_it_cannot_write() {
    let mut rig = Rig::new();
    let mut stand = Stand::holding(&[
        (3, CutPhase::Uploading, &[0]),
        (5, CutPhase::Flushing, &[1]),
    ]);
    let image = rig.image(2);
    rig.send(&format!("a write 4 {}", image.display()), &mut stand);
    assert_eq!(rig.reply(), "a refused: no cut 4 is held\n");
    rig.send(&format!("b write 5 {}", image.display()), &mut stand);
    assert_eq!(
        rig.reply(),
        "b refused: cut 5's flush has not completed: only the cut being uploaded or the one that \
         waits can be written\n"
    );
    let missing = rig.dir.path().join("missing.img");
    rig.send(&format!("c write 3 {}", missing.display()), &mut stand);
    assert!(
        rig.reply()
            .starts_with(&format!("c refused: {}: ", missing.display())),
        "{}",
        rig.reply()
    );
    assert!(!missing.exists(), "the file is not created");
    assert_eq!(stand.reads, 0);
}

#[test]
fn a_command_while_a_write_is_under_way_is_refused_as_busy() {
    let mut rig = Rig::new();
    let map: Vec<u64> = (0..600).collect();
    let mut stand = Stand::holding(&[(3, CutPhase::Uploading, &map)]);
    let image = rig.image(600);
    assert!(rig.send(&format!("w write 3 {}", image.display()), &mut stand));
    assert!(rig.send("h held", &mut stand), "the write goes on");
    assert_eq!(
        rig.reply(),
        "h refused: busy: the write of cut 3 is under way\n"
    );
    rig.finish(&mut stand);
    assert_eq!(
        rig.reply(),
        "w ok 600 blocks\n",
        "the write's reply replaces it"
    );
}

#[test]
fn a_write_whose_cut_is_let_go_under_it_is_refused() {
    let mut rig = Rig::new();
    let map: Vec<u64> = (0..600).collect();
    let mut stand = Stand {
        let_go_after: Some(300),
        ..Stand::holding(&[(3, CutPhase::Uploading, &map)])
    };
    let image = rig.image(600);
    rig.send(&format!("w write 3 {}", image.display()), &mut stand);
    rig.finish(&mut stand);
    assert_eq!(
        rig.reply(),
        "w refused: cut 3 is no longer held after 299 of 600 blocks: it was let go, or abandoned \
         at the bound or at a copy that could not be made\n"
    );
}

#[test]
fn a_write_whose_map_grew_under_it_is_refused() {
    let mut rig = Rig::new();
    let map: Vec<u64> = (0..600).collect();
    let mut stand = Stand {
        grows_after: Some(100),
        ..Stand::holding(&[(4, CutPhase::Waiting, &map)])
    };
    let image = rig.image(600);
    rig.send(&format!("w write 4 {}", image.display()), &mut stand);
    rig.finish(&mut stand);
    assert_eq!(
        rig.reply(),
        "w refused: cut 4's map grew from 600 to 601 blocks during the write (an older cut was \
         abandoned at the bound and gave it its map): write it again\n"
    );
}

// The in-place store, on a simulated disk.

const WAIT: Duration = Duration::from_secs(60);
/// Two slices: the first cut, the whole volume, is written in two turns.
const SIZE_MIB: u64 = 2;
const BLOCKS: u64 = SIZE_MIB << 8;

type Store = InPlace<SimDisk>;

fn run(store: &mut Store, id: u64, op: Op) -> Completion {
    store
        .submit(Request {
            id: RequestId(id),
            op,
        })
        .expect("submitted");
    let mut done = Vec::new();
    while done.is_empty() {
        store.wait(&mut done, WAIT);
    }
    done.pop().expect("a completion")
}

fn write(store: &mut Store, offset: u64, data: Buf) {
    let done = run(store, offset, Op::Write { offset, data });
    assert!(
        done.result.is_ok(),
        "the write at {offset}: {:?}",
        done.result
    );
}

fn flush(store: &mut Store) {
    let done = run(store, u64::MAX, Op::Flush);
    assert!(done.result.is_ok(), "the flush: {:?}", done.result);
}

/// Where label 0's slot for group `txg` is, and an uberblock of that
/// group, as a pool with `ashift` 12 writes it, with the slot's label
/// checksum.
fn uberblock(txg: u64) -> (u64, Buf) {
    let ring = Labels::for_size(SIZE_MIB << 20)
        .rings()
        .next()
        .expect("four labels")
        .start;
    let offset = ring + (txg % (skyzfs_uberblock::RING_SIZE / BLOCK_SIZE)) * BLOCK_SIZE;
    let mut slot = Buf::zeroed(1);
    let mut put = |at: usize, word: u64| slot[at..at + 8].copy_from_slice(&word.to_le_bytes());
    put(0, MAGIC);
    put(8, 5000);
    put(16, txg);
    for n in 0..16 {
        put(40 + 8 * n, txg.rotate_left(n as u32) ^ 0x5a5a);
    }
    seal(&mut slot, offset, ByteOrder::Little);
    (offset, slot)
}

/// Group `txg`'s uberblock, written and flushed: a cut. The block its
/// slot is in.
fn cut(store: &mut Store, txg: u64) -> u64 {
    let (offset, slot) = uberblock(txg);
    write(store, offset, slot);
    flush(store);
    offset / BLOCK_SIZE
}

/// A volume on a simulated disk, started with held cuts in memory.
fn held_cuts_store() -> Store {
    held_cuts_store_on(SimDisk::new(11))
}

/// The same on `disk`, for a test that changes the disk under it.
fn held_cuts_store_on(disk: SimDisk) -> Store {
    let config = VolumeConfig {
        volume: *b"held cuts' rig  ",
        size_mib: SIZE_MIB,
        base: Base::Empty,
        write_unit: 4096,
    };
    inplace::create(&disk, &config).expect("create");
    let lock = inplace::lock(&disk).expect("the lock");
    let options = OpenOptions {
        hold_cuts: Some(HoldCuts {
            region: CutRegion::Memory(MemRegion::new(1)),
            boot: [7; 16],
            bound: None,
            intent_region: None,
            intent_header_sync: false,
        }),
        ..OpenOptions::default()
    };
    Opened::open(disk, lock, options)
        .map_err(|(e, _)| e)
        .expect("open")
        .start(None)
        .expect("start")
}

/// The whole protocol over the in-place store (the daemon's, on a
/// simulated disk): the first cut of a new state is the whole volume,
/// read as it was at the instant though a block changed after it; the
/// second is the blocks that changed between the two instants; only the
/// cut being uploaded is let go.
#[test]
fn the_in_place_store_s_cuts_are_written_as_they_were_at_their_instants() {
    let mut store = held_cuts_store();
    let mut rig = Rig::new();
    write(&mut store, 5 * BLOCK_SIZE, Buf::filled(1, 0x11));
    let first_slot = cut(&mut store, 7);
    write(&mut store, 5 * BLOCK_SIZE, Buf::filled(1, 0x22));
    write(&mut store, 9 * BLOCK_SIZE, Buf::filled(1, 0x33));

    rig.send("h held", &mut store);
    let reply = rig.reply();
    let lines: Vec<&str> = reply.lines().collect();
    assert_eq!(lines.len(), 2, "{reply}");
    assert!(lines[0].starts_with("1 7 uploading "), "{reply}");
    assert!(lines[0].ends_with(&format!(" {BLOCKS}")), "{reply}");
    assert_eq!(lines[1], "h ok");

    let image = rig.image(BLOCKS as usize);
    rig.send(&format!("w write 1 {}", image.display()), &mut store);
    rig.finish(&mut store);
    assert_eq!(rig.reply(), format!("w ok {BLOCKS} blocks\n"));
    let written = std::fs::read(&image).expect("the image");
    let block = |n: u64| &written[n as usize * BLOCK_USIZE..(n as usize + 1) * BLOCK_USIZE];
    assert!(
        block(5).iter().all(|&b| b == 0x11),
        "block 5 as at the instant"
    );
    assert!(
        block(9).iter().all(|&b| b == 0),
        "block 9 as at the instant"
    );
    assert_eq!(block(first_slot), &uberblock(7).1[..]);
    let listed = std::fs::read_to_string(blocks_path(&image)).expect("the list");
    assert_eq!(listed.lines().count() as u64, BLOCKS);

    // The second cut: what changed between the two instants.
    let second_slot = cut(&mut store, 8);
    write(&mut store, 9 * BLOCK_SIZE, Buf::filled(1, 0x44));
    rig.send("h2 held", &mut store);
    let reply = rig.reply();
    assert!(reply.contains("\n2 8 waiting "), "{reply}");
    rig.reply_gone();
    rig.send(&format!("w2 write 2 {}", image.display()), &mut store);
    rig.finish(&mut store);
    let mut changed = vec![5, 9, second_slot];
    changed.sort_unstable();
    assert_eq!(rig.reply(), format!("w2 ok {} blocks\n", changed.len()));
    let listed: Vec<u64> = std::fs::read_to_string(blocks_path(&image))
        .expect("the list")
        .lines()
        .map(|l| l.parse().expect("a number"))
        .collect();
    assert_eq!(listed, changed);
    let written = std::fs::read(&image).expect("the image");
    let block = |n: u64| &written[n as usize * BLOCK_USIZE..(n as usize + 1) * BLOCK_USIZE];
    assert!(
        block(5).iter().all(|&b| b == 0x22),
        "block 5 at the second instant"
    );
    assert!(
        block(9).iter().all(|&b| b == 0x33),
        "block 9 at the second instant"
    );

    rig.send("r release 2", &mut store);
    assert_eq!(
        rig.reply(),
        "r refused: cut 2 is not the one being uploaded: the cut being uploaded is 1\n"
    );
    rig.send("r2 release 1", &mut store);
    assert_eq!(rig.reply(), "r2 ok\n");
    rig.send("h3 held", &mut store);
    assert!(rig.reply().starts_with("2 8 uploading "), "{}", rig.reply());
}

/// A store started without held cuts refuses every command.
#[test]
fn the_in_place_store_without_held_cuts_refuses_the_commands() {
    let disk = SimDisk::new(12);
    let config = VolumeConfig {
        volume: *b"no held cuts    ",
        size_mib: SIZE_MIB,
        base: Base::Empty,
        write_unit: 4096,
    };
    inplace::create(&disk, &config).expect("create");
    let lock = inplace::lock(&disk).expect("the lock");
    let mut store = Opened::open(disk, lock, OpenOptions::default())
        .map_err(|(e, _)| e)
        .expect("open")
        .start(None)
        .expect("start");
    let mut rig = Rig::new();
    rig.send("h held", &mut store);
    assert_eq!(
        rig.reply(),
        "h refused: held cuts are off: the daemon was started without --hold-cuts\n"
    );
}

/// A turn with no signal reads no command, though the command's file is
/// still there and no write is under way: a command is carried out
/// once. Read again, a `release` answered `ok` would be answered again
/// under the same token, refused, and a `write` would begin again each
/// time it ended. From the review of #101 (S2, `gaps.py`).
#[test]
fn a_turn_with_no_signal_reads_no_command() {
    let mut rig = Rig::new();
    let mut stand = Stand::holding(&[(3, CutPhase::Uploading, &[1]), (4, CutPhase::Waiting, &[2])]);
    rig.send("s release 3", &mut stand);
    assert_eq!(rig.reply(), "s ok\n");
    assert!(!rig.commands.turn(false, &mut stand));
    assert_eq!(rig.reply(), "s ok\n", "the command is carried out once");
    assert_eq!(stand.released, [3]);
}

/// After the volume has failed, the instance that saw it still writes
/// out and lets go each cut whose flush had completed, a write under
/// way included (design §5.2, "When a cut may leave the machine"). The
/// cut whose flush failed with the volume is let go with it, so it is
/// refused. The lead's decision on the review of #101: a commit in its
/// review refused every command once the volume had failed.
#[test]
fn after_a_failure_the_cuts_whose_flush_completed_are_written_and_let_go() {
    let disk = SimDisk::new(13);
    let mut store = held_cuts_store_on(disk.clone());
    let mut rig = Rig::new();
    write(&mut store, 5 * BLOCK_SIZE, Buf::filled(1, 0x11));
    let first_slot = cut(&mut store, 7);
    write(&mut store, 5 * BLOCK_SIZE, Buf::filled(1, 0x22));
    let second_slot = cut(&mut store, 8);
    write(&mut store, 5 * BLOCK_SIZE, Buf::filled(1, 0x44));
    write(&mut store, 9 * BLOCK_SIZE, Buf::filled(1, 0x33));

    // Cut 1's write begins: the first of its two slices.
    let image = rig.image(BLOCKS as usize);
    assert!(rig.send(&format!("w write 1 {}", image.display()), &mut store));
    assert_eq!(rig.reply(), "", "no reply after the first slice");

    // Group 9's flush fails, and the volume with it.
    let (offset, slot) = uberblock(9);
    write(&mut store, offset, slot);
    disk.fail_sync_of(skyzfs_local::store::HOME_NAME);
    let done = run(&mut store, u64::MAX, Op::Flush);
    assert!(done.result.is_err(), "the flush: {:?}", done.result);
    assert!(store.is_failed());

    // The write under way goes on to its end, as at the instant.
    rig.finish(&mut store);
    assert_eq!(rig.reply(), format!("w ok {BLOCKS} blocks\n"));
    let written = std::fs::read(&image).expect("the image");
    let block = |n: u64| &written[n as usize * BLOCK_USIZE..(n as usize + 1) * BLOCK_USIZE];
    assert!(block(5).iter().all(|&b| b == 0x11), "block 5 of cut 1");
    assert!(block(9).iter().all(|&b| b == 0), "block 9 of cut 1");
    assert_eq!(block(first_slot), &uberblock(7).1[..]);
    rig.reply_gone();

    // Cut 3, whose flush failed, is not held; cuts 1 and 2 are.
    rig.send("h held", &mut store);
    let reply = rig.reply();
    let lines: Vec<&str> = reply.lines().collect();
    assert_eq!(lines.len(), 3, "{reply}");
    assert!(lines[0].starts_with("1 7 uploading "), "{reply}");
    assert!(lines[1].starts_with("2 8 waiting "), "{reply}");
    assert_eq!(lines[2], "h ok");
    rig.send(&format!("w3 write 3 {}", image.display()), &mut store);
    assert_eq!(rig.reply(), "w3 refused: no cut 3 is held\n");

    // Cut 2 is written as at its instant, and both are let go in turn.
    rig.send(&format!("w2 write 2 {}", image.display()), &mut store);
    rig.finish(&mut store);
    let mut changed = [5, second_slot];
    changed.sort_unstable();
    assert_eq!(rig.reply(), format!("w2 ok {} blocks\n", changed.len()));
    let written = std::fs::read(&image).expect("the image");
    let block = |n: u64| &written[n as usize * BLOCK_USIZE..(n as usize + 1) * BLOCK_USIZE];
    assert!(block(5).iter().all(|&b| b == 0x22), "block 5 of cut 2");
    rig.send("r release 1", &mut store);
    assert_eq!(rig.reply(), "r ok\n");
    rig.send("h2 held", &mut store);
    assert!(rig.reply().starts_with("2 8 uploading "), "{}", rig.reply());
    rig.send("r2 release 2", &mut store);
    assert_eq!(rig.reply(), "r2 ok\n");
    rig.send("h3 held", &mut store);
    assert_eq!(rig.reply(), "h3 ok\n");
}
