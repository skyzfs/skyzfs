//! Review scenarios for #112 at 4c887b5 (M2.4b-1, the next look), at the level of the cut
//! state. Not part of the product: `scenarios.py` puts this file beside
//! `state.rs` as `state/review_m24b1.rs` in a checkout of its own, runs it
//! and takes it out again.
//!
//! The model is the volume at each cut's instant. A held cut is right when
//! every block it reads holds what the volume held at its instant, every
//! block it does not read is as the cut numbered `lo` held it (the cut it
//! says it was made against), and the cuts chain: the oldest against the
//! cut that was uploaded, or whole if none was, and each next one against
//! a cut no newer than the one before it.

use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};

use super::*;

const BOUND: u64 = 64 * BLOCK_SIZE;
const OWNER: Owner = Owner {
    volume: *b"the review's vol",
    boot: *b"the review's boo",
    start: *b"the review's sta",
};

#[derive(Debug, Clone)]
struct Vol {
    image: Vec<u64>,
    copies: BTreeMap<u32, u64>,
}

impl Copier for Vol {
    fn read_image(&mut self, first: u64, buf: &mut [u8]) -> io::Result<()> {
        for (i, block) in buf.chunks_mut(BLOCK_SIZE as usize).enumerate() {
            block[..8].copy_from_slice(&self.image[first as usize + i].to_le_bytes());
        }
        Ok(())
    }

    fn write_copies(&mut self, first: u32, buf: &[u8]) -> io::Result<()> {
        for (i, block) in buf.chunks(BLOCK_SIZE as usize).enumerate() {
            let mut word = [0u8; 8];
            word.copy_from_slice(&block[..8]);
            self.copies
                .insert(first + i as u32, u64::from_le_bytes(word));
        }
        Ok(())
    }
}

struct World {
    region: MemRegion,
    blocks: u64,
    vol: Vol,
    next: u64,
    /// The volume at each cut's instant, by the cut's number.
    snaps: BTreeMap<u64, Vec<u64>>,
}

type St = CutState<MemRegion>;

impl World {
    fn new(blocks: u64) -> (Self, St) {
        let region = MemRegion::new(words_for(blocks, records_for(BOUND)));
        let state = CutState::begin(
            region.clone(),
            blocks * BLOCK_SIZE,
            BOUND,
            OWNER,
            Mutants(0),
        );
        let world = Self {
            region,
            blocks,
            // Not zeros: a block no cut reads must not pass for one read.
            vol: Vol {
                image: (0..blocks).map(|b| 1_000_000 + b).collect(),
                copies: BTreeMap::new(),
            },
            next: 0,
            snaps: BTreeMap::new(),
        };
        (world, state)
    }

    fn size(&self) -> u64 {
        self.blocks * BLOCK_SIZE
    }

    /// The copies first, then the write, as the store does.
    fn write(&mut self, state: &mut St, first: u64, n: u64) {
        state.change(first, n, &mut self.vol).expect("the change");
        for block in first..first + n {
            self.next += 1;
            self.vol.image[block as usize] = self.next;
        }
    }

    fn cut(&mut self, state: &mut St, mark: u64) -> u64 {
        let number = state.freeze(&marked(mark), true);
        self.snaps.insert(number, self.vol.image.clone());
        number
    }

    fn view(&self) -> St {
        CutState::view(self.region.clone(), self.size(), BOUND, OWNER, Mutants(0))
            .map_err(|(e, _)| e)
            .expect("the state, read as it is")
    }

    fn restore(&self) -> St {
        CutState::restore(self.region.clone(), self.size(), BOUND, OWNER, Mutants(0))
            .map_err(|(e, _)| e)
            .expect("the state")
    }

    /// Every held cut is right (see the top of the file). `uploaded` is
    /// the cut in object storage, `None` where nothing was uploaded.
    fn check(&self, state: &St, uploaded: Option<u64>, case: &str) {
        let held = state.held();
        for (i, cut) in held.iter().enumerate() {
            let n = cut.number;
            let whole = state.is_whole_cut(n);
            let against = if i == 0 {
                uploaded
            } else {
                Some(held[i - 1].number)
            };
            assert!(
                whole || against.is_some_and(|against| cut.lo <= against),
                "{case}: cut {n} (lo {}) is not whole and is not made against {against:?}",
                cut.lo
            );
            let snap = &self.snaps[&n];
            for block in 0..self.blocks {
                let at = block as usize;
                match state.read(n, block).expect("a held cut") {
                    Read::Unchanged => {
                        assert!(!whole, "{case}: whole cut {n} does not read block {block}");
                        let base = self
                            .snaps
                            .get(&cut.lo)
                            .unwrap_or_else(|| panic!("{case}: cut {n} has lo {}", cut.lo));
                        assert_eq!(
                            snap[at], base[at],
                            "{case}: cut {n} lacks block {block}, changed after cut {}",
                            cut.lo
                        );
                    }
                    Read::Image => assert_eq!(
                        self.vol.image[at], snap[at],
                        "{case}: cut {n} reads block {block} from the image"
                    ),
                    Read::Copy(slot) => assert_eq!(
                        self.vol.copies.get(&slot).copied(),
                        Some(snap[at]),
                        "{case}: cut {n} reads block {block} from slot {slot}"
                    ),
                }
            }
            // The map from a block on, in slices, is the map.
            let map = state.map(n);
            for size in [1usize, 7, 64] {
                let mut got = Vec::new();
                let mut from = Some(0);
                while let Some(at) = from {
                    let (blocks, next) = state.map_from(n, at, size).expect("a held cut");
                    assert!(blocks.len() <= size, "{case}");
                    assert!(!blocks.is_empty() || map.is_empty(), "{case}");
                    assert!(next.is_none_or(|next| next > at), "{case}");
                    got.extend(blocks);
                    from = next;
                }
                assert!(got == map, "{case}: cut {n} in slices of {size}");
            }
        }
    }
}

fn marked(number: u64) -> State {
    State {
        txg: 100 + number,
        root: [number; ROOT_WORDS],
    }
}

fn quiet() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if info.payload().downcast_ref::<Lost>().is_none() {
                default(info);
            }
        }));
    });
}

fn numbers(state: &St) -> Vec<u64> {
    state.held().iter().map(|c| c.number).collect()
}

/// Cut 1 uploaded and let go; cut 2 being uploaded, cut 3 waiting, cut 4
/// with its flush under way (or, with `flushing` false, flushed: it took
/// cut 3's map and waits); copies made for each of them, and blocks in
/// the open map.
fn three_held(flushing: bool) -> (World, St) {
    let (mut w, mut s) = World::new(200);
    w.write(&mut s, 10, 1);
    assert_eq!(w.cut(&mut s, 1), 1);
    s.flushed(1);
    assert!(s.release(1));
    w.write(&mut s, 20, 1);
    assert_eq!(w.cut(&mut s, 2), 2);
    s.flushed(2);
    w.write(&mut s, 30, 3);
    w.write(&mut s, 100, 1);
    w.write(&mut s, 170, 1);
    w.write(&mut s, 20, 1);
    assert_eq!(w.cut(&mut s, 3), 3);
    s.flushed(3);
    w.write(&mut s, 40, 1);
    w.write(&mut s, 30, 1);
    w.write(&mut s, 20, 1);
    w.write(&mut s, 60, 2);
    assert_eq!(w.cut(&mut s, 4), 4);
    w.write(&mut s, 50, 1);
    w.write(&mut s, 40, 1);
    w.write(&mut s, 20, 1);
    w.write(&mut s, 100, 1);
    w.write(&mut s, 64, 1);
    if !flushing {
        s.flushed(4);
    }
    assert!(s.figures().copied > 0);
    w.check(&s, Some(1), "before");
    (w, s)
}

/// After a mismatch has let every cut go: none is held, the flush that was
/// under way changes nothing when it ends, the next cut holds the whole
/// volume and reads it as at its instant under writes, and the cut after
/// it holds its own changes.
fn after_all(w: &mut World, s: &mut St, case: &str) {
    assert!(s.held().is_empty(), "{case}");
    assert_eq!(s.figures().copied, 0, "{case}");
    let counted = s.figures().abandoned;
    s.flushed(4);
    s.flush_failed(4);
    assert!(s.held().is_empty(), "{case}");
    assert_eq!(s.figures().abandoned, counted, "{case}");
    w.write(s, 7, 1);
    w.write(s, 20, 1);
    let next = w.cut(s, 20);
    assert!(s.is_whole_cut(next), "{case}: the next cut is whole");
    w.write(s, 20, 1);
    w.write(s, 0, 1);
    w.write(s, 199, 1);
    w.write(s, 63, 3);
    w.check(s, None, &format!("{case}, the whole cut under writes"));
    s.flushed(next);
    assert_eq!(
        s.held().first().map(|c| (c.number, c.phase)),
        Some((next, Phase::Uploading)),
        "{case}"
    );
    assert!(s.release(next), "{case}");
    w.write(s, 61, 1);
    let after = w.cut(s, 21);
    assert!(!s.is_whole_cut(after), "{case}: the cut after is not whole");
    assert_eq!(s.map(after), [0, 20, 61, 63, 64, 65, 199], "{case}");
    w.check(s, Some(next), &format!("{case}, the cut after"));
}

/// S1. A death at each store of `abandon_all`, with and without a flush
/// under way: the state read at the death and the state the next start
/// takes up hold right cuts; the count is exact; the daemon then serves
/// (writes, a cut) before the uploader checks again; the second
/// `abandon_all` ends as one without a death ends.
#[test]
fn review_s1_a_death_at_each_store_of_abandon_all() {
    quiet();
    for flushing in [true, false] {
        let mut deaths = 0;
        let mut merges = 0;
        let mut counts: BTreeMap<(u64, usize), u64> = BTreeMap::new();
        for death in 0u64.. {
            let (mut w, mut s) = three_held(flushing);
            let held_before = numbers(&s);
            let before = s.figures().abandoned;
            w.region.die_at(death);
            let done = catch_unwind(AssertUnwindSafe(|| s.abandon_all()));
            w.region.revive();
            let case = format!("flushing {flushing}, a death at store {death}");
            match done {
                Ok(abandoned) => {
                    assert_eq!(abandoned, held_before);
                    assert_eq!(
                        s.figures().abandoned - before,
                        held_before.len() as u64,
                        "no death"
                    );
                    after_all(&mut w, &mut s, "no death");
                    break;
                }
                Err(payload) => {
                    if payload.downcast_ref::<Lost>().is_none() {
                        std::panic::resume_unwind(payload);
                    }
                }
            }
            deaths += 1;
            drop(s);
            let view = w.view();
            w.check(&view, Some(1), &format!("{case}, read at the death"));
            // The start takes its sync as cut 4's flush: cut 4 replaces
            // cut 3 if cut 3 still waits, and that is not an abandoning.
            let waits = |n: u64, phase: Phase| {
                view.held()
                    .iter()
                    .any(|c| c.number == n && c.phase == phase)
            };
            let merged = flushing && waits(3, Phase::Waiting) && waits(4, Phase::Flushing);
            merges += u64::from(merged);
            drop(view);
            let mut s = w.restore();
            w.check(&s, Some(1), &format!("{case}, taken up"));
            let held = numbers(&s);
            let counted = s.figures().abandoned - before;
            *counts.entry((counted, held.len())).or_default() += 1;
            assert_eq!(
                counted + held.len() as u64,
                held_before.len() as u64 - u64::from(merged),
                "{case}: {counted} counted, {held:?} held, merged at the start: {merged}"
            );
            if let Some(oldest) = s.held().first() {
                assert_eq!(oldest.phase, Phase::Uploading, "{case}");
            }
            // The daemon serves before the uploader has checked again.
            w.write(&mut s, 20, 1);
            w.write(&mut s, 70, 2);
            w.write(&mut s, 40, 1);
            let five = w.cut(&mut s, 5);
            w.write(&mut s, 20, 1);
            w.write(&mut s, 71, 1);
            w.check(&s, Some(1), &format!("{case}, served on"));
            let held = numbers(&s);
            assert!(held.contains(&five), "{case}");
            let counted = s.figures().abandoned - before;
            assert_eq!(s.abandon_all(), held, "{case}");
            assert_eq!(
                s.figures().abandoned - before,
                counted + held.len() as u64,
                "{case}: each cut counted once"
            );
            after_all(&mut w, &mut s, &case);
        }
        println!(
            "S1 flushing {flushing}: {deaths} deaths, all right; the start merged cut 3 into cut 4 \
             at {merges} of them; (counted, held) at the start: {counts:?}"
        );
        assert!(deaths > 3);
    }
}

/// S2. The same where nothing was ever uploaded: the cut being uploaded
/// is the whole volume (a new state's first cut). At every store of
/// `abandon_all` the oldest cut held is whole, read at the death and
/// taken up, so that whatever is uploaded next needs no base.
#[test]
fn review_s2_a_death_in_abandon_all_with_the_whole_cut_held() {
    quiet();
    let setup = || {
        let (mut w, mut s) = World::new(200);
        w.write(&mut s, 3, 1);
        assert_eq!(w.cut(&mut s, 1), 1);
        s.flushed(1);
        w.write(&mut s, 4, 1);
        w.write(&mut s, 3, 1);
        assert_eq!(w.cut(&mut s, 2), 2);
        s.flushed(2);
        w.write(&mut s, 5, 1);
        w.write(&mut s, 4, 1);
        assert_eq!(w.cut(&mut s, 3), 3);
        w.write(&mut s, 6, 1);
        w.write(&mut s, 3, 1);
        assert!(s.is_whole_cut(1));
        w.check(&s, None, "before");
        (w, s)
    };
    let mut deaths = 0;
    for death in 0u64.. {
        let (mut w, mut s) = setup();
        w.region.die_at(death);
        let done = catch_unwind(AssertUnwindSafe(|| s.abandon_all()));
        w.region.revive();
        let case = format!("a death at store {death}");
        match done {
            Ok(abandoned) => {
                assert_eq!(abandoned, [1, 2, 3]);
                after_all(&mut w, &mut s, "no death");
                break;
            }
            Err(payload) => {
                if payload.downcast_ref::<Lost>().is_none() {
                    std::panic::resume_unwind(payload);
                }
            }
        }
        deaths += 1;
        drop(s);
        let view = w.view();
        w.check(&view, None, &format!("{case}, read at the death"));
        drop(view);
        let mut s = w.restore();
        w.check(&s, None, &format!("{case}, taken up"));
        w.write(&mut s, 3, 1);
        w.write(&mut s, 9, 1);
        w.check(&s, None, &format!("{case}, served on"));
        // With no check by the uploader at all: the next cut is whole.
        if s.held().is_empty() {
            let mut w2 = World {
                region: w.region.snapshot(),
                blocks: w.blocks,
                vol: w.vol.clone(),
                next: w.next,
                snaps: w.snaps.clone(),
            };
            let mut s2 = w2.restore();
            let next = w2.cut(&mut s2, 30);
            assert!(
                s2.is_whole_cut(next),
                "{case}: no cut held and nothing uploaded, and the next cut is not whole"
            );
        }
        let held = numbers(&s);
        assert_eq!(s.abandon_all(), held, "{case}");
        after_all(&mut w, &mut s, &case);
    }
    println!("S2: {deaths} deaths, the oldest cut held whole at each");
    assert!(deaths > 3);
}

/// S3. `map_from` on a volume whose maps have several words of summary
/// (more than 4096 blocks; the tests of the head have 200 and 256): from
/// every block around the edges of words and of summary words, with
/// several limits, it lists what `map` lists from that block on, and
/// names the block after.
#[test]
fn review_s3_map_from_over_several_words_of_summary() {
    let blocks = 3 * 4096 + 100;
    let (mut w, mut s) = World::new(blocks);
    w.write(&mut s, 0, 1);
    assert_eq!(w.cut(&mut s, 1), 1);
    s.flushed(1);
    // The whole cut, from a block on.
    for from in [0, 1, 4095, 4096, 8191, 8192, blocks - 1, blocks, blocks + 5] {
        for limit in [1usize, 3, 64, 5000] {
            let first = from.min(blocks);
            let end = (first + limit as u64).min(blocks);
            assert_eq!(
                s.map_from(1, from, limit),
                Some(((first..end).collect(), (end < blocks).then_some(end))),
                "the whole cut from {from}, {limit} blocks"
            );
        }
    }
    assert!(s.release(1));
    let set: Vec<u64> = vec![
        0, 63, 64, 65, 4095, 4096, 4097, 8191, 8192, 9000, 12287, 12288, 12387,
    ];
    for &block in &set {
        w.write(&mut s, block, 1);
    }
    assert_eq!(w.cut(&mut s, 2), 2);
    s.flushed(2);
    assert!(!s.is_whole_cut(2));
    assert_eq!(s.map(2), set);
    let mut asked = 0;
    let froms: Vec<u64> = set
        .iter()
        .flat_map(|&b| [b.saturating_sub(1), b, b + 1])
        .chain([2, 100, 4000, 5000, 8000, 10000, 12000, blocks, blocks + 5, 1 << 40])
        .collect();
    for &from in &froms {
        for limit in [1usize, 2, 3, 5, 64] {
            let rest: Vec<u64> = set.iter().copied().filter(|&b| b >= from).collect();
            let expected: Vec<u64> = rest.iter().copied().take(limit).collect();
            let next = rest.get(limit).copied();
            assert_eq!(
                s.map_from(2, from, limit),
                Some((expected, next)),
                "cut 2 from block {from}, {limit} blocks"
            );
            asked += 1;
        }
    }
    // And two parts: cut 3 takes cut 2's map as the bound lets cut 2 go.
    w.write(&mut s, 5000, 1);
    w.write(&mut s, 64, 1);
    assert_eq!(w.cut(&mut s, 3), 3);
    s.flushed(3);
    assert!(s.abandon_uploading(2));
    let mut union = set.clone();
    union.push(5000);
    union.sort_unstable();
    assert_eq!(s.map(3), union);
    for &from in &froms {
        let rest: Vec<u64> = union.iter().copied().filter(|&b| b >= from).collect();
        assert_eq!(
            s.map_from(3, from, 4),
            Some((rest.iter().copied().take(4).collect(), rest.get(4).copied())),
            "cut 3 from block {from}"
        );
    }
    w.check(&s, Some(1), "cut 3 after it took cut 2's map");
    println!("S3: {asked} slices of a map over four words of summary, all as map() lists them");
}
