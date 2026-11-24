
/// Review's variant (next look at #134): B1's race at the first cut,
/// where there is no row. The first cut holds the whole volume, and with
/// no row it goes up only where head's commit has no group. Another
/// writer swaps head between `observe`'s read and `Engine::open`'s read.
/// Where its commit has a group, the cut must not go up onto it: the
/// check must take head's group from the commit the engine sits on.
/// Where its commit has none, the cut goes up onto it, by the rule.
#[tokio::test]
async fn review_variant_race_at_the_first_cut_with_no_row() {
    for (seed, group) in [(93, Some(5)), (94, None)] {
        let case = format!("the other writer's group {group:?}");
        let mut fx = Fixture::new(seed).await;
        let mut store = fx.start();
        fx.write(&mut store, 10, 0x10);
        fx.group(&mut store, 7);
        let before = fx.uploaded().await;
        assert!(before.row.is_none(), "{case}: no row yet");
        let refs = Swapping::new(&fx);
        refs.after_the_next_read(group);
        let remote =
            skyzfs_upload::EngineRemote::new(fx.store.clone(), refs.clone(), fx.vol, cfg());
        let mut up = Driver::with_slices(store, remote, 64);
        let steps = up.run().await.expect("cut 1");
        assert!(!refs.pending(), "{case}: the other writer swapped head");
        let after = fx.uploaded().await;
        match group {
            Some(_) => {
                assert_eq!(
                    told(&steps),
                    [Step::Mismatch {
                        why: Mismatch::HeadMoved,
                        abandoned: vec![1]
                    }],
                    "{case}: cut 1 is not uploaded onto a commit with a group"
                );
                assert_eq!(after.group, group, "{case}: head is the other writer's");
                assert!(after.row.is_none(), "{case}: still no row");
            }
            None => {
                assert!(
                    told(&steps)
                        .iter()
                        .any(|step| matches!(step, Step::Uploaded { number: 1, .. })),
                    "{case}: {steps:?}"
                );
                assert_eq!(after.group, Some(7), "{case}: cut 1 is head");
            }
        }
    }
}
