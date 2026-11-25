// Scratch scenarios of the next look at #142 (head b79083a), included at
// the end of plan/tests.rs in a scratch worktree only (beside
// review142.rs). Each test asserts what the planner DOES at the head, so
// that it passes and documents the case. The "engine" here is a loop that
// records `assisted_ms` at the time of every `Assist`, as the doc comment
// of `assist_due` says the engine does.

mod review142b {
    use super::*;

    /// Drive the planner every `step` ms from `from` to `to` with the
    /// publish pending, `carried(now)` saying whether the newest cut
    /// qualifies, and an engine that records each sync. Returns the times
    /// of the syncs.
    fn drive(
        from: u64,
        to: u64,
        step: u64,
        assisted: Option<u64>,
        carried: impl Fn(u64) -> bool,
    ) -> Vec<u64> {
        let mut assisted_ms = assisted;
        let mut syncs = Vec::new();
        let mut now = from;
        while now <= to {
            let cuts = if carried(now) { carrying() } else { cuts() };
            let mut s = seen().at(now).cuts(cuts);
            s.assisted_ms = assisted_ms;
            match plan(&publish(asked()), &s) {
                Action::Assist => {
                    syncs.push(now);
                    assisted_ms = Some(now);
                }
                Action::Wait => {}
                other => panic!("at {now}: {other:?}"),
            }
            now += step;
        }
        syncs
    }

    fn gaps(syncs: &[u64]) -> Vec<u64> {
        syncs.windows(2).map(|w| w[1] - w[0]).collect()
    }

    /// A cut carries the request for its whole life (an upload that takes
    /// the full default expiry, or a cut abandoned on an idle pool): the
    /// first sync at exactly twice ASSIST_AFTER, then one every
    /// ASSIST_AFTER, 59 in ten minutes. No storm, none before 2x.
    #[test]
    fn review_n1_carried_for_the_whole_expiry() {
        let end = ASKED + EXPIRY_DEFAULT.as_millis() as u64;
        let syncs = drive(ASKED, end, 100, None, |_| true);
        assert_eq!(syncs.first(), Some(&(ASKED + 20 * SECOND)));
        assert!(gaps(&syncs).iter().all(|&g| g == 10 * SECOND), "{:?}", gaps(&syncs));
        assert_eq!(syncs.len(), 59);
        // Polled every ms around the boundary: the same.
        let fine = drive(ASKED + 19_990, ASKED + 40_010, 1, None, |_| true);
        assert_eq!(fine, vec![ASKED + 20_000, ASKED + 30_000, ASKED + 40_000]);
    }

    /// No cut qualifies for the whole life: from ASSIST_AFTER, one a
    /// second (the rule as before; the lead's Q8).
    #[test]
    fn review_n2_never_carried_is_one_a_second() {
        let end = ASKED + EXPIRY_DEFAULT.as_millis() as u64;
        let syncs = drive(ASKED, end, 100, None, |_| false);
        assert_eq!(syncs.first(), Some(&(ASKED + 10 * SECOND)));
        assert!(gaps(&syncs).iter().all(|&g| g == SECOND));
        assert_eq!(syncs.len(), 591);
    }

    /// The usual course on an idle pool: no cut qualifies, the sync at
    /// ASSIST_AFTER makes one that does (and that carries it from then):
    /// the next sync, if the publish has not landed, at twice ASSIST_AFTER.
    #[test]
    fn review_n3_a_sync_that_makes_the_carrying_cut() {
        let first = ASKED + 10 * SECOND;
        let syncs = drive(ASKED, ASKED + 60 * SECOND, 100, None, |now| now > first);
        assert_eq!(syncs[..3], [first, ASKED + 20 * SECOND, ASKED + 30 * SECOND]);
    }

    /// A cut that stops qualifying (another cut state, a cut not opening
    /// as itself): back to one a second at once, counted from the last.
    #[test]
    fn review_n4_a_cut_that_stops_qualifying() {
        let stop = ASKED + 25 * SECOND;
        let syncs = drive(ASKED, ASKED + 30 * SECOND, 100, None, |now| now < stop);
        assert_eq!(
            syncs,
            vec![
                ASKED + 20 * SECOND,
                stop,
                stop + SECOND,
                stop + 2 * SECOND,
                stop + 3 * SECOND,
                stop + 4 * SECOND,
                stop + 5 * SECOND
            ]
        );
    }

    /// `qualifies` flipping at every look (it cannot in one daemon, the
    /// newest cut only moves on): the pace of the rule without a cut,
    /// never faster.
    #[test]
    fn review_n5_flip_flop_is_no_faster_than_one_a_second() {
        let syncs = drive(ASKED, ASKED + 60 * SECOND, 100, None, |now| (now / 100) % 2 == 0);
        assert!(gaps(&syncs).iter().all(|&g| g >= SECOND), "{:?}", gaps(&syncs));
        assert!(syncs[0] >= ASKED + 10 * SECOND);
    }

    /// The last sync was before this `accepted` (of an earlier request of
    /// the same publish, before an AskAnew: `Seen::assisted_ms` is per
    /// publish): the rule waits for twice ASSIST_AFTER after `accepted`.
    /// Pinned by no row of the table (the plant A5 survives).
    #[test]
    fn review_n6_a_sync_of_an_earlier_request() {
        let before = ASKED - 9 * SECOND;
        let carried = |at: u64| seen().at(at).cuts(carrying()).assisted(before);
        let p = publish(asked());
        assert_eq!(plan(&p, &carried(ASKED + SECOND)), Action::Wait);
        assert_eq!(plan(&p, &carried(ASKED + 15 * SECOND)), Action::Wait);
        assert_eq!(plan(&p, &carried(ASKED + 20 * SECOND)), Action::Assist);
    }

    /// The wall clock stepped back an hour after a sync (S4): no sync is
    /// due, with or without a carrying cut, until the clock has passed the
    /// recorded sync; the request expires first (its expiry is the
    /// daemon's), and a request asked anew inherits the same wait if the
    /// engine keeps `assisted_ms` across it.
    #[test]
    fn review_n7_a_clock_stepped_back() {
        let later = ASKED + 3_600 * SECOND;
        let p = publish(asked());
        for cuts in [cuts(), carrying()] {
            for at in [ASKED + 30 * SECOND, ASKED + 599 * SECOND] {
                assert_eq!(
                    plan(&p, &seen().at(at).cuts(cuts).assisted(later)),
                    Action::Wait
                );
            }
        }
    }

    /// The wall clock stepped forward a day: one sync, then the pace again.
    #[test]
    fn review_n8_a_clock_stepped_forward() {
        let day = ASKED + 86_400 * SECOND;
        let syncs = drive(day, day + 30 * SECOND, 100, Some(ASKED + 20 * SECOND), |_| true);
        assert_eq!(syncs, vec![day, day + 10 * SECOND, day + 20 * SECOND, day + 30 * SECOND]);
    }

    /// The ends of u64: nothing panics (no subtraction, saturating adds),
    /// and a time at the top is never due early.
    #[test]
    fn review_n9_extremes_do_not_panic() {
        for asked_ms in [0, 1, u64::MAX - 20_000, u64::MAX - 1, u64::MAX] {
            for now in [0, asked_ms, asked_ms.saturating_add(20_000), u64::MAX] {
                for assisted in [None, Some(0), Some(now), Some(u64::MAX)] {
                    for c in [cuts(), carrying()] {
                        let phase = Phase::Asked {
                            taken: taken(),
                            asked_ms,
                            held_until_ms: held_until(asked_ms, EXPIRY_DEFAULT),
                        };
                        let mut s = seen().at(now).cuts(c);
                        s.assisted_ms = assisted;
                        let _ = plan(&publish(phase), &s);
                    }
                }
            }
        }
        // Accepted at the top: not due until now reaches the top.
        let phase = Phase::Asked {
            taken: taken(),
            asked_ms: u64::MAX - 1,
            held_until_ms: u64::MAX,
        };
        let s = seen().at(u64::MAX - 1).cuts(carrying());
        assert_eq!(plan(&publish(phase), &s), Action::Wait);
    }

    /// No sync where the request is not pending or not this agent's to
    /// help: held and not asked (asked again), `done` (looked), an end
    /// (looked), the snapshot not whole (withdrawn), the daemon away.
    #[test]
    fn review_n10_no_sync_outside_pending() {
        let late = ASKED + 100 * SECOND;
        let c = || seen().at(late).cuts(carrying());
        assert_eq!(plan(&publish(held()), &c()), Action::Ask);
        assert_eq!(plan(&publish(asked()), &c().answer(Reply::Done(COMMIT))), Action::Look);
        assert_eq!(plan(&publish(asked()), &c().answer(Reply::Expired)), Action::Look);
        assert_eq!(plan(&publish(asked()), &c().no_premise()), Action::Withdraw);
        assert_eq!(plan(&publish(asked()), &c().moved()), Action::Withdraw);
        assert_eq!(plan(&publish(asked()), &c().away()), Action::Wait);
        assert_eq!(plan(&publish(asked()), &c().refused(Code::Busy)), Action::Wait);
    }

    /// The held_until row (2): past held_until with the daemon here and
    /// the request pending, the hold is kept whether or not a sync is due:
    /// with one due the planner syncs, never releases.
    #[test]
    fn review_n11_past_held_until_here_keeps_the_hold_either_way() {
        let p = publish(asked());
        let base = || seen().at(UNTIL + 1);
        assert_eq!(plan(&p, &base().cuts(carrying())), Action::Assist);
        assert_eq!(plan(&p, &base().cuts(carrying()).assisted(UNTIL)), Action::Wait);
        assert_eq!(plan(&p, &base()), Action::Assist);
        assert_eq!(plan(&p, &base().assisted(UNTIL + 1)), Action::Wait);
        assert_eq!(plan(&p, &base().at(UNTIL + 100 * SECOND).cuts(carrying())), Action::Assist);
    }
}
