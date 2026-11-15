## Next look at #109, head 944986c

**Verdict: can merge once CI on 944986c is green. Nothing is open from the review.**

- **G1 is closed.** `a_store_given_no_region_keeps_the_map_in_regions_of_the_default` opens a store with `intent_region: None` and holds its region size and its region count to the constant. With my plant P1 (the store takes 64 MiB whatever the constant says) it fails, and nothing else does. The three plants now each fail one test (`review/m2.3-default/plants_944986c.out`): the store to the constant, the constant to 4, the help to the constant.
- **The comment** now says the 1 MiB cost was the map's syncs and its page writes.
- **Nothing else changed** since 618f536. The tests of `skyzfs-local` and `skyzfsd` with `test-hooks` pass here: 255 and 99.
- **CI on 944986c** had only begun when I wrote this: the daemon rig, the replay jobs, the cut rig through `skyzfsd` and the unit jobs were queued or running. They are the first rig runs at 4 MiB regions, so their tables are worth the lead's look. Codex has no findings on this head.
