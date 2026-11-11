//! The translation's rules (design §4.1 as §4.2 applies it), each with
//! the defect that shows the test would notice.

use proptest::prelude::*;
use skyzfs_blk::{Buf, Op};

use super::*;

type Outcome = Result<(), String>;

/// A device of 8 MiB: 16384 sectors, 2048 blocks.
fn geometry() -> Geometry {
    Geometry::new(8, 64, false).unwrap()
}

const SECTORS: u64 = 8 * 1024 * 1024 / SECTOR;

/// The scenario holds for the translation as it is and fails with the
/// defect.
fn caught(scenario: fn(&Translator) -> Outcome, mutant: u32) {
    holds(scenario);
    let broken = Translator::with_mutants(&geometry(), mutant);
    assert!(
        scenario(&broken).is_err(),
        "the defect {mutant:#x} was not noticed"
    );
}

fn holds(scenario: fn(&Translator) -> Outcome) {
    if let Err(why) = scenario(&Translator::new(&geometry())) {
        panic!("{why}");
    }
}

fn desc(op: u8, flags: u32, start_sector: u64, nr_sectors: u32) -> Desc {
    Desc {
        op_flags: u32::from(op) | flags,
        nr_sectors,
        start_sector,
    }
}

fn expect(t: &Translator, d: Desc, want: Result<Plan, Refusal>) -> Outcome {
    let got = t.translate(d);
    if got == want {
        Ok(())
    } else {
        Err(format!("{d:?}: expected {want:?}, got {got:?}"))
    }
}

fn plain(action: Action) -> Result<Plan, Refusal> {
    Ok(Plan {
        action,
        then_flush: false,
    })
}

/// Every flag the driver can set that is only a hint.
const HINTS: [u32; 6] = [
    flag::FAILFAST_DEV,
    flag::FAILFAST_TRANSPORT,
    flag::FAILFAST_DRIVER,
    flag::META,
    flag::NOUNMAP,
    flag::SWAP,
];

/// The action each ranged operation makes of sectors 16..32.
fn ranged() -> [(u8, Action); 4] {
    let (offset, len) = (16 * SECTOR, 16 * SECTOR);
    [
        (op::READ, Action::Read { offset, len }),
        (op::WRITE, Action::Write { offset, len }),
        (op::DISCARD, Action::Discard { offset, len }),
        (op::WRITE_ZEROES, Action::WriteZeroes { offset, len }),
    ]
}

/// The five operations: each is translated to its operation of the
/// contract with the range in bytes, under every hint flag the driver
/// can set, alone and all together, and under bits this file does not
/// know. A flush has no range, whatever its sector fields hold.
#[test]
fn the_five_operations_are_translated_under_every_hint_flag() {
    fn scenario(t: &Translator) -> Outcome {
        let all = HINTS.iter().fold(0, |a, f| a | f);
        let unknown = (1 << 12) | (1 << 14) | (1 << 20) | (1 << 31);
        for flags in HINTS.into_iter().chain([0, all, unknown, all | unknown]) {
            for (op, action) in ranged() {
                expect(t, desc(op, flags, 16, 16), plain(action))?;
            }
            for (start, sectors) in [(0, 0), (16, 16), (u64::MAX, u32::MAX), (3, 5)] {
                expect(
                    t,
                    desc(op::FLUSH, flags, start, sectors),
                    plain(Action::Flush),
                )?;
            }
        }
        Ok(())
    }
    holds(scenario);
}

/// Forced unit access: a write, a write-zeroes and a discard that carry
/// the flag are followed by a flush before they are acknowledged, with
/// or without other flags. A read and a flush are not changed by it.
/// Mutant: the flag is dropped.
#[test]
fn a_request_with_fua_is_followed_by_a_flush() {
    fn scenario(t: &Translator) -> Outcome {
        for extra in [0, flag::META | flag::FAILFAST_DEV, flag::NOUNMAP] {
            for (op, action) in ranged() {
                let want = Ok(Plan {
                    action,
                    then_flush: op != op::READ,
                });
                expect(t, desc(op, flag::FUA | extra, 16, 16), want)?;
            }
            expect(
                t,
                desc(op::FLUSH, flag::FUA | extra, 0, 0),
                plain(Action::Flush),
            )?;
        }
        Ok(())
    }
    caught(scenario, Mutants::FUA_WITHOUT_FLUSH);
}

/// Operations outside the five are refused and build nothing:
/// write-same, every zone operation, and every other number, under any
/// flags. Mutant: write-same is taken for a write.
#[test]
fn operations_outside_the_five_are_refused() {
    fn scenario(t: &Translator) -> Outcome {
        let five = [
            op::READ,
            op::WRITE,
            op::FLUSH,
            op::DISCARD,
            op::WRITE_ZEROES,
        ];
        for number in (0..=255u8).filter(|n| !five.contains(n)) {
            for flags in [0, flag::FUA, flag::META] {
                expect(
                    t,
                    desc(number, flags, 16, 16),
                    Err(Refusal::Operation(number)),
                )?;
            }
        }
        for zoned in op::ZONED.into_iter().chain([op::WRITE_SAME]) {
            if five.contains(&zoned) {
                return Err(format!("{zoned} is listed as outside the five"));
            }
        }
        Ok(())
    }
    caught(scenario, Mutants::WRITE_SAME_IS_WRITE);
}

/// A flag that says the data is somewhere else is refused for every
/// operation: this transport would read or write the wrong bytes.
#[test]
fn a_request_whose_data_is_elsewhere_is_refused() {
    fn scenario(t: &Translator) -> Outcome {
        for elsewhere in [flag::NEED_REG_BUF, flag::INTEGRITY, flag::SHMEM_ZC] {
            for (op, _) in ranged() {
                expect(
                    t,
                    desc(op, elsewhere | flag::META, 16, 16),
                    Err(Refusal::Flags(elsewhere)),
                )?;
            }
        }
        Ok(())
    }
    holds(scenario);
}

/// A request whose offset or length is not a multiple of 4 KiB is
/// refused and builds nothing, for all four ranged operations: it is
/// not rounded. Mutant: it is rounded to blocks and passed on.
#[test]
fn a_misaligned_request_is_refused() {
    fn scenario(t: &Translator) -> Outcome {
        for (start, sectors) in [(1, 8), (7, 8), (8, 1), (8, 7), (8, 9), (4, 4), (0, 2047)] {
            for (op, _) in ranged() {
                expect(
                    t,
                    desc(op, 0, start, sectors),
                    Err(Refusal::Misaligned { start, sectors }),
                )?;
            }
        }
        Ok(())
    }
    caught(scenario, Mutants::ROUND_MISALIGNED);
}

/// The edges of the device: the first block and the last are served; a
/// request that ends one block beyond the end, starts at the end, lies
/// far beyond it, or whose end overflows, is refused and builds
/// nothing. Mutant: a request beyond the end is passed on.
#[test]
fn a_request_beyond_the_end_is_refused_and_the_last_block_is_served() {
    fn scenario(t: &Translator) -> Outcome {
        let last = SECTORS - 8;
        for (op, _) in ranged() {
            for start in [0, last] {
                if t.translate(desc(op, 0, start, 8)).is_err() {
                    return Err(format!("op {op} at sector {start} was refused"));
                }
            }
            for (start, sectors) in [
                (last, 16),
                (SECTORS, 8),
                (SECTORS + 8, 8),
                (1 << 40, 8),
                (u64::MAX - 7, 8),
                ((u64::MAX / SECTOR + 1) & !7, 8),
                (!7u64, 8),
            ] {
                expect(
                    t,
                    desc(op, 0, start, sectors),
                    Err(Refusal::OutOfBounds { start, sectors }),
                )?;
            }
        }
        Ok(())
    }
    caught(scenario, Mutants::PASS_OUT_OF_BOUNDS);
}

/// An empty request is refused; a read or a write of exactly 1 MiB is
/// served and one block more is refused; a discard and a write-zeroes
/// may span the whole device.
#[test]
fn the_limits_on_length_are_those_of_the_contract() {
    fn scenario(t: &Translator) -> Outcome {
        let mib = (MAX_IO_BYTES / SECTOR) as u32;
        for (op, _) in ranged() {
            expect(t, desc(op, 0, 16, 0), Err(Refusal::Empty))?;
        }
        for op in [op::READ, op::WRITE] {
            if t.translate(desc(op, 0, 8, mib)).is_err() {
                return Err("1 MiB was refused".into());
            }
            expect(
                t,
                desc(op, 0, 8, mib + 8),
                Err(Refusal::TooLarge { sectors: mib + 8 }),
            )?;
        }
        let whole = SECTORS as u32;
        let len = SECTORS * SECTOR;
        expect(
            t,
            desc(op::DISCARD, 0, 0, whole),
            plain(Action::Discard { offset: 0, len }),
        )?;
        expect(
            t,
            desc(op::WRITE_ZEROES, 0, 0, whole),
            plain(Action::WriteZeroes { offset: 0, len }),
        )
    }
    holds(scenario);
}

/// A flush or a discard is never answered "not supported", and neither
/// is anything else: every refusal and every failure, of whatever kind,
/// is the plain I/O error. Mutant: a refused discard is answered "not
/// supported".
#[test]
fn every_failure_is_a_plain_io_error_and_never_not_supported() {
    fn scenario(t: &Translator) -> Outcome {
        let failures = [
            BlkError::Failed,
            BlkError::ReadOnly,
            BlkError::ZeroLength,
            BlkError::Misaligned { offset: 1, len: 1 },
            BlkError::OutOfBounds {
                offset: 0,
                len: 1,
                size: 0,
            },
            BlkError::TooLarge { len: 2, max: 1 },
        ];
        for number in 0..=255u8 {
            let d = desc(number, 0, 1, 1);
            if t.refused(d) != -EIO {
                return Err(format!(
                    "a refused op {number} is answered {}",
                    t.refused(d)
                ));
            }
        }
        let flush = Plan {
            action: Action::Flush,
            then_flush: false,
        };
        let plans = ranged().map(|(_, action)| Plan {
            action,
            then_flush: false,
        });
        for plan in plans.iter().chain([&flush]) {
            for failure in &failures {
                let got = t.result(plan, &Err(*failure));
                if got != -EIO {
                    return Err(format!("{plan:?} failing with {failure:?} gives {got}"));
                }
            }
        }
        Ok(())
    }
    caught(scenario, Mutants::DISCARD_NOT_SUPPORTED);
    assert_eq!(EIO, 5);
}

/// No short counts: a read or a write that succeeded is answered with
/// its whole length in bytes, up to 1 MiB, and the other operations
/// with 0. Mutant: a read or write is answered with 0 bytes.
#[test]
fn a_success_is_answered_with_the_whole_length() {
    fn scenario(t: &Translator) -> Outcome {
        let mib = (MAX_IO_BYTES / SECTOR) as u32;
        for sectors in [8, 16, mib] {
            for op in [op::READ, op::WRITE] {
                let plan = t
                    .translate(desc(op, 0, 0, sectors))
                    .map_err(|e| e.to_string())?;
                let got = t.result(&plan, &Ok(()));
                if got != (sectors * 512) as i32 {
                    return Err(format!("{sectors} sectors answered with {got}"));
                }
            }
        }
        for (start, sectors, op) in [
            (0, 8, op::DISCARD),
            (8, 64, op::WRITE_ZEROES),
            (0, 0, op::FLUSH),
        ] {
            let plan = t
                .translate(desc(op, 0, start, sectors))
                .map_err(|e| e.to_string())?;
            if t.result(&plan, &Ok(())) != 0 {
                return Err(format!("op {op} is not answered with 0"));
            }
        }
        Ok(())
    }
    caught(scenario, Mutants::SHORT_COUNT);
}

/// The contract's operation for a plan, to ask the contract's own
/// validation about it.
fn op_of(action: Action) -> Op {
    match action {
        Action::Read { offset, len } => Op::Read {
            offset,
            buf: Buf::zeroed((len / BLOCK_SIZE) as usize),
        },
        Action::Write { offset, len } => Op::Write {
            offset,
            data: Buf::zeroed((len / BLOCK_SIZE) as usize),
        },
        Action::Flush => Op::Flush,
        Action::Discard { offset, len } => Op::Discard { offset, len },
        Action::WriteZeroes { offset, len } => Op::WriteZeroes { offset, len },
    }
}

proptest! {
    /// Rejected requests reach nothing, as a property: whatever the
    /// descriptor, a plan is built only for a request the contract's
    /// own validation accepts, with exactly the descriptor's range in
    /// bytes; and a request of the five whose range the contract would
    /// accept is not refused.
    #[test]
    fn a_plan_is_built_exactly_for_what_the_contract_accepts(
        op in prop_oneof![0u8..=5, any::<u8>()],
        flags in prop_oneof![Just(0u32), any::<u32>().prop_map(|f| f & !0xff & !flag::DATA_ELSEWHERE)],
        start in prop_oneof![0..SECTORS + 64, any::<u64>()],
        sectors in prop_oneof![0u32..4096, any::<u32>()],
    ) {
        let geometry = geometry();
        let t = Translator::new(&geometry);
        let d = desc(op, flags, start, sectors);
        let five = [op::READ, op::WRITE, op::FLUSH, op::DISCARD, op::WRITE_ZEROES].contains(&op);
        match t.translate(d) {
            Ok(plan) => {
                prop_assert!(five);
                let built = op_of(plan.action);
                prop_assert!(geometry.validate(&built).is_ok(), "{:?} from {:?}", plan, d);
                if let Some((offset, len)) = built.range() {
                    prop_assert_eq!(offset, start * SECTOR);
                    prop_assert_eq!(len, u64::from(sectors) * SECTOR);
                }
                prop_assert_eq!(plan.then_flush, flags & flag::FUA != 0 && op != op::READ && op != op::FLUSH);
            }
            Err(_) if !five => {}
            Err(refusal) => {
                // Refused although one of the five: the range is bad.
                let aligned = start % 8 == 0 && sectors % 8 == 0 && sectors > 0;
                let len = u64::from(sectors) * SECTOR;
                let inside = start
                    .checked_mul(SECTOR)
                    .and_then(|o| o.checked_add(len))
                    .is_some_and(|end| end <= geometry.size_bytes());
                let data = op == op::READ || op == op::WRITE;
                let fits = !data || len <= MAX_IO_BYTES;
                prop_assert!(!(aligned && inside && fits), "{:?} refused: {}", d, refusal);
            }
        }
    }
}
