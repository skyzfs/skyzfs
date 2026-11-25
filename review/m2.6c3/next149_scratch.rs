//! Review scratch tests for #149's next look (head 0ad7944). Copied into
//! a scratch worktree as `crates/skyzfs-agent/src/engine/review_scratch.rs`
//! with `#[path = "review_scratch.rs"] mod review_scratch;` appended to
//! `crates/skyzfs-agent/src/engine/tests.rs` (so that it sees that
//! module's private helpers); never committed to the product. Run with
//! `cargo test -p skyzfs-agent --release --lib review_scratch --
//! --nocapture --test-threads=1`. Each test asserts what the reviewer
//! expects; a FAILED test is a finding (see next_look.md).

use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};

use skyzfs_control::{Code, CutsReply, Reply, Request, SnapReply, UnreadableReply, Unread};

use super::*;
use crate::plan::{self, Action, DaemonSeen, PoolSeen, Seen, SnapSeen};
use crate::record::{Count, Member, Person, Phase, Publish, Taken};

// ---------------------------------------------------------------------
// N1. B1's fix, searched: a person's give-up against every sequence of
// the daemon's answers, bounded.
// ---------------------------------------------------------------------

/// The kind of a question the agent asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Cuts,
    Progress,
    Withdraw,
    Look,
}

/// How many answers each kind of question can be given here.
fn arity(kind: Kind) -> u8 {
    match kind {
        Kind::Cuts => 2,
        Kind::Progress | Kind::Withdraw => 4,
        Kind::Look => 3,
    }
}

/// An answer: the fake's (truthful) or one of the daemon's refusals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ans {
    Truthful,
    /// No answer at all (`Err`): the daemon away.
    Away,
    /// `refused no-answer`, the question not passed on.
    NoAnswer,
    /// `refused no-answer` to a withdraw that the uploader still carries
    /// out (skyzfsd's withdraw sits in the uploader's inbox).
    NoAnswerApplied,
    Ended,
    NoUpload,
    Store,
}

fn answer_of(kind: Kind, c: u8) -> Ans {
    match (kind, c) {
        (_, 0) => Ans::Truthful,
        (Kind::Cuts, _) => Ans::Away,
        (Kind::Progress, 1) | (Kind::Withdraw, 1) | (Kind::Look, 1) => Ans::NoAnswer,
        (Kind::Progress, 2) | (Kind::Withdraw, 3) => Ans::Ended,
        (Kind::Progress, _) => Ans::NoUpload,
        (Kind::Withdraw, _) => Ans::NoAnswerApplied,
        (Kind::Look, _) => Ans::Store,
    }
}

/// What the daemon answers after the enumerated prefix, until `until`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tail {
    Truthful,
    /// skyzfsd's uploader in a long step: `progress` and `withdraw`
    /// answered `refused no-answer`, `cuts` and `look` answered.
    NoAnswer,
    /// `progress` and `withdraw` answered `refused ended`.
    Ended,
    /// skyzfsd's handler with no uploader: everything `refused no-upload`.
    NoUpload,
    /// No answer to anything.
    Away,
}

fn tail_answer(tail: Tail, kind: Kind) -> Ans {
    match (tail, kind) {
        (Tail::Truthful, _) => Ans::Truthful,
        (Tail::NoAnswer, Kind::Progress | Kind::Withdraw) => Ans::NoAnswer,
        (Tail::Ended, Kind::Progress | Kind::Withdraw) => Ans::Ended,
        (Tail::NoAnswer | Tail::Ended, _) => Ans::Truthful,
        (Tail::NoUpload, _) => Ans::NoUpload,
        (Tail::Away, _) => Ans::Away,
    }
}

struct Script {
    prefix: Vec<u8>,
    pos: usize,
    arities: Vec<u8>,
    tail: Tail,
    until: u64,
    /// A random answer at each question after the prefix where not 0.
    rng: u64,
    still: (u64, u32),
}

impl Script {
    fn choose(&mut self, kind: Kind, now: u64) -> Ans {
        if self.still.0 == now {
            self.still.1 += 1;
            assert!(
                self.still.1 < 20_000,
                "livelock: 20000 questions at {now} with no time passing"
            );
        } else {
            self.still = (now, 0);
        }
        let pos = self.pos;
        self.pos += 1;
        if self.arities.len() == pos {
            self.arities.push(arity(kind));
        }
        if let Some(&c) = self.prefix.get(pos) {
            return answer_of(kind, c % arity(kind));
        }
        if self.rng != 0 {
            // xorshift64
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            // Mostly the tail; a random answer one time in four.
            if self.rng % 4 == 0 {
                return answer_of(kind, ((self.rng >> 8) % u64::from(arity(kind))) as u8);
            }
        }
        if now >= self.until {
            Ans::Truthful
        } else {
            tail_answer(self.tail, kind)
        }
    }
}

struct Oracle {
    world: Rc<RefCell<World>>,
    script: Rc<RefCell<Script>>,
}

impl Daemon for Oracle {
    fn ask(&mut self, request: &Request, manifest: &[u8]) -> io::Result<Reply> {
        let kind = match request {
            Request::Cuts => Kind::Cuts,
            Request::Progress(_) => Kind::Progress,
            Request::Withdraw(_) => Kind::Withdraw,
            Request::Look(_) => Kind::Look,
            Request::Publish(_) => return FakeDaemon(self.world.clone()).ask(request, manifest),
        };
        let now = self.world.borrow().now_ms;
        let ans = self.script.borrow_mut().choose(kind, now);
        let refused = |code| Ok(Reply::Refused(code, String::new()));
        match ans {
            Ans::Truthful => FakeDaemon(self.world.clone()).ask(request, manifest),
            Ans::Away => Err(io::Error::new(io::ErrorKind::TimedOut, "away")),
            Ans::NoAnswer => refused(Code::NoAnswer),
            Ans::NoAnswerApplied => {
                let _ = FakeDaemon(self.world.clone()).ask(request, manifest);
                refused(Code::NoAnswer)
            }
            Ans::Ended => refused(Code::Ended),
            Ans::NoUpload => refused(Code::NoUpload),
            Ans::Store => refused(Code::Store),
        }
    }

    fn device(&mut self) -> io::Result<u64> {
        FakeDaemon(self.world.clone()).device()
    }
}

/// Where the first agent stops before the person's give-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Start {
    /// Just after `accepted` was recorded: the request pends.
    Accepted,
    /// Just after the send: the request accepted, not recorded (the
    /// record says asked, from `asking`).
    SentNotRecorded,
    /// Just after `asking` was recorded: nothing sent.
    RecordedNotSent,
    /// Just after `held` was recorded: nothing sent.
    Held,
}

fn stop_point(start: Start) -> Point {
    let (effect, after) = match start {
        Start::Accepted => (Effect::Asked, true),
        Start::SentNotRecorded => (Effect::Ask, true),
        Start::RecordedNotSent => (Effect::Asking, true),
        Start::Held => (Effect::Held, true),
    };
    Point { effect, after }
}

#[derive(Debug, Clone, Copy)]
struct Case {
    start: Start,
    mode: Mode,
    lag_ms: u64,
    tail: Tail,
    until_rel: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    Published,
    Abandoned,
}

fn panic_text(e: &Box<dyn std::any::Any + Send>) -> String {
    e.downcast_ref::<String>()
        .cloned()
        .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "a panic".into())
}

/// One run: the first agent stopped at `case.start`, a person's give-up
/// by a second agent whose daemon answers by `prefix`, then the tail,
/// and a person who destroys `pool@t` as soon as it is not held. Then
/// two hours for whatever pends to land. The verdict, or the violation.
fn run_case(case: &Case, prefix: &[u8], rng: u64) -> (Result<Verdict, String>, Vec<u8>) {
    let world = World::new(case.mode);
    world.borrow_mut().lag_ms = case.lag_ms;
    let keep = MemKeep::default();
    let stop = stop_point(case.start);
    let mut agent0 = agent(&world, &keep).with_points(Box::new(move |p: Point, _: &str| p == stop));
    let t = agent0.begin().expect("begun");
    match agent0.settle(LIMIT) {
        Err(Error::Stopped) => {}
        other => return (Err(format!("the first agent did not stop: {other:?}")), vec![]),
    }
    drop(agent0);
    let start_ms = world.borrow().now_ms;
    world.borrow_mut().destroyer = Some(t.clone());
    let script = Rc::new(RefCell::new(Script {
        prefix: prefix.to_vec(),
        pos: 0,
        arities: Vec::new(),
        tail: case.tail,
        until: start_ms.saturating_add(case.until_rel),
        rng,
        still: (0, 0),
    }));
    let oracle = Oracle {
        world: world.clone(),
        script: script.clone(),
    };
    let ran = catch_unwind(AssertUnwindSafe(|| {
        let mut again = Agent::new(
            FakeZfs(world.clone()),
            oracle,
            keep.clone(),
            FakeClock(world.clone()),
            VOLUME,
            POOL,
        )
        .expect("the agent");
        again.recover(&t, Choice::GiveUp).expect("recovered");
        let settled = again.settle(LIMIT).map_err(|e| e.to_string());
        (settled, again.outcomes().to_vec())
    }));
    let arities = script.borrow().arities.clone();
    let (settled, outcomes) = match ran {
        Ok(r) => r,
        Err(e) => return (Err(format!("panic: {}", panic_text(&e))), arities),
    };
    if let Err(e) = settled {
        return (Err(format!("not settled: {e}")), arities);
    }
    let verdict = match outcomes.as_slice() {
        [(n, Outcome::Published(_))] if *n == t => Verdict::Published,
        [(n, Outcome::Abandoned(Why::Person))] if *n == t => Verdict::Abandoned,
        other => return (Err(format!("outcomes {other:?}")), arities),
    };
    world.borrow_mut().advance(Duration::from_secs(2 * 3600));
    let w = world.borrow();
    if !holds(&w).is_empty() {
        return (Err(format!("holds left: {:?}", holds(&w))), arities);
    }
    if let Some(r) = w.refs.get(&t) {
        let manifest = Manifest::decode(&r.manifest).expect("the manifest");
        let cut = &w.commits[&r.commit].cut;
        for m in &manifest.members {
            let at = format!("{}@{t}", m.name);
            if cut.snaps.get(&at) != Some(&m.snapshot_guid) {
                return (
                    Err(format!(
                        "WRONG PUBLISH: snap/{t} names cut {} (group {}) without {at}; {verdict:?}",
                        cut.number, cut.group
                    )),
                    arities,
                );
            }
        }
        if verdict == Verdict::Abandoned {
            return (
                Err(format!("given up as the person's, and snap/{t} made afterwards (whole)")),
                arities,
            );
        }
    } else if verdict == Verdict::Published {
        return (Err(format!("published, and no snap/{t}")), arities);
    }
    (Ok(verdict), arities)
}

#[derive(Default)]
struct Stats {
    runs: u64,
    published: u64,
    abandoned: u64,
    wrong: Vec<String>,
    late: Vec<String>,
    other: Vec<String>,
    /// The runs given up then made, by tail and start.
    late_by: BTreeMap<String, usize>,
}

impl Stats {
    fn add(&mut self, case: &Case, prefix: &[u8], v: Result<Verdict, String>) {
        self.runs += 1;
        match v {
            Ok(Verdict::Published) => self.published += 1,
            Ok(Verdict::Abandoned) => self.abandoned += 1,
            Err(e) => {
                let line = format!("{case:?} prefix {prefix:?}: {e}");
                if e.starts_with("WRONG") {
                    self.wrong.push(line);
                } else if e.starts_with("given up") {
                    *self
                        .late_by
                        .entry(format!("{:?} {:?} {:?}", case.tail, case.start, case.mode))
                        .or_default() += 1;
                    self.late.push(line);
                } else {
                    self.other.push(line);
                }
            }
        }
    }
}

fn dfs(case: &Case, depth: usize, stats: &mut Stats) {
    let mut stack: Vec<Vec<u8>> = vec![Vec::new()];
    while let Some(prefix) = stack.pop() {
        let (v, arities) = run_case(case, &prefix, 0);
        stats.add(case, &prefix, v);
        if prefix.len() < depth {
            if let Some(&a) = arities.get(prefix.len()) {
                for c in 0..a {
                    let mut p = prefix.clone();
                    p.push(c);
                    stack.push(p);
                }
            }
        }
    }
}

const STARTS: [Start; 4] = [
    Start::Accepted,
    Start::SentNotRecorded,
    Start::RecordedNotSent,
    Start::Held,
];
const SEARCH_MODES: [Mode; 3] = [Mode::Busy, Mode::Idle, Mode::Abandoned];
const LAGS: [u64; 2] = [60_000, 365 * 24 * 3_600_000];
const UNTILS: [u64; 9] = [
    30_000,
    300_000,
    599_000,
    601_000,
    639_000,
    641_000,
    679_000,
    681_000,
    u64::MAX,
];

fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for start in STARTS {
        for mode in SEARCH_MODES {
            for lag_ms in LAGS {
                cases.push(Case {
                    start,
                    mode,
                    lag_ms,
                    tail: Tail::Truthful,
                    until_rel: 0,
                });
                for tail in [Tail::NoAnswer, Tail::Ended, Tail::NoUpload, Tail::Away] {
                    for until_rel in UNTILS {
                        cases.push(Case {
                            start,
                            mode,
                            lag_ms,
                            tail,
                            until_rel,
                        });
                    }
                }
            }
        }
    }
    cases
}

fn report(name: &str, stats: &Stats) {
    eprintln!(
        "{name}: {} runs: {} published, {} given up; {} wrong publishes, {} given up then made, {} other",
        stats.runs,
        stats.published,
        stats.abandoned,
        stats.wrong.len(),
        stats.late.len(),
        stats.other.len()
    );
    for (by, n) in &stats.late_by {
        eprintln!("  given up then made: {by}: {n}");
    }
    for line in stats.wrong.iter().chain(&stats.other).take(20) {
        eprintln!("  {line}");
    }
    for line in stats.late.iter().take(3) {
        eprintln!("  {line}");
    }
}

/// N1a. Every prefix of the first `DEPTH` answers (cuts: answered or
/// away; progress: answered, no-answer, ended, no-upload; withdraw:
/// answered, no-answer, no-answer and carried out, ended; look:
/// answered, no-answer, store), then each tail until each time.
#[test]
fn n1a_give_up_bounded_exhaustive() {
    let depth: usize = std::env::var("N1_DEPTH")
        .ok()
        .and_then(|d| d.parse().ok())
        .unwrap_or(4);
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let mut stats = Stats::default();
    let began = std::time::Instant::now();
    for case in cases() {
        dfs(&case, depth, &mut stats);
    }
    std::panic::set_hook(hook);
    report(
        &format!("N1a depth {depth} ({:.0} s)", began.elapsed().as_secs_f64()),
        &stats,
    );
    assert!(stats.wrong.is_empty() && stats.other.is_empty());
}

/// N1b. Random answers at every question (one in four), over the tails.
#[test]
fn n1b_give_up_random() {
    let n: u64 = std::env::var("N1_RANDOM")
        .ok()
        .and_then(|d| d.parse().ok())
        .unwrap_or(20);
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let mut stats = Stats::default();
    let began = std::time::Instant::now();
    for (i, case) in cases().into_iter().enumerate() {
        for s in 0..n {
            let seed = 0x9e37_79b9_7f4a_7c15_u64
                .wrapping_mul(i as u64 + 1)
                .wrapping_add(s.wrapping_mul(0xbf58_476d_1ce4_e5b9))
                | 1;
            let (v, _) = run_case(&case, &[], seed);
            stats.add(&case, &[seed as u8], v);
        }
    }
    std::panic::set_hook(hook);
    report(
        &format!("N1b {n} seeds a case ({:.0} s)", began.elapsed().as_secs_f64()),
        &stats,
    );
    assert!(stats.wrong.is_empty() && stats.other.is_empty());
}

// ---------------------------------------------------------------------
// N2. The planner, exhaustively: a person's give-up against the age
// bound's, for every phase, answer, sight, withdraw and time.
// ---------------------------------------------------------------------

fn taken() -> Taken {
    Taken {
        group: 40,
        members: vec![Member {
            name: POOL.into(),
            snapshot_guid: 7,
        }],
        manifest_len: 100,
        manifest_hash: [5; 32],
    }
}

const ASKED_MS: u64 = 1_000_000;
const UNTIL_MS: u64 = ASKED_MS + 640_000;

fn publish_in(phase: Phase, person: Option<Person>) -> Publish {
    Publish {
        name: "skz-t".into(),
        count: Count {
            attachment: [1; 16],
            state: [2; 16],
            after: 10,
            load_guid: 99,
        },
        phase,
        held: true,
        attempts: 1,
        stalls: 0,
        begun_ms: ASKED_MS,
        person,
    }
}

fn phases() -> Vec<(&'static str, Phase)> {
    vec![
        ("intent", Phase::Intent),
        ("taken", Phase::Taken(taken())),
        ("held", Phase::Held(taken())),
        (
            "asked",
            Phase::Asked {
                taken: taken(),
                asked_ms: ASKED_MS,
                held_until_ms: UNTIL_MS,
            },
        ),
    ]
}

const CODES: [Code; 17] = [
    Code::Name,
    Code::Attachment,
    Code::State,
    Code::Manifest,
    Code::Expiry,
    Code::Asked,
    Code::Busy,
    Code::Taken,
    Code::Stopped,
    Code::Rewound,
    Code::Version,
    Code::Malformed,
    Code::Peer,
    Code::NoUpload,
    Code::Ended,
    Code::NoAnswer,
    Code::Store,
];

fn replies() -> Vec<(String, Option<Reply>)> {
    let mut all: Vec<(String, Option<Reply>)> = vec![
        ("(away)".into(), None),
        (
            "cuts".into(),
            Some(Reply::Cuts(CutsReply {
                attachment: [1; 16],
                state: Some([2; 16]),
                count: 11,
                newest: Some((41, true)),
            })),
        ),
        ("accepted".into(), Some(Reply::Accepted)),
        ("pending".into(), Some(Reply::Pending)),
        ("done".into(), Some(Reply::Done([9; 32]))),
        ("expired".into(), Some(Reply::Expired)),
        ("withdrawn".into(), Some(Reply::Withdrawn)),
        ("unknown".into(), Some(Reply::Unknown)),
        ("absent".into(), Some(Reply::Absent)),
        (
            "snap".into(),
            Some(Reply::Snap(SnapReply {
                commit: [9; 32],
                state: [2; 16],
                number: 12,
                group: 41,
                manifest_len: 100,
                manifest_hash: [5; 32],
            })),
        ),
        (
            "unreadable".into(),
            Some(Reply::Unreadable(UnreadableReply {
                commit: [9; 32],
                why: Unread::NoRow,
            })),
        ),
    ];
    for code in CODES {
        all.push((
            format!("refused {}", code.word()),
            Some(Reply::Refused(code, String::new())),
        ));
    }
    all
}

fn seen_of(answer: Option<Reply>, snap: Option<SnapSeen>, withdrawn: bool, now_ms: u64, too_old: bool) -> Seen {
    Seen {
        pool: PoolSeen {
            load_guid: 99,
            premise: true,
            snapshot: Some(taken().members),
            held: true,
        },
        daemon: match answer {
            None => DaemonSeen::Away,
            Some(answer) => DaemonSeen::Here {
                cuts: CutsReply {
                    attachment: [1; 16],
                    state: Some([2; 16]),
                    count: 11,
                    newest: Some((41, true)),
                },
                answer,
            },
        },
        snap,
        now_ms,
        assisted_ms: None,
        assists: 0,
        too_old,
        withdrawn,
        assist_after: plan::ASSIST_AFTER,
    }
}

fn gives_up(a: &Action) -> bool {
    matches!(a, Action::Release | Action::Abandon(_))
}

/// N2. For a publish recorded asked, given up by a person, before
/// `held_until`, with no withdraw of its own answered: an answer that
/// may hide a request that can still land (the daemon away, `pending`,
/// `accepted`, `refused asked`, and the silent `no-answer`, `ended`,
/// `no-upload`) never gives the hold up, whatever `look` saw but ours.
/// Printed: every row where the person's give-up and the age bound's
/// differ.
#[test]
fn n2_planner_person_against_age_bound() {
    let mut bad = Vec::new();
    let mut differ = Vec::new();
    for (pname, phase) in phases() {
        for (aname, answer) in replies() {
            for snap in [
                None,
                Some(SnapSeen::Absent),
                Some(SnapSeen::Ours([9; 32])),
                Some(SnapSeen::NotOurs),
                Some(SnapSeen::Unreadable),
            ] {
                for withdrawn in [false, true] {
                    for now in [ASKED_MS + 60_000, UNTIL_MS] {
                        let person = plan::plan(
                            &publish_in(phase.clone(), Some(Person::GiveUp)),
                            &seen_of(answer.clone(), snap, withdrawn, now, false),
                        );
                        let aged = plan::plan(
                            &publish_in(phase.clone(), None),
                            &seen_of(answer.clone(), snap, withdrawn, now, true),
                        );
                        let before = now < UNTIL_MS;
                        let may_land = answer.as_ref().is_none_or(|a| {
                            matches!(
                                a,
                                Reply::Pending
                                    | Reply::Accepted
                                    | Reply::Refused(
                                        Code::Asked | Code::NoAnswer | Code::Ended | Code::NoUpload,
                                        _
                                    )
                            )
                        });
                        let row = format!(
                            "{pname:6} {aname:20} snap={snap:?} withdrawn={withdrawn} before_until={before}: person {person:?} | age bound {aged:?}"
                        );
                        if pname == "asked"
                            && before
                            && !withdrawn
                            && may_land
                            && !matches!(snap, Some(SnapSeen::Ours(_)))
                            && gives_up(&person)
                        {
                            bad.push(row.clone());
                        }
                        if matches!(pname, "held" | "asked")
                            && gives_up(&person) != gives_up(&aged)
                        {
                            differ.push(row);
                        }
                    }
                }
            }
        }
    }
    eprintln!("N2: rows where one gives the hold up and the other does not ({}):", differ.len());
    for row in &differ {
        eprintln!("  {row}");
    }
    eprintln!("N2: unsafe rows ({}):", bad.len());
    for row in &bad {
        eprintln!("  {row}");
    }
    assert!(bad.is_empty());
}

/// N2b. The engine's reading of an answer to `publish` (`may_pend`)
/// against the give-up's reading of the same code from `progress`.
#[test]
fn n2b_may_pend_against_silent() {
    for code in CODES {
        let answer = Reply::Refused(code, String::new());
        let pends = super::super::may_pend(&answer);
        let held = phases()[2].1.clone();
        let asked = phases()[3].1.clone();
        let gave_held = plan::plan(
            &publish_in(held, Some(Person::GiveUp)),
            &seen_of(Some(answer.clone()), Some(SnapSeen::Absent), false, ASKED_MS + 60_000, false),
        );
        let gave_asked = plan::plan(
            &publish_in(asked, Some(Person::GiveUp)),
            &seen_of(Some(answer.clone()), Some(SnapSeen::Absent), false, ASKED_MS + 60_000, false),
        );
        eprintln!(
            "N2b: {:10} to publish: {} | give-up from progress, asked: {gave_asked:?}; held: {gave_held:?}",
            code.word(),
            if pends { "stays asked" } else { "held again" }
        );
    }
}

// ---------------------------------------------------------------------
// N3. The stall test's window, measured.
// ---------------------------------------------------------------------

/// N3. For each stop of `a_stop_at_any_point_of_a_stalling_run_counts_each_stall_once`,
/// the extra stalls the run counts (`more`) and whether the stop is in
/// the window the test allows one in. Tight: one exactly in the window,
/// none outside it.
#[test]
fn n3_stall_window_measured() {
    let whole = stalling_run_stopped_at(usize::MAX).points;
    let window = |k: usize| {
        let at = |j: usize, effect: Effect, after: bool| whole.get(j) == Some(&Point { effect, after });
        let mut j = k;
        loop {
            if at(j, Effect::Asking, true) {
                return true;
            }
            let inside = at(j, Effect::Ask, false) || at(j, Effect::Ask, true) || at(j, Effect::Asked, false);
            if !inside || j == 0 {
                return false;
            }
            j -= 1;
        }
    };
    let assist = |k: usize| whole.get(k).is_some_and(|p| p.effect == Effect::Assist);
    let mut table: BTreeMap<(String, bool, u32), usize> = BTreeMap::new();
    let mut loose = Vec::new();
    let mut over = Vec::new();
    for k in (0..whole.len()).filter(|&k| !assist(k) || k < 2 || !assist(k - 2)) {
        let run = stalling_run_stopped_at(k);
        let more: u32 = run
            .asked
            .windows(2)
            .map(|w| w[1].saturating_sub(w[0] + 1))
            .sum();
        let inw = window(k);
        let p = whole[k];
        let word = format!("{}:{}", p.effect.word(), if p.after { "after" } else { "before" });
        *table.entry((word.clone(), inw, more)).or_default() += 1;
        if inw && more == 0 {
            loose.push(format!("{k} {word}"));
        }
        if more > u32::from(inw) {
            over.push(format!("{k} {word} more={more}"));
        }
    }
    eprintln!("N3: {} points; (point, in window, extra stalls) -> stops:", whole.len());
    for ((word, inw, more), n) in &table {
        if *inw || *more > 0 {
            eprintln!("  {word:16} window={inw} more={more}: {n}");
        }
    }
    let outside: usize = table.iter().filter(|((_, w, m), _)| !w && *m == 0).map(|(_, n)| n).sum();
    eprintln!("  outside the window with none extra: {outside}");
    eprintln!("N3: in the window with none extra (slack): {loose:?}");
    eprintln!("N3: over the allowance: {over:?}");
    assert!(over.is_empty());
    assert!(loose.is_empty(), "the window allows one where none is counted: {loose:?}");
}

// ---------------------------------------------------------------------
// N4. fresh_name's chains: renamed up to six times in one second, with
// and without a stop between, through `execute` and `settle`.
// ---------------------------------------------------------------------

/// A hook that takes, in `pool/db`, each new name a publish's take is
/// about to use, until `n` names are taken; and stops the agent at the
/// `stop_at`-th program run where given.
fn taker(
    world: &Rc<RefCell<World>>,
    taken: &Rc<RefCell<Vec<String>>>,
    n: usize,
    programs: &Rc<Cell<usize>>,
    stop_at: Option<usize>,
) -> Points {
    let (world, taken, programs) = (world.clone(), taken.clone(), programs.clone());
    Box::new(move |point: Point, at: &str| {
        if point.effect == Effect::Program && point.after {
            programs.set(programs.get() + 1);
            if Some(programs.get()) == stop_at {
                return true;
            }
        }
        let mut taken = taken.borrow_mut();
        if point.effect == Effect::Intent
            && point.after
            && !at.is_empty()
            && taken.len() < n
            && !taken.iter().any(|t| t == at)
        {
            taken.push(at.to_string());
            let mut world = world.borrow_mut();
            let pool = world.pool.as_mut().expect("the pool");
            let snap = super::super::fake::Snap {
                guid: 0xdead_0000 + taken.len() as u64,
                createtxg: pool.txg,
                holds: BTreeSet::new(),
                held_in: None,
            };
            pool.snaps.insert(format!("{POOL}/db@{at}"), snap);
        }
        false
    })
}

#[test]
fn n4_rename_chains_end_and_exit_by_their_outcome() {
    let mut wrong = Vec::new();
    for n in 1..=6 {
        // Through execute (`publish`), in one process.
        let world = World::new(Mode::Busy);
        let keep = MemKeep::default();
        let taken = Rc::new(RefCell::new(Vec::new()));
        let programs = Rc::new(Cell::new(0));
        let mut a = agent(&world, &keep).with_points(taker(&world, &taken, n, &programs, None));
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = crate::program::execute(&mut a, &crate::program::Task::Publish, LIMIT, &mut out, &mut err);
        let out = String::from_utf8_lossy(&out).into_owned();
        let made = out.strip_prefix("snap/").and_then(|o| o.split(' ').next()).unwrap_or("");
        eprintln!(
            "N4 execute n={n}: code {code}, programs {}, taken {:?}, made {made}",
            programs.get(),
            taken.borrow()
        );
        if code != 0 || made.is_empty() || taken.borrow().iter().any(|t| t == made) || taken.borrow().len() != n {
            wrong.push(format!("execute n={n}: code {code} out {out:?}"));
        }
        // With a stop at each program run in turn, and a new agent.
        for stop in 1..=n {
            let world = World::new(Mode::Busy);
            let keep = MemKeep::default();
            let taken = Rc::new(RefCell::new(Vec::new()));
            let programs = Rc::new(Cell::new(0));
            let mut a = agent(&world, &keep).with_points(taker(&world, &taken, n, &programs, Some(stop)));
            let t = a.begin().expect("begun");
            let first = a.settle(LIMIT);
            drop(a);
            let mut b = agent(&world, &keep).with_points(taker(&world, &taken, n, &programs, None));
            let ran = catch_unwind(AssertUnwindSafe(|| b.settle(LIMIT).map(|()| b.outcomes().to_vec())));
            let line = format!(
                "N4 stop n={n} at program {stop}: first {first:?}, programs {}, taken {:?}, then {:?}",
                programs.get(),
                taken.borrow(),
                ran.as_ref().map_err(panic_text)
            );
            eprintln!("{line}");
            match ran {
                Ok(Ok(outcomes)) => match outcomes.as_slice() {
                    [(name, Outcome::Published(_))]
                        if !taken.borrow().iter().any(|x| x == name) && *name != t || n == 0 => {}
                    _ => wrong.push(line),
                },
                _ => wrong.push(line),
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// N4b. `recover --retry` of a publish stopped at Intent whose name a
/// person then took in a member: it is renamed, and `recover` exits by
/// the renamed publish's outcome.
#[test]
fn n4b_recover_follows_a_rename() {
    let world = World::new(Mode::Busy);
    let keep = MemKeep::default();
    let stop = |p: Point, _: &str| p.effect == Effect::Program && !p.after;
    let mut a = agent(&world, &keep).with_points(Box::new(stop));
    let t = a.begin().expect("begun");
    assert!(matches!(a.settle(LIMIT), Err(Error::Stopped)));
    drop(a);
    {
        let mut w = world.borrow_mut();
        let pool = w.pool.as_mut().expect("pool");
        let snap = super::super::fake::Snap {
            guid: 0xbeef,
            createtxg: pool.txg,
            holds: BTreeSet::new(),
            held_in: None,
        };
        pool.snaps.insert(format!("{POOL}/db@{t}"), snap);
    }
    let mut b = agent(&world, &keep);
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = crate::program::execute(
        &mut b,
        &crate::program::Task::Recover(t.clone(), Choice::Retry),
        LIMIT,
        &mut out,
        &mut err,
    );
    let out = String::from_utf8_lossy(&out).into_owned();
    eprintln!("N4b: code {code} out {out:?} err {:?}", String::from_utf8_lossy(&err));
    assert_eq!(code, 0, "{out}");
    assert!(out.starts_with("snap/") && !out.contains(&format!("snap/{t} ")), "{out}");
}

// ---------------------------------------------------------------------
// N5. L1's `asking` record and the wall clock: an ask sent in this
// process whose answer is lost is held until `held_until` read on the
// wall clock, where a publish accepted in this process reads it on the
// clock that does not step (S4 of #142's review).
// ---------------------------------------------------------------------

/// A daemon that takes the first `publish` (the fake accepts it) and
/// loses its answer (`Err`, or `refused no-answer`), then is away for
/// `away_s`; as it loses the answer, the agent's wall clock steps
/// forward by `skew_ms`.
struct LostAnswer {
    world: Rc<RefCell<World>>,
    lost: bool,
    no_answer: bool,
    away_s: u64,
    skew_ms: i64,
}

impl Daemon for LostAnswer {
    fn ask(&mut self, request: &Request, manifest: &[u8]) -> io::Result<Reply> {
        let answer = FakeDaemon(self.world.clone()).ask(request, manifest);
        if matches!(request, Request::Publish(_)) && !self.lost {
            self.lost = true;
            let mut w = self.world.borrow_mut();
            w.away_until = w.now_ms + self.away_s * 1000;
            w.agent_skew_ms = self.skew_ms;
            w.log.push(format!("publish answered {answer:?}; the answer is lost"));
            return if self.no_answer {
                Ok(Reply::Refused(Code::NoAnswer, String::new()))
            } else {
                Err(io::Error::new(io::ErrorKind::ConnectionReset, "lost"))
            };
        }
        answer
    }

    fn device(&mut self) -> io::Result<u64> {
        FakeDaemon(self.world.clone()).device()
    }
}

/// Whether `snap/t` was made on a cut without `pool@t`, and the holds
/// and outcomes, after the run.
fn lost_answer_run(mode: Mode, no_answer: bool, skew_ms: i64, give_up: bool) -> (bool, String) {
    let world = World::new(mode);
    let keep = MemKeep::default();
    let daemon = LostAnswer {
        world: world.clone(),
        lost: false,
        no_answer,
        away_s: 900,
        skew_ms,
    };
    let mut a = Agent::new(
        FakeZfs(world.clone()),
        daemon,
        keep.clone(),
        FakeClock(world.clone()),
        VOLUME,
        POOL,
    )
    .expect("the agent");
    let t = a.begin().expect("begun");
    world.borrow_mut().destroyer = Some(t.clone());
    if give_up {
        // The person's give-up in the same run, once the answer is lost.
        let _ = a.settle(Duration::from_secs(5));
        a.recover(&t, Choice::GiveUp).expect("recovered");
    }
    let settled = a.settle(LIMIT);
    let outcomes = a.outcomes().to_vec();
    drop(a);
    world.borrow_mut().advance(Duration::from_secs(2 * 3600));
    let w = world.borrow();
    let wrong = w.refs.get(&t).is_some_and(|r| {
        !w.commits[&r.commit]
            .cut
            .snaps
            .contains_key(&format!("{POOL}@{t}"))
    });
    let line = format!(
        "{mode:?} no_answer={no_answer} skew={skew_ms} give_up={give_up}: settled {settled:?}, outcomes {outcomes:?}, snap/t {:?}, wrong {wrong}\n  log tail {:?}",
        w.refs.get(&t).map(|r| r.number),
        w.log.iter().rev().take(8).collect::<Vec<_>>()
    );
    (wrong, line)
}

/// N5. With no step of the wall clock, the hold is kept until the
/// request has expired: no wrong publish (the control). With a step
/// forward of two hours as the answer is lost, the hold must still be
/// kept (the request was accepted in this process): a wrong publish is
/// the finding.
#[test]
fn n5_an_unanswered_ask_and_a_step_of_the_wall_clock() {
    let mut wrong = Vec::new();
    for mode in [Mode::Idle, Mode::Abandoned, Mode::Busy] {
        for no_answer in [false, true] {
            for give_up in [false, true] {
                for skew in [0_i64, 2 * 3_600_000] {
                    let (w, line) = lost_answer_run(mode, no_answer, skew, give_up);
                    eprintln!("N5 {line}");
                    if w {
                        wrong.push(line);
                    }
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
