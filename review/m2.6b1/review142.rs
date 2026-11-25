// Scratch scenarios of the review of #142, included at the end of
// plan/tests.rs in a scratch worktree only. Each test asserts what the
// planner DOES at the head, so that it passes and documents the case.

mod review142 {
    use super::*;

    /// F1: a publish an earlier daemon made, with `pool@t` gone since (a
    /// `-T` rewind past G), answered `unknown` by the next daemon: given
    /// up as destroyed without a look, even when a look has seen ours.
    #[test]
    fn review_f1_unknown_with_the_snapshot_gone_gives_up_without_a_look() {
        let ours = || {
            seen()
                .answer(Reply::Unknown)
                .gone()
                .looked(SnapSeen::Ours(COMMIT))
        };
        assert_eq!(
            plan(&publish(asked()), &ours()),
            Action::Abandon(Why::Destroyed)
        );
        assert_eq!(
            plan(&publish(held()), &ours()),
            Action::Abandon(Why::Destroyed)
        );
        // Not looked at all: the planner does not ask for one.
        assert_eq!(
            plan(&publish(asked()), &seen().answer(Reply::Unknown).gone()),
            Action::Abandon(Why::Destroyed)
        );
        // `refused rewound` the same, with the reason Rewound.
        let rewound = || {
            seen()
                .refused(Code::Rewound)
                .gone()
                .looked(SnapSeen::Ours(COMMIT))
        };
        assert_eq!(
            plan(&publish(asked()), &rewound()),
            Action::Abandon(Why::Rewound)
        );
        // A member replaced (renamed) or the premise refused: the same.
        assert_eq!(
            plan(
                &publish(asked()),
                &seen()
                    .answer(Reply::Unknown)
                    .no_premise()
                    .unheld()
                    .looked(SnapSeen::Ours(COMMIT))
            ),
            Action::Abandon(Why::Premise)
        );
        // By contrast, `expired` with the snapshot gone looks first and
        // records ours done.
        assert_eq!(
            plan(
                &publish(asked()),
                &seen()
                    .answer(Reply::Expired)
                    .gone()
                    .looked(SnapSeen::Ours(COMMIT))
            ),
            Action::Done(COMMIT)
        );
    }

    /// Codex P2: the newest cut qualifies, so no sync is run, however long
    /// the request pends. The planner cannot tell a cut abandoned from one
    /// uploading.
    #[test]
    fn review_codex_a_qualifying_newest_cut_stops_every_assist() {
        for minutes in [1, 5, 9] {
            let at = ASKED + minutes * 60 * SECOND;
            assert_eq!(
                plan(&publish(asked()), &seen().at(at).cuts(carrying())),
                Action::Wait,
                "{minutes} min"
            );
        }
    }

    /// S: at the intent the premise is not read: a pool refused (a
    /// checkpoint, a mirror) still has its snapshot taken, then given up
    /// at Taken, keeping `pool@t`.
    #[test]
    fn review_s_intent_takes_whatever_the_premise() {
        let none = || {
            seen()
                .with(|s| s.pool.snapshot = None)
                .unheld()
                .no_premise()
        };
        assert_eq!(plan(&publish(intent()), &none()), Action::Take);
        assert_eq!(
            plan(&publish(taken_phase()), &seen().no_premise().unheld()),
            Action::Abandon(Why::Premise)
        );
    }

    /// S: `done` read with the snapshot gone and `load_guid` moved (a rewind
    /// the daemon's rule did not see, e.g. to a checkpoint while the agent
    /// was down) is recorded done, with nothing reported.
    #[test]
    fn review_s_done_after_a_move_with_the_snapshot_gone_is_done() {
        let s = seen()
            .answer(Reply::Done(COMMIT))
            .gone()
            .moved()
            .looked(SnapSeen::Ours(COMMIT));
        assert_eq!(plan(&publish(asked()), &s), Action::Done(COMMIT));
    }

    /// S: a member renamed (same GUIDs) between the snapshot and the
    /// publish gives the publish up as replaced.
    #[test]
    fn review_s_a_rename_gives_the_publish_up() {
        let mut renamed = members();
        renamed[1].name = "tank/db2".into();
        assert_eq!(
            plan(&publish(asked()), &seen().members(renamed.clone())),
            Action::Withdraw
        );
        assert_eq!(
            plan(&publish(taken_phase()), &seen().members(renamed).unheld()),
            Action::Abandon(Why::Replaced)
        );
    }

    /// Reading 1: a row of our manifest is ours whatever its cut's state
    /// and number, and whatever the count recorded now; a row of another
    /// manifest is not, even with our count's cut.
    #[test]
    fn review_reading_1_the_manifest_decides() {
        let t = taken();
        let mut row = row(G);
        row.state = [0x77; 16];
        row.number = 0;
        assert_eq!(
            snap_seen(&Reply::Snap(row), &t),
            Some(SnapSeen::Ours(COMMIT))
        );
        let mut other = super::row(G);
        other.manifest_hash = [0x4e; 32];
        assert_eq!(snap_seen(&Reply::Snap(other), &t), Some(SnapSeen::NotOurs));
    }
}
