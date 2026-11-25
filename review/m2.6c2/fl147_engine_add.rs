
// ---- Fourth look at #147 (scratch, not part of the pull request) ----

/// The world's hold on `t`: (members tagged, members).
fn fl147_hold_state(world: &World, t: &str) -> (usize, usize) {
    let Some(pool) = &world.pool else {
        return (0, 0);
    };
    let suffix = format!("@{t}");
    let all: Vec<_> = pool
        .snaps
        .iter()
        .filter(|(n, _)| n.ends_with(&suffix))
        .collect();
    (
        all.iter().filter(|(_, s)| s.holds.contains(HOLD_TAG)).count(),
        all.len(),
    )
}

struct Fl147 {
    ran: Ran,
    viol: Vec<String>,
    /// The world's hold at each of the agent's releases (before it).
    rel: Vec<(usize, usize)>,
    /// The publish was asked while the world's hold was not whole.
    asked_part: usize,
}

/// One publish in `mode`; a person's `user` at point `k`; with
/// `destroyer`, a person destroys `pool@t -r` as soon as nothing is held,
/// from the first time the hold is whole. Strict check: at each of the
/// agent's releases, no request of the publish pends at the fake daemon
/// (read from its own state, not through the channel).
fn fl147_run(mode: Mode, k: usize, user: User, destroyer: bool) -> Fl147 {
    let world = World::new(mode);
    let keep = MemKeep::default();
    let count = Rc::new(Cell::new(0usize));
    let viol = Rc::new(RefCell::new(Vec::new()));
    let rel = Rc::new(RefCell::new(Vec::new()));
    let asked_part = Rc::new(Cell::new(0usize));
    let gone = Rc::new(RefCell::new(BTreeSet::new()));
    let hook = {
        let (count, world, viol, rel, gone, asked_part) = (
            count.clone(),
            world.clone(),
            viol.clone(),
            rel.clone(),
            gone.clone(),
            asked_part.clone(),
        );
        move |point: Point| {
            let at = count.get();
            count.set(at + 1);
            note_gone(&world, &gone, point);
            let mut w = world.borrow_mut();
            let t = w.programs.keys().last().cloned().unwrap_or_default();
            let (tagged, total) = fl147_hold_state(&w, &t);
            if destroyer && !t.is_empty() && w.destroyer.is_none() && total > 0 && tagged == total {
                w.destroyer = Some(t.clone());
            }
            if point
                == (Point {
                    effect: Effect::Release,
                    after: false,
                })
            {
                let now = w.now_ms;
                if w.daemon.fl147_pends(&t, now) {
                    viol.borrow_mut()
                        .push(format!("released at point {at} while a request pends"));
                }
                rel.borrow_mut().push((tagged, total));
            }
            if point
                == (Point {
                    effect: Effect::Ask,
                    after: false,
                })
                && tagged != total
            {
                asked_part.set(asked_part.get() + 1);
            }
            if at == k {
                w.user(user, &t);
            }
            false
        }
    };
    let mut agent = agent(&world, &keep).with_points(Box::new(hook));
    let settled = agent
        .begin()
        .and_then(|_| agent.settle(LIMIT))
        .map_err(|e| e.to_string());
    let cuts_read = world.borrow().cuts_read;
    let ran = Ran {
        world: world.clone(),
        files: keep.0.clone(),
        settled,
        outcomes: agent.outcomes().to_vec(),
        reports: agent.reports().to_vec(),
        points: count.get(),
        cuts_read,
        stopped_at: None,
        gone_at_done: gone.take(),
    };
    let viol = viol.borrow().clone();
    let rel = rel.borrow().clone();
    Fl147 {
        ran,
        viol,
        rel,
        asked_part: asked_part.get(),
    }
}

/// The matrix of a partial release, tabulated, with the strict release
/// check and a stricter oracle: `TooOld` only where the hold was a part
/// at the agent's last release, and a `Refused` report only `rewound` in
/// ErrLow.
#[test]
fn fl147_partial_release_tabulated() {
    use std::collections::BTreeMap;
    for destroyer in [false, true] {
        for user in [User::ReleaseChild, User::DestroyChild, User::Destroy, User::Retake] {
            let mut table: BTreeMap<String, usize> = BTreeMap::new();
            let mut fails = Vec::new();
            for mode in MODES {
                let points = run(mode, Fault::None).points;
                for k in 0..points {
                    let fl = fl147_run(mode, k, user, destroyer);
                    let world = fl.ran.world.borrow();
                    let t = world.programs.keys().last().cloned().unwrap_or_default();
                    let partly = world.log.contains(&format!("partly released {t}"));
                    drop(world);
                    *table.entry(format!("{mode:?} runs")).or_default() += 1;
                    if partly {
                        *table.entry(format!("{mode:?} partly released")).or_default() += 1;
                    }
                    for (_, o) in &fl.ran.outcomes {
                        *table
                            .entry(format!("{mode:?} outcome {o:?}").replace(|c: char| c == '[' || c == ']', ""))
                            .or_default() += 1;
                    }
                    for (_, r) in &fl.ran.reports {
                        *table.entry(format!("{mode:?} report {r:?}")).or_default() += 1;
                    }
                    if fl.asked_part > 0 {
                        *table.entry(format!("{mode:?} asked with a part")).or_default() += 1;
                    }
                    let author = check(&fl.ran);
                    let mut stricter = Vec::new();
                    for (_, o) in &fl.ran.outcomes {
                        if *o == Outcome::Abandoned(Why::TooOld) {
                            let part_at_last = fl
                                .rel
                                .last()
                                .is_some_and(|(tagged, total)| *tagged > 0 && tagged < total);
                            if !part_at_last {
                                stricter.push(format!("TooOld, the hold at the releases {:?}", fl.rel));
                            }
                        }
                    }
                    for (_, r) in &fl.ran.reports {
                        if let Report::Refused(code) = r {
                            if !(mode == Mode::ErrLow && *code == skyzfs_control::Code::Rewound) {
                                stricter.push(format!("{r:?} in {mode:?}"));
                            }
                            if !fl.ran.outcomes.iter().any(|(_, o)| *o == Outcome::Abandoned(Why::TooOld)) {
                                stricter.push(format!("{r:?} not given up too old"));
                            }
                        }
                    }
                    if let Err(why) = &author {
                        *table.entry(format!("{mode:?} author's oracle FAILS")).or_default() += 1;
                        fails.push(format!("{mode:?} k={k}: author's oracle: {why}"));
                    }
                    if !stricter.is_empty() {
                        *table.entry(format!("{mode:?} stricter oracle FAILS")).or_default() += 1;
                        fails.push(format!("{mode:?} k={k}: stricter: {}", stricter.join("; ")));
                    }
                    if !fl.viol.is_empty() {
                        *table.entry(format!("{mode:?} RELEASE WHILE PENDING")).or_default() += 1;
                        fails.push(format!("{mode:?} k={k}: {}", fl.viol.join("; ")));
                    }
                }
            }
            println!("FL147 {user:?} destroyer={destroyer}");
            for (key, n) in &table {
                println!("FL147   {key}: {n}");
            }
            for f in fails.iter().take(12) {
                println!("FL147   ! {f}");
            }
            println!("FL147   failures: {}", fails.len());
        }
    }
}

/// A partial release at point `k1`, then the agent stops at point `k2`
/// (after it) and a new agent recovers, with or without a destroyer of
/// `pool@t` once nothing is held: the strict release check at both
/// agents' releases, and the author's oracle.
#[test]
fn fl147_partial_release_then_a_stop() {
    let mut runs = 0usize;
    let mut fails = Vec::new();
    let mut too_old = 0usize;
    let mut published = 0usize;
    for destroyer in [false, true] {
        for mode in MODES {
            let points = run(mode, Fault::None).points;
            for k1 in 0..points {
                for k2 in (k1 + 1)..(points + 2) {
                    runs += 1;
                    let world = World::new(mode);
                    let keep = MemKeep::default();
                    let count = Rc::new(Cell::new(0usize));
                    let viol = Rc::new(RefCell::new(Vec::new()));
                    let gone = Rc::new(RefCell::new(BTreeSet::new()));
                    let stopped = Rc::new(Cell::new(None));
                    let make_hook = |second: bool| {
                        let (count, world, viol, gone, stopped) = (
                            count.clone(),
                            world.clone(),
                            viol.clone(),
                            gone.clone(),
                            stopped.clone(),
                        );
                        move |point: Point| {
                            note_gone(&world, &gone, point);
                            let mut w = world.borrow_mut();
                            let t = w.programs.keys().last().cloned().unwrap_or_default();
                            let (tagged, total) = fl147_hold_state(&w, &t);
                            if destroyer && !t.is_empty() && w.destroyer.is_none() && total > 0 && tagged == total {
                                w.destroyer = Some(t.clone());
                            }
                            if point
                                == (Point {
                                    effect: Effect::Release,
                                    after: false,
                                })
                                && w.daemon.fl147_pends(&t, w.now_ms)
                            {
                                viol.borrow_mut().push(format!("released while a request pends (second agent {second})"));
                            }
                            if second {
                                return false;
                            }
                            let at = count.get();
                            count.set(at + 1);
                            if at == k1 {
                                w.user(User::ReleaseChild, &t);
                            }
                            if at == k2 {
                                stopped.set(Some(point));
                                return true;
                            }
                            false
                        }
                    };
                    let mut first = agent(&world, &keep).with_points(Box::new(make_hook(false)));
                    let mut outcomes = Vec::new();
                    let mut reports = Vec::new();
                    let settled = first.begin().and_then(|_| first.settle(LIMIT));
                    outcomes.extend(first.outcomes().iter().cloned());
                    reports.extend(first.reports().iter().cloned());
                    drop(first);
                    let settled = match settled {
                        Err(Error::Stopped) => {
                            world.borrow_mut().restart(Restart::None, None);
                            world.borrow_mut().advance(DOWN);
                            let mut again = agent(&world, &keep).with_points(Box::new(make_hook(true)));
                            let s = again.settle(LIMIT);
                            outcomes.extend(again.outcomes().iter().cloned());
                            reports.extend(again.reports().iter().cloned());
                            s
                        }
                        other => other,
                    };
                    let cuts_read = world.borrow().cuts_read;
                    let ran = Ran {
                        world: world.clone(),
                        files: keep.0.clone(),
                        settled: settled.map_err(|e| e.to_string()),
                        outcomes,
                        reports,
                        points: count.get(),
                        cuts_read,
                        stopped_at: stopped.get(),
                        gone_at_done: gone.take(),
                    };
                    for (_, o) in &ran.outcomes {
                        match o {
                            Outcome::Abandoned(Why::TooOld) => too_old += 1,
                            Outcome::Published(_) => published += 1,
                            _ => {}
                        }
                    }
                    if let Err(why) = check(&ran) {
                        fails.push(format!("{mode:?} destroyer={destroyer} k1={k1} k2={k2}: {why}"));
                    }
                    for v in viol.borrow().iter() {
                        fails.push(format!("{mode:?} destroyer={destroyer} k1={k1} k2={k2}: {v}"));
                    }
                }
            }
        }
    }
    println!("FL147 partial release then a stop: {runs} runs, {published} published, {too_old} given up too old, {} failures", fails.len());
    for f in fails.iter().take(12) {
        println!("FL147   ! {f}");
    }
}
