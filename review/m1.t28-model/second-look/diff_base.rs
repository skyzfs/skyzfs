//! Review probe: random histories through the base's model and the head's
//! model with `InFlight::Reissued`. Every answer must agree.
use skyzfs_blk::model::{InFlight, Model as Head};
use skyzfs_blk::model_base::Model as Old;
use skyzfs_blk::*;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[derive(Clone)]
enum Kind {
    Mutation,
    Flush,
    Read { count: usize },
}

fn history(seed: u64, steps: usize, stats: &mut [u64; 8]) -> bool {
    let g = Geometry::new(1, 8, false).unwrap();
    let mut h = Head::new(g);
    let mut o = Old::new(g);
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut next = 0u64;
    let mut fill = 0u8;
    let mut flight: Vec<(RequestId, Kind)> = Vec::new();
    let mut failed = false;
    for step in 0..steps {
        let at = format!("seed {seed} step {step}");
        let mutations = flight.iter().filter(|f| matches!(f.1, Kind::Mutation)).count();
        let reads = flight.iter().filter(|f| matches!(f.1, Kind::Read { .. })).count();
        let roll = rng.below(20);
        let mut submit = |op: Op, kind: Kind, h: &mut Head, o: &mut Old, flight: &mut Vec<(RequestId, Kind)>| {
            next += 1;
            let request = Request { id: RequestId(next), op };
            let a = h.submit(&request).is_ok();
            let b = o.submit(&request).is_ok();
            assert_eq!(a, b, "submit, {at}");
            if a {
                flight.push((request.id, kind));
            }
        };
        match roll {
            0..=5 if mutations < 3 => {
                let block = rng.below(3) as u64;
                let count = 1 + rng.below(2);
                let op = match rng.below(8) {
                    0 => Op::WriteZeroes { offset: block * BLOCK_SIZE, len: count as u64 * BLOCK_SIZE },
                    1 => Op::Discard { offset: block * BLOCK_SIZE, len: count as u64 * BLOCK_SIZE },
                    _ => {
                        fill = fill.wrapping_add(1).max(1);
                        Op::Write { offset: block * BLOCK_SIZE, data: Buf::filled(count, fill) }
                    }
                };
                submit(op, Kind::Mutation, &mut h, &mut o, &mut flight);
            }
            6..=8 if reads < 2 => {
                let block = rng.below(3) as u64;
                let count = 1 + rng.below(2);
                submit(
                    Op::Read { offset: block * BLOCK_SIZE, buf: Buf::zeroed(count) },
                    Kind::Read { count },
                    &mut h, &mut o, &mut flight,
                );
            }
            9 => submit(Op::Flush, Kind::Flush, &mut h, &mut o, &mut flight),
            10..=15 if !flight.is_empty() => {
                let (id, kind) = flight.swap_remove(rng.below(flight.len()));
                match kind {
                    Kind::Read { count } => {
                        // Every answer of whole blocks of one fill each.
                        let fills: Vec<u8> = (0..=fill).collect();
                        let mut accepted = None;
                        let mut candidates = Vec::new();
                        if count == 1 {
                            for f in &fills { candidates.push(vec![*f]); }
                        } else {
                            for f in &fills { for g2 in &fills { candidates.push(vec![*f, *g2]); } }
                        }
                        for c in candidates {
                            let bytes: Vec<u8> = c.iter().flat_map(|f| vec![*f; BLOCK_USIZE]).collect();
                            let completion = Completion { id, result: Ok(()), buf: Buf::from_blocks(&bytes) };
                            let a = h.clone().complete(&completion).is_ok();
                            let b = o.clone().complete(&completion).is_ok();
                            assert_eq!(a, b, "read answer {c:?}, {at}");
                            stats[if a { 0 } else { 1 }] += 1;
                            if a && (accepted.is_none() || rng.below(3) == 0) {
                                accepted = Some(completion);
                            }
                        }
                        let completion = accepted.unwrap_or(Completion { id, result: Err(BlkError::Failed), buf: None });
                        let a = h.complete(&completion).is_ok();
                        let b = o.complete(&completion).is_ok();
                        assert_eq!(a, b, "read completion, {at}");
                    }
                    _ => {
                        for result in [Ok(()), Err(BlkError::Failed)] {
                            let completion = Completion { id, result, buf: None };
                            let a = h.clone().complete(&completion).is_ok();
                            let b = o.clone().complete(&completion).is_ok();
                            assert_eq!(a, b, "completion {result:?}, {at}");
                        }
                        let result = if failed { Err(BlkError::Failed) } else { Ok(()) };
                        let completion = Completion { id, result, buf: None };
                        let a = h.complete(&completion).is_ok();
                        let b = o.complete(&completion).is_ok();
                        assert_eq!(a, b, "completion, {at}");
                        stats[2] += 1;
                    }
                }
            }
            16 | 17 => {
                h.kill(if std::env::var("DIFF_WIDE").is_ok() { InFlight::MayHaveCompleted } else { InFlight::Reissued });
                o.kill();
                stats[3] += 1;
                if mutations > 0 { stats[4] += 1; }
            }
            18 if rng.below(4) == 0 => {
                if rng.below(3) == 0 {
                    h.log_failed();
                    o.log_failed();
                    failed = true;
                    stats[6] += 1;
                } else {
                    if failed { h.reopened(); o.reopened(); failed = false; } else { h.power_cut(); o.power_cut(); }
                    flight.clear();
                    stats[5] += 1;
                }
            }
            _ => {}
        }
        assert_eq!(h.worlds(), o.worlds(), "worlds, {at}");
        assert_eq!(h.in_flight(), o.in_flight(), "in flight, {at}");
        assert_eq!(h.readback(), o.readback(), "readback, {at}");
        stats[7] = stats[7].max(h.worlds() as u64);
        if h.worlds() > 400 {
            // Too many worlds to go on with in memory: end this history.
            return true;
        }
    }
    false
}

#[test]
fn reissued_is_the_base() {
    let n: u64 = std::env::var("DIFF_HISTORIES").ok().and_then(|s| s.parse().ok()).unwrap_or(2000);
    let mut stats = [0u64; 8];
    let mut cut = 0;
    for seed in 1..=n {
        if history(seed, 60, &mut stats) {
            cut += 1;
        }
    }
    println!(
        "histories {n} ({cut} ended early at more than 400 worlds); read answers compared: accepted {} refused {}; mutation and flush completions {}; kills {} (with a mutation in flight {}); power cuts and reopens {}; log failures {}; most worlds {}",
        stats[0], stats[1], stats[2], stats[3], stats[4], stats[5], stats[6], stats[7]
    );
}
