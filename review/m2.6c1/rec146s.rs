// The livelock search of the next look at #146 (heads 0213a6c and, as a
// control, 4ff4549). Appended to `engine/tests.rs` in a scratch worktree
// by `search146.py`; it uses only what both heads' fakes have.
//
// The engine runs on the fake's pool, clock and `cuts`, with a scripted
// daemon for `publish`, `progress`, `withdraw` and `look`, as skyzfsd's
// `Publishing` answers them (`crates/skyzfs-upload/src/publish.rs`):
//
// - `publish`: an identical request that pends is accepted again, another
//   refused `asked`; otherwise the script's first answer for the first
//   ask of the name, its later answer for each after. `accepted` makes
//   the request pend and forgets the name's last outcome; a refusal is not
//   kept (as skyzfsd's door refusals, `stopped` among them).
// - The request accepted ends as the script's fate at the first
//   `progress` or `withdraw` after it (it ended between two rounds), or
//   pends for ever (fate `pending`).
// - `progress`: `pending` while it pends, else the kept outcome, else
//   `unknown`. `withdraw`: a request that pends ends `withdrawn`; then as
//   `progress`.
// - `look`: the script's answer (absent, this publish's, another's,
//   unreadable, refused `store`, or no answer).
//
// For every script, one publish (and two for a subset) is begun and
// settled, too young (AGE_MAX) for 90 s of the world, or with an age
// bound of 20 s for 120 s. A livelock is 300 points of the engine with no
// time passing (the fake's own check, 2000 questions, is caught too).

use std::collections::BTreeMap as R7Map;

/// An answer of the script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum R7 {
    Accepted,
    Refused(skyzfs_control::Code),
    NoAnswer,
    Pending,
    Done,
    Expired,
    Withdrawn,
    Unknown,
}

/// What the script's `look` answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum R7Look {
    Absent,
    Ours,
    NotOurs,
    Unreadable,
    Store,
    NoAnswer,
}

const R7_COMMIT: [u8; 32] = [0xc0; 32];

const R7_CODES: [skyzfs_control::Code; 17] = {
    use skyzfs_control::Code::*;
    [
        Name, Attachment, State, Manifest, Expiry, Asked, Busy, Taken, Stopped, Rewound, Version,
        Malformed, Peer, NoUpload, Ended, NoAnswer, Store,
    ]
};

#[derive(Debug, Clone, Copy)]
struct R7Script {
    first: R7,
    later: R7,
    fate: R7,
    look: R7Look,
}

#[derive(Debug, Default)]
struct R7Name {
    asks: u32,
    pends: Option<Request>,
    fate_due: bool,
    outcome: Option<R7>,
    manifest: Vec<u8>,
    group: u64,
}

struct R7Daemon {
    fake: FakeDaemon,
    script: R7Script,
    names: Rc<RefCell<R7Map<String, R7Name>>>,
}

fn r7_reply(a: R7) -> io::Result<Reply> {
    match a {
        R7::Accepted => Ok(Reply::Accepted),
        R7::Refused(code) => Ok(Reply::Refused(code, String::new())),
        R7::NoAnswer => Err(io::Error::other("no answer")),
        R7::Pending => Ok(Reply::Pending),
        R7::Done => Ok(Reply::Done(R7_COMMIT)),
        R7::Expired => Ok(Reply::Expired),
        R7::Withdrawn => Ok(Reply::Withdrawn),
        R7::Unknown => Ok(Reply::Unknown),
    }
}

impl R7Daemon {
    fn fate(&self, n: &mut R7Name) {
        if n.fate_due {
            n.fate_due = false;
            if self.script.fate != R7::Pending {
                n.pends = None;
                n.outcome = Some(self.script.fate);
            }
        }
    }

    fn progress(n: &R7Name) -> io::Result<Reply> {
        if n.pends.is_some() {
            return Ok(Reply::Pending);
        }
        r7_reply(n.outcome.unwrap_or(R7::Unknown))
    }
}

impl Daemon for R7Daemon {
    fn ask(&mut self, request: &Request, manifest: &[u8]) -> io::Result<Reply> {
        let mut names = self.names.borrow_mut();
        match request {
            Request::Cuts => {
                drop(names);
                self.fake.ask(request, manifest)
            }
            Request::Publish(p) => {
                let n = names.entry(p.name.clone()).or_default();
                n.manifest = manifest.to_vec();
                n.group = p.group;
                if let Some(pending) = &n.pends {
                    return if pending == request {
                        Ok(Reply::Accepted)
                    } else {
                        Ok(Reply::Refused(skyzfs_control::Code::Asked, String::new()))
                    };
                }
                let a = if n.asks == 0 {
                    self.script.first
                } else {
                    self.script.later
                };
                n.asks += 1;
                if a == R7::Accepted {
                    n.pends = Some(request.clone());
                    n.fate_due = true;
                    n.outcome = None;
                }
                r7_reply(a)
            }
            Request::Progress(name) => {
                let n = names.entry(name.clone()).or_default();
                self.fate(n);
                Self::progress(n)
            }
            Request::Withdraw(name) => {
                let n = names.entry(name.clone()).or_default();
                self.fate(n);
                if n.pends.take().is_some() {
                    n.outcome = Some(R7::Withdrawn);
                }
                Self::progress(n)
            }
            Request::Look(name) => {
                let n = names.entry(name.clone()).or_default();
                let row = |hash: [u8; 32]| {
                    Reply::Snap(skyzfs_control::SnapReply {
                        commit: R7_COMMIT,
                        state: [0; 16],
                        number: 1,
                        group: n.group,
                        manifest_len: u32::try_from(n.manifest.len()).unwrap_or(u32::MAX),
                        manifest_hash: hash,
                    })
                };
                match self.script.look {
                    _ if n.manifest.is_empty() => Ok(Reply::Absent),
                    R7Look::Absent => Ok(Reply::Absent),
                    R7Look::Ours => Ok(row(crate::manifest::hash(&n.manifest))),
                    R7Look::NotOurs => Ok(row([0xee; 32])),
                    R7Look::Unreadable => Ok(Reply::Unreadable(skyzfs_control::UnreadableReply {
                        commit: R7_COMMIT,
                        why: skyzfs_control::Unread::NoRow,
                    })),
                    R7Look::Store => Ok(Reply::Refused(skyzfs_control::Code::Store, String::new())),
                    R7Look::NoAnswer => Err(io::Error::other("no answer")),
                }
            }
        }
    }

    fn device(&mut self) -> io::Result<u64> {
        self.fake.device()
    }
}

/// How a run of the search ended.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum R7End {
    Livelock(String),
    Ended(String),
    Unsettled,
    Error(String),
}

fn r7_run(script: R7Script, old: bool, publishes: usize) -> R7End {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let keep = MemKeep::default();
    let names = Rc::new(RefCell::new(R7Map::new()));
    let daemon = R7Daemon {
        fake: FakeDaemon(world.clone()),
        script,
        names,
    };
    let (age_max, limit) = if old {
        (Duration::from_secs(20), Duration::from_secs(120))
    } else {
        (plan::AGE_MAX, Duration::from_secs(90))
    };
    let still = Rc::new(Cell::new((0_u64, 0_u32)));
    let trail = Rc::new(RefCell::new(Vec::<Effect>::new()));
    let looped = Rc::new(Cell::new(false));
    let hook = {
        let (world, still, trail, looped) =
            (world.clone(), still.clone(), trail.clone(), looped.clone());
        move |point: Point| {
            let now = world.borrow().now_ms;
            let (at, n) = still.get();
            let n = if at == now { n + 1 } else { 0 };
            still.set((now, n));
            if !point.after {
                let mut t = trail.borrow_mut();
                t.push(point.effect);
                if t.len() > 24 {
                    t.remove(0);
                }
            }
            if n >= 300 {
                looped.set(true);
                return true;
            }
            false
        }
    };
    let mut agent = Agent::new(
        FakeZfs(world.clone()),
        daemon,
        keep.clone(),
        FakeClock(world.clone()),
        VOLUME,
        POOL,
    )
    .expect("the agent")
    .with_age_max(age_max)
    .with_points(Box::new(hook));
    for _ in 0..publishes {
        if let Err(e) = agent.begin() {
            return R7End::Error(format!("begin: {e}"));
        }
    }
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| agent.settle(limit)));
    let trail = || format!("{:?}", trail.borrow());
    match ran {
        Err(_) => R7End::Livelock(format!("the fake's check; {}", trail())),
        Ok(_) if looped.get() => R7End::Livelock(trail()),
        Ok(Ok(())) => R7End::Ended(format!("{:?}", {
            let mut o: Vec<String> = agent.outcomes().iter().map(|(_, o)| format!("{o:?}")).collect();
            o.sort();
            o
        })),
        Ok(Err(Error::Unsettled(_))) => R7End::Unsettled,
        Ok(Err(e)) => R7End::Error(e.to_string()),
    }
}

#[test]
fn review7_livelock_search() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let mut doors = vec![R7::Accepted, R7::NoAnswer];
    doors.extend(R7_CODES.iter().map(|&c| R7::Refused(c)));
    let mut fates = vec![
        R7::Pending,
        R7::Done,
        R7::Expired,
        R7::Withdrawn,
        R7::Unknown,
    ];
    fates.extend(R7_CODES.iter().map(|&c| R7::Refused(c)));
    let looks = [
        R7Look::Absent,
        R7Look::Ours,
        R7Look::NotOurs,
        R7Look::Unreadable,
        R7Look::Store,
        R7Look::NoAnswer,
    ];
    let only = std::env::var("R7_PUBLISHES").ok();
    let counts: Vec<usize> = match only.as_deref() {
        Some("2") => vec![2],
        Some("1") => vec![1],
        _ => vec![1, 2],
    };
    let mut runs = 0_u32;
    let mut tally: R7Map<String, u32> = R7Map::new();
    let mut livelocks = Vec::new();
    for &publishes in &counts {
        for old in [false, true] {
            for &look in &looks {
                for &first in &doors {
                    for &later in &doors {
                        for &fate in &fates {
                            // The fate matters only where an ask is accepted.
                            let accepted = first == R7::Accepted || later == R7::Accepted;
                            if !accepted && fate != R7::Pending {
                                continue;
                            }
                            // Two publishes: the scripts that ask, refused or not.
                            if publishes == 2
                                && !(matches!(first, R7::Accepted | R7::Refused(skyzfs_control::Code::Stopped))
                                    && matches!(later, R7::Accepted | R7::Refused(skyzfs_control::Code::Stopped)))
                            {
                                continue;
                            }
                            let script = R7Script {
                                first,
                                later,
                                fate,
                                look,
                            };
                            runs += 1;
                            let end = r7_run(script, old, publishes);
                            let key = match &end {
                                R7End::Livelock(_) => "livelock".to_string(),
                                R7End::Ended(o) => format!("ended {o}"),
                                R7End::Unsettled => "unsettled (waits, with ticks)".to_string(),
                                R7End::Error(e) => format!("error {e}"),
                            };
                            *tally.entry(key).or_default() += 1;
                            if let R7End::Livelock(trail) = end {
                                livelocks.push(format!(
                                    "LIVELOCK publishes {publishes} old {old} {script:?}: {trail}"
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    std::panic::set_hook(prev);
    println!("R7 runs {runs}");
    for (k, v) in &tally {
        println!("R7 {v:6} {k}");
    }
    println!("R7 livelocks {}", livelocks.len());
    for l in &livelocks {
        println!("{l}");
    }
}
