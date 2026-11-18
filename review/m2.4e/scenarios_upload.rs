
// ---- review of #125 (M2.4e): a scenario the tests lack ----
//
// Appended to crates/skyzfs-upload/tests/holes.rs for a run.

/// U1. The first cut, whole, goes up onto another writer's commit that
/// holds 0xAB in every block of the volume, not only in block 0: every
/// block of every run of zeros, in every slice, must be written over.
/// Blocks written on the image are spread so that the slices (8 blocks of
/// buffer) name several runs each, of several blocks, and some slices
/// name runs only.
#[tokio::test]
async fn review_m24e_u1_a_whole_cut_writes_every_hole_over_old_bytes_everywhere() {
    let mut fx = Fixture::sparse(41).await;
    {
        let inner = fx.store.inner().clone();
        let refs = skynbd_refs::ObjectRefStore::new(inner.clone());
        let mut engine = skynbd_engine::Engine::open(inner, refs, fx.vol, cfg())
            .await
            .expect("the engine");
        engine
            .write(0, &vec![0xAB; SIZE as usize])
            .expect("a write");
        engine
            .commit(skynbd_format::CommitKind::Flush)
            .await
            .expect("another writer's commit");
    }
    let mut store = fx.start();
    for (block, byte) in [(18, 0x12), (20, 0x14), (37, 0x25), (38, 0x26), (90, 0x5a), (200, 0xc8)] {
        fx.write(&mut store, block, byte);
    }
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 8);
    let steps = up.run().await.expect("the upload");
    assert!(
        steps.contains(&Step::Began {
            number: 1,
            rule: Rule::Whole
        }),
        "{steps:?}"
    );
    fx.assert_uploaded(7, "onto old bytes in every block").await;
}
