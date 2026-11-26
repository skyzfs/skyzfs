//! [`Zfs`] on the `zfs` and `zpool` commands ([`ZfsCli`]), run on a
//! [`Host`] with a time limit each (the plan of M2.6, §8, package c).
//!
//! # What each call runs
//!
//! - [`Zfs::layout`]: `zpool status -P POOL` and `zpool status -P -g
//!   POOL`. The first gives each vdev's name (a leaf's path, an interior
//!   vdev's kind) and the pool's scan and removal lines, the second each
//!   vdev's GUID in the same places; the two trees must have the same
//!   shape. A leaf's device number is read by `stat` of its path. A leaf
//!   that names no path, or whose path cannot be read, has none
//!   ([`Vdev::rdev`] `None`), which the premise refuses.
//! - [`Zfs::props`]: `zpool get -H -p -o property,value
//!   guid,load_guid,multihost,readonly,checkpoint POOL`. ZFS 2.2's
//!   `zpool get` says `missing pool name.` (exit 2) of a pool that does
//!   not exist, as of no pool named at all; then `zpool status -P POOL`
//!   is asked, and its `no such pool` gives [`ZfsError::NoPool`].
//! - [`Zfs::nocacheflush`]: the module parameter's file,
//!   [`NOCACHEFLUSH`].
//! - [`Zfs::snapshot`]: `zfs snapshot -r POOL@T` ("The take", below).
//! - [`Zfs::members`]: a read-only channel program (`zfs program -n
//!   -j`), `members.lua`, beside this file, on its standard input.
//! - [`Zfs::tagged`]: `zfs list -H -p -r -t snapshot -o name,userrefs
//!   POOL`, then `zfs holds -H` on the snapshots it lists with a hold, at
//!   most [`HOLDS_AT_ONCE`] and [`HOLDS_ARG_BYTES`] of names a command.
//!   No channel program: a read-only program may hold up the pool's
//!   sync for as long as it runs (#155), and this read walks every
//!   snapshot in the pool (S1 of #161's review; the lead's ruling,
//!   M2.6c-5d).
//! - [`Zfs::datasets`]: `zfs list` and twice `zfs get` ("The manifest's
//!   read", below).
//! - [`Zfs::destroy`]: `zfs destroy -r POOL@T`.
//! - [`Zfs::held`]: `zfs holds -H -r POOL@T`, and the members program
//!   where `POOL@T` has the tag.
//! - [`Zfs::hold`] and [`Zfs::release`]: `zfs hold -r` and `zfs release
//!   -r`. A hold that ZFS refuses because a member has the tag already
//!   held nothing (ZFS holds all or none): the members are read with the
//!   members program and their holds with `zfs holds -H -r pool@t`, and
//!   it is held only where every member's `@t` has the tag (F3 of the
//!   review of #147). A release that ZFS refuses for want of the tag is
//!   checked with `zfs holds -H -r pool@t`: no member may keep the tag.
//!   A message is matched on the command's standard error, on the words
//!   after a line's last `: `, so that a dataset's name is never read as
//!   one (S1).
//! - [`Zfs::assist`]: `zfs set skyzfs:assist=V POOL`, then `zpool sync
//!   POOL`.
//!
//! Every command runs in the locale `C`, so that its messages can be
//! read; `no such pool` gives [`ZfsError::NoPool`], `dataset does not
//! exist` [`ZfsError::NoSnapshot`] where a snapshot was named, and
//! anything else [`ZfsError::Other`] with the command and its message.
//! A command that says its pool is not there gives
//! [`ZfsError::NoPool`], with its words, only where `zpool list -H -o
//! name` does not list the pool the command acts on (that pool alone,
//! not any word of the command). Where it lists it, libzfs
//! said `no such pool or dataset` of a dataset or snapshot destroyed
//! as the command read it (testing's agent rig, `zfs holds -r` of a
//! `@t` being destroyed): [`ZfsError::Vanished`], which the reads of
//! [`Zfs::members`], [`Zfs::held`] and [`Zfs::tagged`] make again, at
//! most [`VANISHED_READS`] times in all; a read that goes on vanishing
//! is that error. Where `zpool list` fails, an error that says so: a
//! pool is never taken for gone on a message alone.
//! `zfs hold`, `holds` and `release` say `dataset does not exist` of a
//! pool that does not exist: they give [`ZfsError::NoSnapshot`], not
//! [`ZfsError::NoPool`] (recorded on both images).
//! A command past its limit is killed and not waited for ([`Host`]), and
//! gives [`ZfsError::Other`].
//!
//! # The take
//!
//! `zfs snapshot -r pool@t` lists the pool's file systems and volumes,
//! then asks for every `@t` in one ioctl, which ZFS does in one sync
//! task: all are taken, in one transaction group, or none is (design
//! §6.2; the lead's decision on Q10). A member's `@t` that exists fails
//! it as a whole: [`ZfsError::Exists`], read from `dataset already
//! exists`. Any other failure took nothing either, and is
//! [`ZfsError::Other`].
//!
//! **A take killed at its limit is not cancelled.** [`Limits::change`]
//! kills the command, not the ioctl it is waiting in: the sync task ends
//! in its own time, and takes every `@t` or none. The engine's next round
//! then finds `pool@t`, or its next take finds `EEXIST` at the name its
//! record holds, and adopts it after the checks ([`super::taken`]; the
//! lead's rule for M2.6c-2b), since the agent's names are its own and
//! one agent holds the pool's lock.
//!
//! The programs' values are all strings (`tostring`), and its tables
//! are keyed by name: `zfs program -j` writes a Lua number as a signed
//! 64-bit one (a GUID above 2^63 negative) and a list as an object keyed
//! by position, so names and strings are what reads the same on every
//! version. A GUID is read as unsigned, or as signed and taken modulo
//! 2^64.
//!
//! Output that cannot be read after the program exited 0 (cut off, not
//! UTF-8, or not a whole result) is an error: the programs only read.
//!
//! # The manifest's read
//!
//! The manifest is read before the take and after it ([`Zfs::datasets`];
//! the lead's ruling on M2.6c-5), by commands that only read and never
//! by a program over the whole pool, which holds the pool's sync for its
//! whole run (N6 of #153's review):
//!
//! 1. `zfs list -r -H -p -t filesystem,volume,snapshot -o
//!    name,guid,type,createtxg POOL`: each file system's and volume's
//!    type, GUID and `createtxg`, and those of its `@t` where it has one
//!    (`parse_listed`).
//! 2. `zfs get -r -H -p -s local -t filesystem,volume -o name,property
//!    all POOL`: the properties set locally, by dataset (`parse_pairs`).
//! 3. The same with `-o name,property,value`: their values, each line
//!    split at its first two tabs only, since a value may hold a tab
//!    (`match_values`).
//!
//! A value may hold a newline too, which `zfs get -H` prints as it is.
//! The values are taken only where the lines of (3) name exactly the
//! pairs of (2), in their order: a newline adds a line. Otherwise the
//! datasets whose lines cannot be trusted (`match_values`) are read
//! again one value at a time, `zfs get -H -p -o value PROPERTY NAME`,
//! which prints the value and one newline: that newline alone is taken
//! off. A value that still cannot be read (no newline at its end, longer
//! than [`VALUE_MOST`], or the command fails) fails the read: the
//! engine reports the error, which names the dataset and the property
//! ([`crate::plan::Report::Unreadable`]), and reads the pool again at a
//! later round, until the publish is too old (the lead's ruling on
//! M2.6c-5).
//!
//! - Names (of datasets and of properties) can hold neither a tab nor a
//!   newline, by ZFS's rules; values can hold any byte but NUL.
//! - Every command runs in the locale `C`. `zfs get -H` prints a value's
//!   bytes as they are, unlike `zfs program -j`.
//! - The properties are the datasets' as they are at the read: ZFS keeps
//!   on a snapshot only the properties set on the snapshot itself
//!   (`dsl_prop_get_ds` reads any other through the dataset's directory).
//!   Only those set locally are carried, not received ones.
//! - `skyzfs:assist`, the agent's own ([`ASSIST_PROPERTY`]), is left out,
//!   and so are `filesystem_count` and `snapshot_count`, which libzfs
//!   calls local though they are counters, and which no read alone
//!   fetches (`COUNTERS`).
//! - A dataset that (2) or (3) names and (1) did not list was created
//!   after the list: it is no member of the take, and is left out. The
//!   engine's check reads the pool again after its take, and a pool that
//!   changed between the two reads is taken again.
//! - (2), (3) and a value read alone are three reads, and the rule
//!   above is exact only where nothing changes between them. A property
//!   set or inherited between them can make a line of (3), or a value
//!   read alone, wrong, and a value set between (2) and (3) whose
//!   newlines make lines of other datasets can give a dataset a value
//!   nobody set (N2 of #157's review). That needs write access to the
//!   property it forges, and a change between the gets: the engine's
//!   check sees it where the read before the take and the read after
//!   it then differ, which is where the change does not land between
//!   the gets of both reads alike. A value of `-` read alone may be a
//!   property inherited meanwhile, and is found the same way.
//! - A pool that does not exist: `zfs list` and `zfs get` say `dataset
//!   does not exist` of the pool's name, which is [`ZfsError::NoPool`]
//!   here where `zpool list` agrees (above); the same message of another
//!   name is an error.
//!
//! # What was checked on ZFS, and what was not
//!
//! The parsers are tested on output written by hand from OpenZFS's
//! sources (`tests.rs`) and on output recorded by testing's recorder on
//! ZFS 2.2 userland over a 2.4.1 module (Ubuntu 24.04) and on 2.4.1
//! (26.04), `cli/recorded/` (`recordings.rs`; the plan's Q10). The
//! recordings show:
//!
//! - `zfs program` reads a script named `-` from its standard input, and
//!   writes a returned GUID above 2^63 as a negative number.
//! - The members program runs and returns what the parser reads, on the
//!   rig's pool and on a mirror of two files.
//! - The channel-program sandbox has no `pcall` (a take program of
//!   `366b217`, since dropped, failed on it on both images).
//! - The count of the datasets (`zfs list -r -H -o name -t
//!   filesystem,volume`) reads what the parser reads, on both pools and
//!   both images (`recorded/full-<image>.json`), and a pool that does not
//!   exist.
//! - The manifest's read ("The manifest's read"): testing's scale run
//!   45733395008 (Ubuntu 26.04) shows that `zfs get -H` and `zfs list`
//!   do not hold the pool's sync: 707 groups were written during the get
//!   at 10 000 datasets.
//! - A checkpoint is none where `zpool get -p` gives `-`; one that exists
//!   gives its size.
//! - A resilver under way says `resilver in progress` on its `scan:`
//!   line; a scrub under way, paused or cancelled, and a resilver that
//!   has ended, do not make the pool busy.
//! - A log on its own line reads `logs` with a tab after it.
//! - `zfs hold -r` of a tag one member has already fails with `tag
//!   already exists` and holds nothing; `zfs release -r` of a tag no
//!   member has fails with `no such tag`.
//! - A pool that does not exist, for each command (above): `zpool` and
//!   `zfs program` say `no such pool`, `zfs hold`, `holds`, `release`
//!   and `destroy -r` say `dataset does not exist`; `zfs snapshot -r`
//!   says it too, with its usage, and exits 2: [`ZfsError::Other`], which
//!   the engine tries again at its next round, where it reads no pool.
//! - `zfs snapshot -r` (testing's #152): taken, exit 0 with no output,
//!   even with `snapshot_limit=0` set on a member (as root). A member's
//!   `@t` that exists fails it with one `dataset already exists` line for
//!   each such member, in an order that varies, then `no snapshots were
//!   created`; nothing was taken.
//! - `zfs destroy -r` of a snapshot that is not there says `could not
//!   find any snapshots to destroy`. A member's `@t` with a hold fails it
//!   and destroys none; ZFS 2.2's userland says `dataset is busy` of it,
//!   and 2.4.1's `it's being held`.
//! - `zfs hold -r` where the root's `@t` alone has the tag fails with
//!   `tag already exists`, and `zfs holds -r` then lists the root's alone
//!   (TL-T2 of #147's review); `zfs release -r` then releases it.
//!
//! **Readings not checked on ZFS:**
//!
//! - "Set locally" is a property whose source, as `zfs.get_prop` and the
//!   user-property list give it, is the member itself. A received value
//!   is not local here.
//! - The programs' limits are the module's defaults for the most a
//!   program may ask: 100 000 000 instructions and 100 MiB of memory
//!   ([`PROGRAM_INSTRUCTIONS`], [`PROGRAM_MEMORY`]). A pool with more
//!   datasets than that allows fails the program, which only reads.
//! - `zfs snapshot -r`'s messages: `dataset already exists` for a member's
//!   `@t` that exists.
//! - A checkpoint is none where `zpool get -p` gives `0`.
//! - A rebuild under way: the `scan:` line names a rebuild, or a
//!   resilver with its vdev in brackets. A removal: any `remove:` line,
//!   under way or done, which the premise refuses (the lead's L4 on
//!   #147); one under way is busy too. `zpool status` never prints an
//!   indirect vdev.
//! - The manifest's read by `zfs list` and `zfs get` ("The manifest's
//!   read"), recorded on Ubuntu 24.04 and 26.04 (testing's run
//!   45738224324, #156; `recorded/read-*.json`): the list, the values
//!   pass and each odd value read alone, on two pools that hold values
//!   with a tab, a newline, a forged line, raw bytes and the longest
//!   value ZFS sets ([`VALUE_MOST`]; one byte more is refused), and a
//!   snapshot limit, under which `filesystem_count` and `snapshot_count`
//!   are listed as local; and again with the pairs pass, in four states
//!   of one pool and with a pool that does not exist (run 45751625028,
//!   #158; `recorded/cli-read-*.json`). `zfs get -H` writes a value's bytes as they
//!   are, with a tab between the columns and a newline after each line.
//! - The time limits ([`Limits`]), all provisional (the lead's rules on
//!   M2.6c-2b and on N2 of #153's review):
//!   - each of the manifest's two gets ([`Zfs::datasets`]): 60 s plus
//!     200 ms for each of the pool's datasets as last counted, at most 35
//!     minutes; a value read alone is a read as below;
//!   - the manifest's list, which lists every snapshot as well: the
//!     same, plus [`Limits::list_per_snapshot`], 10 ms, for each snapshot
//!     counted, at most 35 minutes (R1 of #157's review and the lead's
//!     ruling on M2.6c-5d);
//!   - `zfs holds -r` and the sweep's list of snapshots with their holds
//!     ([`Zfs::tagged`]), which walk every snapshot in the pool: a read's
//!     limit (below), plus for each snapshot counted 10 ms
//!     ([`Limits::holds_per_snapshot`] for `holds -r`,
//!     [`Limits::list_per_snapshot`] for the list), at most 35 minutes
//!     (R1-reach of #160's review, the lead's ruling (b)); a read's
//!     limit where the snapshots were not counted. Each of the sweep's
//!     `zfs holds -H` takes a read's limit;
//!   - the 10 ms a snapshot is about five times the slowest seen, on an
//!     idle pool in testing's recorder (run 45800940382, one run on each
//!     image): on Ubuntu 24.04 at 101 000 snapshots, `holds -r` 1.72 ms
//!     a snapshot, the snapshots' list 1.84 ms and their count 1.25 ms;
//!     on 26.04, `holds -r` 0.37 ms and the list 0.41 ms (the lead's
//!     ruling on M2.6c-5d);
//!   - every other read: 60 s, and a change 120 s, plus 100 ms for each
//!     dataset counted, at most 20 minutes;
//!   - the count itself 60 s; a count that fails leaves the bases. After
//!     a dataset count, `zfs list -r -H -o name -t snapshot` counts the
//!     snapshots, within 35 minutes: it took 1.25 ms a snapshot on
//!     Ubuntu 24.04 (126.2 s at 101 000), past a read's limit on the
//!     datasets, and a limit that rests on the count it makes would cut
//!     it off (the lead's ruling on M2.6c-5d). Where it fails, the list
//!     takes the gets' limit and the other walks a read's: the
//!     conservative reading (the design is silent), which gives no longer
//!     a limit than before on a pool whose commands fail. A walk that
//!     then fails says that the snapshots were not counted, and why.
//!
//!   They come from testing's recordings on Ubuntu 26.04 (run
//!   45720225770, one run) at 101, 1 001 and 10 001 datasets: the
//!   members program in full (which the manifest's read replaced) 8.7,
//!   88.2 and 911.8 s (about 90 ms a dataset, linear), so its 200 ms is
//!   about twice that; the members program plain 0.34,
//!   2.2 and 21.5 s; the count 0.23, 1.7 and 16.4 s; `zfs snapshot -r`
//!   2.2, 4.4 and 46.3 s. In #152, at 10 001 file systems on a slow
//!   runner, `zfs snapshot -r` took 391 s under a heavy write load, and
//!   the sweep's channel program then (`tagged.lua`, since replaced by
//!   the CLI) 68.7 s with none. In testing's scale run 45733395008
//!   (26.04, under a write load) at 10 000 datasets, `zfs get -H` took
//!   468.2 s and `zfs list` 33.8 s, so the manifest's read about 970 s,
//!   less than one of its reads' limits (2060 s). Correctness does not
//!   rest on them: a take killed at its limit is adopted at a later round
//!   (above).
//! - **What a publish costs.** A publish reads the manifest twice
//!   (before its take and after) and takes the pool once: at 10 000
//!   datasets about 2 × 970 s and 172 s in testing's scale run. Each
//!   round's count, members and `holds -r` reads, the hold, the release
//!   and the sweep add some 4 to 5 minutes at 10 000 (review of #153).
//!   Each round's count also lists the snapshots' names once (R1 of
//!   #157's review), and the manifest's list, `holds -r` and the sweep's
//!   list walk every snapshot: about 0.4 ms a snapshot each in testing's
//!   recorder (41.2 s for the manifest's list at 101 000 snapshots, run
//!   45781769557; `holds -r` 37.3 s there, about 0.37 ms, run
//!   45800940382). The sweep's `zfs holds -H` reads only the snapshots
//!   that have a hold.
//!   The design's scale is tens to hundreds of datasets.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use super::{Dataset, Held, Layout, Props, Snap, Vdev, VdevKind, Zfs, ZfsError, ASSIST_PROPERTY};
use crate::host::{Host, Node, Output};
use crate::manifest::Kind;
use crate::record::Member;

/// The module parameter that says whether ZFS sends cache flushes.
pub const NOCACHEFLUSH: &str = "/sys/module/zfs/parameters/zfs_nocacheflush";

/// The channel program that reads each member's `@t` ([module
/// docs](self)).
pub const MEMBERS_PROGRAM: &str = include_str!("cli/members.lua");

/// How many snapshots one `zfs holds -H` of the sweep names at most
/// ([`Zfs::tagged`]). [`HOLDS_ARG_BYTES`] bounds a command too, and
/// first where the names are long.
pub const HOLDS_AT_ONCE: usize = 1000;

/// How many bytes of snapshot names one `zfs holds -H` of the sweep
/// carries at most, counting each name with its NUL and its argument
/// pointer ([`arg_cost`]). The kernel counts the same against its limit
/// on a command's arguments and environment together (`fs/exec.c`,
/// `bprm_stack_limits`): a quarter of the stack limit (2 MiB with the
/// default 8 MiB, 256 KiB at `LimitSTACK=1M`), and never less than
/// `ARG_MAX`, 128 KiB. 64 KiB is half that floor, so the command's other words
/// and the agent's environment have 64 KiB whatever the stack limit:
/// about 248 names of 255 bytes, the longest ZFS allows. The design is
/// silent on the agent's environment; half the floor is the
/// conservative reading. Codex's P2 on #162 found 1000 such names
/// (264 000 bytes) over the 256 KiB of `LimitSTACK=1M`.
pub const HOLDS_ARG_BYTES: usize = 64 << 10;

/// What one argument costs against the kernel's limit on a command's
/// arguments: its bytes, its NUL, and its pointer in `argv`.
pub fn arg_cost(arg: &str) -> usize {
    arg.len() + 1 + std::mem::size_of::<usize>()
}

/// The sweep's commands of `zfs holds -H`: `names` cut, in their order,
/// into runs of at most [`HOLDS_AT_ONCE`] names and at most
/// [`HOLDS_ARG_BYTES`] of [`arg_cost`], each run at least one name. A
/// name longer than the bound alone (ZFS allows none) is a run alone.
pub fn holds_batches<'a, 'b>(names: &'b [&'a str]) -> Vec<&'b [&'a str]> {
    let mut batches = Vec::new();
    let mut start = 0;
    let mut bytes = 0;
    for (i, name) in names.iter().enumerate() {
        let cost = arg_cost(name);
        if i > start && (i - start == HOLDS_AT_ONCE || bytes + cost > HOLDS_ARG_BYTES) {
            batches.push(&names[start..i]);
            start = i;
            bytes = 0;
        }
        bytes += cost;
    }
    if start < names.len() {
        batches.push(&names[start..]);
    }
    batches
}

/// The most instructions a channel program is let run: the default of
/// `zfs_lua_max_instrlim`.
pub const PROGRAM_INSTRUCTIONS: u64 = 100_000_000;

/// The most memory a channel program is let use: the default of
/// `zfs_lua_max_memlimit`.
pub const PROGRAM_MEMORY: u64 = 100 << 20;

/// How long a command may run before it is killed. A read or a change
/// takes [`Limits::per_dataset`] more for each of the pool's datasets as
/// [`Zfs::count`] last read them, up to [`Limits::most`]; each of the
/// manifest's reads take [`Limits::full_per_dataset`] more, up to
/// [`Limits::full_most`], and its list [`Limits::list_per_snapshot`]
/// for each snapshot counted, as does the sweep's list of snapshots;
/// `zfs holds -r` takes [`Limits::holds_per_snapshot`] for each (the
/// lead's rules on M2.6c-2b, on N2 of #153's review, on R1 of #157's,
/// on R1-reach of #160's and on M2.6c-5d; provisional; [module
/// docs](self)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// A command that reads, on a pool of no datasets: `zpool status`,
    /// `zpool get`, `zfs holds`, a read-only program.
    pub read: Duration,
    /// A command that changes the pool and waits for a group to be
    /// written, on a pool of no datasets: `zfs snapshot -r`, `zfs
    /// destroy -r`, `zfs hold`, `zfs release`, `zfs set`, `zpool sync`.
    pub change: Duration,
    /// What each of the pool's datasets adds to a read or a change.
    pub per_dataset: Duration,
    /// The most a read or a change is let run, however many datasets.
    pub most: Duration,
    /// What each of the pool's datasets adds to each of the manifest's
    /// reads ([`Zfs::datasets`]), whose base is [`Limits::read`]. It was
    /// set at about twice the 90 ms a dataset the members program in full
    /// took on Ubuntu 26.04 (testing's run 45720225770), which the
    /// manifest's reads replaced; it is kept (the lead's ruling on
    /// M2.6c-5): `zfs get -H` took about 47 ms a dataset under a write
    /// load (468.2 s at 10 000 datasets, run 45733395008).
    pub full_per_dataset: Duration,
    /// The most each of the manifest's reads is let run, however many
    /// datasets: above its limit at 10 001 datasets (2060.2 s; the
    /// lead's ruling on N2 of #153's review).
    pub full_most: Duration,
    /// The count of the datasets itself ([`Zfs::count`]), whatever their
    /// number.
    pub count: Duration,
    /// What each snapshot counted adds to `zfs holds -r`, which walks
    /// every snapshot in the pool ([`Zfs::held`], [`Zfs::hold`],
    /// [`Zfs::release`]). Set from testing's `holds -r` recording at
    /// R1's scales (the lead's ruling on M2.6c-5d): it grew with every
    /// snapshot in the pool, not with the members it printed. On an idle
    /// pool, Ubuntu 24.04 took about 1.72 ms a snapshot (18.9 s at
    /// 11 000 snapshots, 174.2 s at 101 000) and 26.04 0.36 to 0.37 ms
    /// (3.9 s at 11 000, 21.9 s at 60 500, 37.3 s at 101 000). 10 ms is
    /// about five times the slower. The figures rest on one run on each
    /// image (run 45800940382), not under a write load.
    pub holds_per_snapshot: Duration,
    /// What each snapshot counted adds to the manifest's list
    /// ([`Zfs::datasets`]) and to the sweep's list of snapshots
    /// ([`Zfs::tagged`]), which list every snapshot in the pool (the
    /// lead's ruling on M2.6c-5d). The snapshots' list took about
    /// 1.84 ms a snapshot on Ubuntu 24.04 (185.9 s at 101 000 snapshots)
    /// and 0.41 ms on 26.04 (41.6 s); 10 ms is about five times the
    /// slower. The figures rest on one run on each image (run
    /// 45800940382), on an idle pool, at 1000 snapshots a dataset. The
    /// cost of a snapshot grows with the snapshots a dataset holds: the
    /// review of #162 read 0.40 ms a line up to 5500 a dataset and
    /// 1.06 ms at 12 500, beside writers, so at 12 500 a dataset on
    /// 24.04 the margin may be about two (N-b, N-c of #162's review). Before this ruling, a snapshot
    /// counted as a dataset: [`Limits::full_per_dataset`], 200 ms.
    pub list_per_snapshot: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            read: Duration::from_secs(60),
            change: Duration::from_secs(120),
            per_dataset: Duration::from_millis(100),
            most: Duration::from_secs(20 * 60),
            full_per_dataset: Duration::from_millis(200),
            full_most: Duration::from_secs(35 * 60),
            count: Duration::from_secs(60),
            holds_per_snapshot: Duration::from_millis(10),
            list_per_snapshot: Duration::from_millis(10),
        }
    }
}

/// What the limits are scaled by ([`Zfs::count`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scale {
    /// Not counted yet, or the last count failed: the bases. A failed
    /// count does not give the round the longest limits: its own calls
    /// then say what is wrong, and one round does not wait an hour on a
    /// pool that holds every command (the lead's ruling on N2 of #153's
    /// review, which reverses the longest limits first ruled).
    Base,
    /// The pool's datasets as last counted, and its snapshots where
    /// they were counted too.
    Counted {
        /// File systems and volumes.
        datasets: u64,
        /// Snapshots; `None` where their count failed.
        snapshots: Option<u64>,
    },
}

/// `base` and `per` more for each of `n`, at most `most`.
fn grown(base: Duration, per: Duration, most: Duration, n: u64) -> Duration {
    let more = u32::try_from(n)
        .ok()
        .and_then(|n| per.checked_mul(n))
        .unwrap_or(most);
    base.saturating_add(more).min(most)
}

/// [`Zfs`] on the commands ([module docs](self)).
#[derive(Debug)]
pub struct ZfsCli<H> {
    host: H,
    limits: Limits,
    scale: Scale,
    /// Why the last count did not count the snapshots, where it counted
    /// the datasets and not them: the walks' errors name it.
    uncounted: Option<String>,
}

impl<H: Host> ZfsCli<H> {
    /// The commands on `host`, each within `limits`.
    pub fn new(host: H, limits: Limits) -> Self {
        ZfsCli {
            host,
            limits,
            scale: Scale::Base,
            uncounted: None,
        }
    }

    /// `e`, a walk's error, with the reason its limit did not grow with
    /// the snapshots where their count failed (the lead's ruling on
    /// M2.6c-5d: an error that names the count, and not exit 1 on every
    /// round with no reason). A missing pool or a vanished snapshot is
    /// returned as it is.
    fn walked(&self, e: ZfsError) -> ZfsError {
        match (e, &self.uncounted) {
            (ZfsError::Other(why), Some(count)) => ZfsError::Other(format!(
                "{why} (on a limit that did not grow with the snapshots, since \
                 their count failed: {count})"
            )),
            (e, _) => e,
        }
    }

    /// The limit of a command whose base is `base`, `per` more for each
    /// dataset counted, at most `most` ([`Limits`]).
    fn scaled(&self, base: Duration, per: Duration, most: Duration) -> Duration {
        match self.scale {
            Scale::Base => base.min(most),
            Scale::Counted { datasets, .. } => grown(base, per, most, datasets),
        }
    }

    /// The limit of a command that reads.
    fn read_limit(&self) -> Duration {
        let l = &self.limits;
        self.scaled(l.read, l.per_dataset, l.most)
    }

    /// The limit of a command that changes the pool.
    fn change_limit(&self) -> Duration {
        let l = &self.limits;
        self.scaled(l.change, l.per_dataset, l.most)
    }

    /// The limit of each of the manifest's two gets ([`Zfs::datasets`]).
    fn full_read_limit(&self) -> Duration {
        let l = &self.limits;
        self.scaled(l.read, l.full_per_dataset, l.full_most)
    }

    /// The limit of the manifest's list ([`Zfs::datasets`]), which lists
    /// every snapshot in the pool as well as its datasets: each snapshot
    /// counted adds [`Limits::list_per_snapshot`] (R1 of #157's review,
    /// the lead's ruling on M2.6c-5d), at most [`Limits::full_most`].
    /// Where the snapshots' count failed, the datasets' scale:
    /// the conservative reading, no longer a limit than before on a pool
    /// whose commands fail.
    fn list_limit(&self) -> Duration {
        let l = &self.limits;
        match self.scale {
            Scale::Counted {
                datasets,
                snapshots: Some(snapshots),
            } => grown(
                grown(l.read, l.full_per_dataset, l.full_most, datasets),
                l.list_per_snapshot,
                l.full_most,
                snapshots,
            ),
            _ => self.full_read_limit(),
        }
    }

    /// The limit of a read that walks every snapshot in the pool as well
    /// as its datasets, `zfs holds -r` and the sweep's list of snapshots
    /// ([`Zfs::tagged`]): a read's (at most [`Limits::most`]), and
    /// `per_snapshot` more for each snapshot counted, at most
    /// [`Limits::full_most`] (R1-reach of #160's review, the lead's ruling
    /// (b)). Where the snapshots were not counted, a read's limit, as
    /// before. A read's limit is its floor only while `most` is at most
    /// `full_most`, as in the defaults (20 and 35 min): with a larger
    /// `most`, the cap could cut it below a read's (nit E of #161's
    /// review).
    fn walk_limit(&self, per_snapshot: Duration) -> Duration {
        let l = &self.limits;
        match self.scale {
            Scale::Counted {
                snapshots: Some(snapshots),
                ..
            } => grown(self.read_limit(), per_snapshot, l.full_most, snapshots),
            _ => self.read_limit(),
        }
    }

    /// `zfs` with `args`, which read in `pool`, within `limit`: its
    /// output's bytes where it exits 0. Where the command names the pool
    /// itself (`names_pool`), `cannot open 'POOL': dataset does not
    /// exist` is [`ZfsError::NoPool`] where `zpool list` agrees
    /// ([`ZfsCli::confirm`]): `zfs list` and `zfs get` say so of a pool
    /// that does not exist. The same message of a child or a snapshot is
    /// an error (N5 of #157's review).
    fn zfs_bytes(
        &mut self,
        args: &[&str],
        limit: Duration,
        pool: &str,
        names_pool: bool,
    ) -> Result<Vec<u8>, ZfsError> {
        let output = self.exec("zfs", args, &[], limit)?;
        if output.code == Some(0) {
            return Ok(output.stdout);
        }
        if names_pool {
            let gone = format!("cannot open '{pool}': dataset does not exist");
            if String::from_utf8_lossy(&output.stderr)
                .lines()
                .any(|line| line == gone)
            {
                return Err(self.confirm("zfs", args, &output, pool));
            }
        }
        Err(self.failed("zfs", args, &output, false, pool))
    }

    /// One local property's value alone ([module docs](self), "The
    /// manifest's read"): `zfs get -H -p -o value PROPERTY NAME`, which
    /// prints the value and one newline, exactly. Anything else, or a
    /// value longer than ZFS lets one be ([`VALUE_MOST`]), cannot be read.
    fn value(&mut self, name: &str, property: &str) -> Result<Vec<u8>, ZfsError> {
        let args = ["get", "-H", "-p", "-o", "value", property, name];
        let out = self.zfs_bytes(&args, self.read_limit(), pool_of(name), false)?;
        match out.strip_suffix(b"\n") {
            Some(value) if value.len() <= VALUE_MOST => Ok(value.to_vec()),
            _ => Err(ZfsError::Other(format!(
                "{}: the value of `{property}` on `{name}` cannot be read ({} bytes, {})",
                command("zfs", &args),
                out.len(),
                if out.ends_with(b"\n") {
                    "longer than a value can be"
                } else {
                    "with no newline at its end"
                }
            ))),
        }
    }

    /// The host, for the tests.
    pub fn host(&mut self) -> &mut H {
        &mut self.host
    }

    /// Run `program` with `args` within `limit`: what it gave, whatever
    /// its exit code.
    fn exec(
        &mut self,
        program: &str,
        args: &[&str],
        stdin: &[u8],
        limit: Duration,
    ) -> Result<Output, ZfsError> {
        self.host
            .run(program, args, stdin, limit)
            .map_err(|e| ZfsError::Other(format!("{}: {e}", command(program, args))))
    }

    /// Run `program` with `args`, which act on `pool`, within `limit`;
    /// its standard output where it exits 0, and otherwise the error its
    /// message says.
    fn run(
        &mut self,
        program: &str,
        args: &[&str],
        stdin: &[u8],
        limit: Duration,
        snapshot: bool,
        pool: &str,
    ) -> Result<String, ZfsError> {
        let output = self.exec(program, args, stdin, limit)?;
        if output.code == Some(0) {
            return String::from_utf8(output.stdout).map_err(|_| {
                ZfsError::Other(format!(
                    "{}: its output is not UTF-8",
                    command(program, args)
                ))
            });
        }
        Err(self.failed(program, args, &output, snapshot, pool))
    }

    /// The error a command that did not exit 0 gives ([`failure`]); one
    /// that says the pool is not there only where `zpool list` agrees
    /// that `pool`, the one the command acts on, is not
    /// ([`ZfsCli::confirm`]).
    fn failed(
        &mut self,
        program: &str,
        args: &[&str],
        output: &Output,
        snapshot: bool,
        pool: &str,
    ) -> ZfsError {
        match failure(program, args, output, snapshot) {
            ZfsError::NoPool(_) => self.confirm(program, args, output, pool),
            e => e,
        }
    }

    /// A command that said its pool is not there, by `zpool list -H -o
    /// name`: [`ZfsError::NoPool`] where `pool`, the pool the command
    /// acts on, is not a line of it, and [`ZfsError::Vanished`] where it
    /// is. Only the target pool counts, and not any word of the command:
    /// a pool named like a subcommand or an option's value (`get`,
    /// `name`) is not the command's (Codex on #160). libzfs says `no such
    /// pool or dataset` of a dataset gone between a command's open of it
    /// and its read (testing's agent rig, a snapshot destroyed during `zfs
    /// holds -r`; the lead's ruling). `zpool` and `zfs program` say `no
    /// such pool` of a pool that is not there; a program whose pool is
    /// listed is read as vanished too, which is not known to happen: a
    /// read-only program holds the pool's configuration lock for its
    /// whole run (`dsl_pool_hold`), and a destroy, a sync task, takes it
    /// to write (nit B of #161's review), so its reading again is
    /// defensive.
    /// Where `zpool list` does not answer, an error that says so, and not
    /// [`ZfsError::NoPool`]: the conservative reading.
    fn confirm(&mut self, program: &str, args: &[&str], output: &Output, pool: &str) -> ZfsError {
        let said = format!(
            "{}: {}",
            command(program, args),
            String::from_utf8_lossy(&output.stderr).trim()
        );
        let list = ["list", "-H", "-o", "name"];
        let listed = match self.exec("zpool", &list, &[], self.read_limit()) {
            Ok(out) if out.code == Some(0) => out.stdout,
            Ok(out) => {
                return ZfsError::Other(format!(
                    "{said}; and `zpool list -H -o name` failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ))
            }
            Err(e) => return ZfsError::Other(format!("{said}; and {e}")),
        };
        if String::from_utf8_lossy(&listed)
            .lines()
            .any(|line| line == pool)
        {
            ZfsError::Vanished(format!("{said} (`zpool list` lists its pool)"))
        } else {
            ZfsError::NoPool(command(program, args))
        }
    }

    /// `read` again while it fails as [`ZfsError::Vanished`], at most
    /// [`VANISHED_READS`] times in all: a `zfs destroy -r` is one sync
    /// task, so a read after one member's snapshot was found gone sees
    /// the others gone too.
    fn again<T>(
        &mut self,
        mut read: impl FnMut(&mut Self) -> Result<T, ZfsError>,
    ) -> Result<T, ZfsError> {
        let mut tries = 1;
        loop {
            match read(self) {
                Err(ZfsError::Vanished(_)) if tries < VANISHED_READS => tries += 1,
                done => return done,
            }
        }
    }

    fn zpool(&mut self, args: &[&str], limit: Duration, pool: &str) -> Result<String, ZfsError> {
        self.run("zpool", args, &[], limit, false, pool)
    }

    fn zfs(
        &mut self,
        args: &[&str],
        limit: Duration,
        snapshot: bool,
        pool: &str,
    ) -> Result<String, ZfsError> {
        self.run("zfs", args, &[], limit, snapshot, pool)
    }

    /// A channel program's returned value, the object under `return`.
    fn program(
        &mut self,
        pool: &str,
        text: &str,
        args: &[&str],
        read_only: bool,
        limit: Duration,
    ) -> Result<serde_json::Map<String, Value>, ZfsError> {
        let out = self.program_output(pool, text, args, read_only, limit)?;
        readable(&out).map_err(|why| ZfsError::Other(format!("zfs program: {why}")))
    }

    /// `zfs destroy -r pool@t`. `zfs destroy -r` of a snapshot that is
    /// not there says `could not find any snapshots to destroy`, or
    /// `dataset does not exist` of a pool that is not (both recorded):
    /// both are destroyed. `zfs destroy` takes `%`
    /// ranges and `,` lists after the `@`: a `t` that is not a snapshot's
    /// name ([`skyzfs_control::valid_name`]) is not destroyed at all.
    fn destroy_r(&mut self, pool: &str, t: &str) -> Result<(), ZfsError> {
        let snapshot = format!("{pool}@{t}");
        if !skyzfs_control::valid_name(t) {
            return Err(ZfsError::Other(format!(
                "{snapshot} was not destroyed: `{t}` is not a snapshot's name"
            )));
        }
        let args = ["destroy", "-r", snapshot.as_str()];
        // Killed at its limit, or not run to its end: the destroy's ioctl
        // goes on in the kernel and may finish in its own time (TL-F3 of
        // #147's review).
        let output = self
            .exec("zfs", &args, &[], self.change_limit())
            .map_err(|e| match e {
                ZfsError::Other(why) => {
                    ZfsError::Other(format!("{snapshot} may not have been destroyed: {why}"))
                }
                e => e,
            })?;
        if output.code == Some(0) || says(&output.stderr, "could not find any snapshots to destroy")
        {
            return Ok(());
        }
        match self.failed("zfs", &args, &output, true, pool) {
            ZfsError::NoSnapshot => Ok(()),
            e => Err(e),
        }
    }

    /// The snapshots under `snapshot` (`pool@t` and its members' `@t`)
    /// that have a hold of `tag`, by `zfs holds -H -r`; `None` where
    /// `snapshot` does not exist.
    fn tagged_snapshots(
        &mut self,
        snapshot: &str,
        tag: &str,
    ) -> Result<Option<BTreeSet<String>>, ZfsError> {
        let args = ["holds", "-H", "-r", snapshot];
        let limit = self.walk_limit(self.limits.holds_per_snapshot);
        let holds = match self.zfs(&args, limit, true, pool_of(snapshot)) {
            Ok(holds) => holds,
            Err(ZfsError::NoSnapshot) => return Ok(None),
            Err(e) => return Err(self.walked(e)),
        };
        Ok(Some(
            holds
                .lines()
                .filter_map(|line| {
                    let mut fields = line.split('\t');
                    let name = fields.next()?;
                    (fields.next() == Some(tag)).then(|| name.to_string())
                })
                .collect(),
        ))
    }

    /// One read of [`Zfs::tagged`]: the snapshots in `pool` that have
    /// any hold (`userrefs` above 0), by `zfs list` within the list's
    /// limit on the snapshots counted ([`ZfsCli::walk_limit`]), then their
    /// holds by `zfs holds -H`, at most [`HOLDS_AT_ONCE`] snapshots and
    /// [`HOLDS_ARG_BYTES`] of names a command ([`holds_batches`]), each
    /// within a read's limit. `zfs holds -r` would not do: it reads
    /// the snapshots of one name `t` alone. A snapshot gone between the
    /// two reads (`dataset does not exist`, or libzfs's `no such pool or
    /// dataset` where `zpool list` lists the pool) is
    /// [`ZfsError::Vanished`], and the whole read is made again. A
    /// dataset gone during the list is no error: libzfs ends its walk
    /// there and the list exits 0 without that dataset's snapshots,
    /// whose holds are gone with them, which is right for the sweep; the
    /// list's own `no such pool or dataset` is read again all the same,
    /// defensively (N-a of #162's review). A pool that is not there is [`ZfsError::NoPool`] where
    /// `zpool list` agrees: `zfs list POOL` says `cannot open 'POOL':
    /// dataset does not exist` of it.
    fn tagged_once(&mut self, pool: &str, tag: &str) -> Result<BTreeSet<String>, ZfsError> {
        let list = [
            "list",
            "-H",
            "-p",
            "-r",
            "-t",
            "snapshot",
            "-o",
            "name,userrefs",
            pool,
        ];
        let limit = self.walk_limit(self.limits.list_per_snapshot);
        let listed = self
            .zfs_bytes(&list, limit, pool, true)
            .map_err(|e| self.walked(e))?;
        let listed = String::from_utf8_lossy(&listed);
        let mut held = Vec::new();
        for line in listed.lines() {
            let refs = line
                .rsplit_once('\t')
                .and_then(|(name, refs)| Some((name, refs.parse::<u64>().ok()?)));
            match refs {
                Some((name, refs)) if name.contains('@') => {
                    if refs > 0 {
                        held.push(name);
                    }
                }
                _ => {
                    return Err(ZfsError::Other(format!(
                        "{}: a line that is not a snapshot and its count of holds: {line:?}",
                        command("zfs", &list)
                    )))
                }
            }
        }
        let mut tagged = BTreeSet::new();
        for names in holds_batches(&held) {
            let mut args = vec!["holds", "-H"];
            args.extend_from_slice(names);
            // Read as bytes: another holder's tag may hold any byte but
            // NUL, and one that is not UTF-8 must not fail the sweep (D1
            // of #162's review).
            let output = self.exec("zfs", &args, &[], self.read_limit())?;
            if output.code != Some(0) {
                return Err(match self.failed("zfs", &args, &output, true, pool) {
                    ZfsError::NoSnapshot => ZfsError::Vanished(format!(
                        "{}: a snapshot listed with a hold is gone",
                        command("zfs", &args)
                    )),
                    e => e,
                });
            }
            let asked: BTreeSet<&[u8]> = names.iter().map(|name| name.as_bytes()).collect();
            for line in output.stdout.split(|&b| b == b'\n') {
                let mut fields = line.split(|&b| b == b'\t');
                // A line of each hold: the snapshot, the tag, when. Only
                // another holder's tag that holds a newline or a tab
                // makes a line whose first field is not a snapshot this
                // command asked for, or that has no second field: such a
                // line is skipped (F1 and N-e of #162's review). The
                // agent's own tag has neither, so no hold of the agent's
                // is skipped. A tag made to forge a whole line can still
                // name an asked snapshot with the agent's tag; the sweep
                // then releases the agent's tag there, which only the
                // agent sets, and nothing of another holder's.
                let (Some(snapshot), Some(held_by)) = (fields.next(), fields.next()) else {
                    continue;
                };
                if held_by != tag.as_bytes() || !asked.contains(snapshot) {
                    continue;
                }
                if let Some((_, t)) = String::from_utf8_lossy(snapshot).split_once('@') {
                    tagged.insert(t.to_string());
                }
            }
        }
        Ok(tagged)
    }

    /// A channel program's standard output, where it exits 0 within
    /// `limit`.
    fn program_output(
        &mut self,
        pool: &str,
        text: &str,
        args: &[&str],
        read_only: bool,
        limit: Duration,
    ) -> Result<Vec<u8>, ZfsError> {
        let instructions = PROGRAM_INSTRUCTIONS.to_string();
        let memory = PROGRAM_MEMORY.to_string();
        let mut all = vec!["program"];
        if read_only {
            all.push("-n");
        }
        all.extend(["-j", "-t", &instructions, "-m", &memory, pool, "-"]);
        all.extend(args);
        let output = self.exec("zfs", &all, text.as_bytes(), limit)?;
        if output.code == Some(0) {
            Ok(output.stdout)
        } else {
            Err(self.failed("zfs", &all, &output, false, pool))
        }
    }

    /// The tree of `zpool status -P` (`guids` false) or `-P -g` (true).
    fn status(&mut self, pool: &str, guids: bool) -> Result<Status, ZfsError> {
        let mut args = vec!["status", "-P"];
        if guids {
            args.push("-g");
        }
        args.push(pool);
        let out = self.zpool(&args, self.read_limit(), pool)?;
        parse_status(&out, pool).map_err(|why| ZfsError::Other(format!("zpool status: {why}")))
    }

    /// A vdev of the `-P` tree, its GUID from the `-g` tree's node in the
    /// same place.
    fn vdev(&mut self, named: &Entry, guided: &Entry) -> Result<Vdev, ZfsError> {
        let guid = parse_guid(&guided.name).ok_or_else(|| {
            ZfsError::Other(format!("zpool status -g: `{}` is not a GUID", guided.name))
        })?;
        let mut children = Vec::with_capacity(named.children.len());
        for (n, g) in named.children.iter().zip(&guided.children) {
            children.push(self.vdev(n, g)?);
        }
        let (kind, rdev) = match interior(&named.name) {
            Some(kind) => (kind, None),
            None if !children.is_empty() => (VdevKind::Other(named.name.clone()), None),
            None if named.name.starts_with('/') => {
                match self.host.stat(Path::new(&named.name)) {
                    Ok(Node::Block(rdev)) => (VdevKind::Disk, Some(rdev)),
                    Ok(Node::File) => (VdevKind::File, None),
                    // A path that is neither, or cannot be read: a disk
                    // whose device is not known, which the premise refuses.
                    Ok(Node::Other) | Err(_) => (VdevKind::Disk, None),
                }
            }
            // No path: a leaf ZFS cannot open, named by its GUID.
            None => (VdevKind::Other(named.name.clone()), None),
        };
        Ok(Vdev {
            kind,
            guid,
            rdev,
            children,
        })
    }
}

impl<H: Host> Zfs for ZfsCli<H> {
    fn layout(&mut self, pool: &str) -> Result<Layout, ZfsError> {
        let named = self.status(pool, false)?;
        let guided = self.status(pool, true)?;
        if !named.same_shape(&guided) {
            return Err(ZfsError::Other(
                "zpool status: the pool's vdevs changed between two reads".into(),
            ));
        }
        let mut layout = Layout {
            data: Vec::new(),
            special: 0,
            dedup: 0,
            other: 0,
            busy: named.busy.clone(),
            removed: named.removed.clone(),
        };
        for ((class, named), (_, guided)) in named.classes.iter().zip(&guided.classes) {
            match class {
                Class::Data => {
                    for (n, g) in named.iter().zip(guided) {
                        let vdev = self.vdev(n, g)?;
                        layout.data.push(vdev);
                    }
                }
                Class::Special => layout.special += named.len(),
                Class::Dedup => layout.dedup += named.len(),
                Class::Logs | Class::Cache | Class::Spares => layout.other += named.len(),
            }
        }
        Ok(layout)
    }

    fn props(&mut self, pool: &str) -> Result<Props, ZfsError> {
        let args = [
            "get",
            "-H",
            "-p",
            "-o",
            "property,value",
            "guid,load_guid,multihost,readonly,checkpoint",
            pool,
        ];
        let output = self.exec("zpool", &args, &[], self.read_limit())?;
        if output.code != Some(0) {
            let error = self.failed("zpool", &args, &output, false, pool);
            // ZFS 2.2's `zpool get` takes a name that is no pool's for no
            // pool named at all ([module docs](self)): `zpool status`
            // says whether the pool is there.
            if matches!(error, ZfsError::Other(_)) && says(&output.stderr, "missing pool name") {
                if let Err(gone @ ZfsError::NoPool(_)) =
                    self.zpool(&["status", "-P", pool], self.read_limit(), pool)
                {
                    return Err(gone);
                }
            }
            return Err(error);
        }
        let out = String::from_utf8(output.stdout)
            .map_err(|_| ZfsError::Other("zpool get: its output is not UTF-8".into()))?;
        parse_props(&out).map_err(|why| ZfsError::Other(format!("zpool get: {why}")))
    }

    fn nocacheflush(&mut self) -> Result<bool, ZfsError> {
        let bytes = self
            .host
            .read(Path::new(NOCACHEFLUSH))
            .map_err(|e| ZfsError::Other(format!("{NOCACHEFLUSH}: {e}")))?;
        let text = String::from_utf8_lossy(&bytes);
        match text.trim().parse::<i64>() {
            Ok(n) => Ok(n != 0),
            Err(_) => Err(ZfsError::Other(format!(
                "{NOCACHEFLUSH}: `{}` is not a number",
                text.trim()
            ))),
        }
    }

    fn count(&mut self, pool: &str) -> Result<u64, ZfsError> {
        let args = [
            "list",
            "-r",
            "-H",
            "-o",
            "name",
            "-t",
            "filesystem,volume",
            pool,
        ];
        let counted = self
            .run("zfs", &args, &[], self.limits.count, false, pool)
            .map(|out| out.lines().filter(|line| !line.is_empty()).count() as u64);
        self.uncounted = None;
        let datasets = match counted {
            Ok(n) => n,
            Err(e) => {
                self.scale = Scale::Base;
                return Err(e);
            }
        };
        self.scale = Scale::Counted {
            datasets,
            snapshots: None,
        };
        // The snapshots, for the limits of the reads that walk them (the
        // manifest's list, `zfs holds -r` and the sweep's list), within
        // the longest limit: not within one that hangs on the count it
        // makes (the lead's ruling on M2.6c-5d).
        let args = ["list", "-r", "-H", "-o", "name", "-t", "snapshot", pool];
        let snapshots = match self.run("zfs", &args, &[], self.limits.full_most, false, pool) {
            Ok(out) => Some(out.lines().filter(|line| !line.is_empty()).count() as u64),
            Err(e) => {
                self.uncounted = Some(e.to_string());
                None
            }
        };
        self.scale = Scale::Counted {
            datasets,
            snapshots,
        };
        Ok(datasets)
    }

    fn snapshot(&mut self, pool: &str, t: &str) -> Result<(), ZfsError> {
        let snapshot = format!("{pool}@{t}");
        let args = ["snapshot", "-r", snapshot.as_str()];
        let output = self.exec("zfs", &args, &[], self.change_limit())?;
        if output.code == Some(0) {
            return Ok(());
        }
        if says(&output.stderr, "dataset already exists") {
            return Err(ZfsError::Exists);
        }
        Err(self.failed("zfs", &args, &output, false, pool))
    }

    fn datasets(&mut self, pool: &str, t: &str) -> Result<Vec<Dataset>, ZfsError> {
        let limit = self.full_read_limit();
        let list = list_words(pool);
        let listed = self
            .zfs_bytes(&list, self.list_limit(), pool, true)
            .map_err(|e| self.walked(e))?;
        let mut datasets = parse_listed(&listed, pool, t)
            .map_err(|why| ZfsError::Other(format!("{}: {why}", command("zfs", &list))))?;
        let get = get_words(pool, PAIRS);
        let pairs = self.zfs_bytes(&get, limit, pool, true)?;
        let pairs = parse_pairs(&pairs)
            .map_err(|why| ZfsError::Other(format!("{}: {why}", command("zfs", &get))))?;
        let values = self.zfs_bytes(&get_words(pool, VALUES), limit, pool, true)?;
        let (mut read, again) = match_values(&pairs, &values);
        let again = pairs
            .iter()
            .filter(|(name, property)| again.contains(name) && carried(property));
        for (name, property) in again {
            let value = self.value(name, property)?;
            read.insert((name.clone(), property.clone()), value);
        }
        for ((name, property), value) in read {
            if !carried(&property) {
                continue;
            }
            // A dataset created after the list is not a member of the
            // take; the engine's check sees it in the next read.
            if let Some(dataset) = datasets.get_mut(&name) {
                dataset.local.insert(property, value);
            }
        }
        Ok(datasets.into_values().collect())
    }

    fn destroy(&mut self, pool: &str, t: &str) -> Result<(), ZfsError> {
        self.destroy_r(pool, t)
    }

    fn members(&mut self, pool: &str, t: &str) -> Result<Option<Vec<Member>>, ZfsError> {
        let returned = self.again(|zfs| {
            let limit = zfs.read_limit();
            zfs.program(pool, MEMBERS_PROGRAM, &[pool, t], true, limit)
        })?;
        parse_members(&returned, pool)
            .map_err(|why| ZfsError::Other(format!("zfs program (members): {why}")))
    }

    fn hold(&mut self, pool: &str, t: &str, tag: &str) -> Result<(), ZfsError> {
        let snapshot = format!("{pool}@{t}");
        let args = ["hold", "-r", tag, &snapshot];
        let output = self.exec("zfs", &args, &[], self.change_limit())?;
        if output.code == Some(0) {
            return Ok(());
        }
        if !says(&output.stderr, "tag already exists") {
            return Err(self.failed("zfs", &args, &output, true, pool));
        }
        // A member has the tag, and so the command held none (ZFS holds
        // all or none): the hold is there only where every member's `@t`
        // has it.
        let Some(members) = self.members(pool, t)? else {
            return Err(ZfsError::NoSnapshot);
        };
        let tagged = self.tagged_snapshots(&snapshot, tag)?.unwrap_or_default();
        let lacking: Vec<String> = members
            .iter()
            .map(|m| format!("{}@{t}", m.name))
            .filter(|name| !tagged.contains(name))
            .collect();
        if lacking.is_empty() {
            Ok(())
        } else {
            Err(ZfsError::Other(format!(
                "zfs hold -r {tag} {snapshot}: the tag is on some members and not on {}",
                lacking.join(", ")
            )))
        }
    }

    fn release(&mut self, pool: &str, t: &str, tag: &str) -> Result<(), ZfsError> {
        let snapshot = format!("{pool}@{t}");
        let args = ["release", "-r", tag, &snapshot];
        let output = self.exec("zfs", &args, &[], self.change_limit())?;
        if output.code == Some(0) {
            return Ok(());
        }
        if !says(&output.stderr, "no such tag") {
            return match self.failed("zfs", &args, &output, true, pool) {
                ZfsError::NoSnapshot => Ok(()),
                e => Err(e),
            };
        }
        // `-r` releases what has the tag and refuses for what has not:
        // that no member keeps it is checked.
        let Some(tagged) = self.tagged_snapshots(&snapshot, tag)? else {
            return Ok(());
        };
        if tagged.is_empty() {
            Ok(())
        } else {
            let kept: Vec<&str> = tagged.iter().map(String::as_str).collect();
            Err(ZfsError::Other(format!(
                "zfs release -r {tag} {snapshot}: {} kept the hold: {}",
                kept.join(", "),
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        }
    }

    fn held(
        &mut self,
        pool: &str,
        t: &str,
        tag: &str,
        members: Option<&[Member]>,
    ) -> Result<Held, ZfsError> {
        // Every member's `@t`, not only the root's: a hold on some members
        // only (made by hand, or by a `hold -r` that a member's snapshot
        // came after) is not held, so that the planner holds again and
        // the hold's check refuses it (NL-F1 of #147's review); and it is
        // a part, which is released when the publish ends (TL-F1).
        let snapshot = format!("{pool}@{t}");
        let Some(tagged) = self.again(|zfs| zfs.tagged_snapshots(&snapshot, tag))? else {
            return Ok(Held::No);
        };
        if tagged.is_empty() {
            return Ok(Held::No);
        }
        if !tagged.contains(&snapshot) {
            return Ok(Held::Part);
        }
        // `pool@t` gone: the tags read are released all the same.
        let Some(members) = members else {
            return Ok(Held::Part);
        };
        if members
            .iter()
            .all(|m| tagged.contains(&format!("{}@{t}", m.name)))
        {
            Ok(Held::All)
        } else {
            Ok(Held::Part)
        }
    }

    fn tagged(&mut self, pool: &str, tag: &str) -> Result<BTreeSet<String>, ZfsError> {
        self.again(|zfs| zfs.tagged_once(pool, tag))
    }

    fn assist(&mut self, pool: &str, value: u64) -> Result<(), ZfsError> {
        let set = format!("{ASSIST_PROPERTY}={value}");
        self.zfs(&["set", &set, pool], self.change_limit(), false, pool)?;
        self.zpool(&["sync", pool], self.change_limit(), pool)?;
        Ok(())
    }
}

/// `program args...`, as a person would type it.
fn command(program: &str, args: &[&str]) -> String {
    let mut words = vec![program];
    words.extend(args);
    words.join(" ")
}

/// Whether a line of `stderr` says `what` in its message: the text after
/// the line's last `": "`, so that a name, which comes before it, cannot
/// say it.
fn says(stderr: &[u8], what: &str) -> bool {
    String::from_utf8_lossy(stderr).lines().any(|line| {
        let message = line.rsplit_once(": ").map_or(line, |(_, message)| message);
        message.contains(what)
    })
}

/// How many times a read whose dataset vanished
/// ([`ZfsError::Vanished`]) is made in all, by [`Zfs::members`],
/// [`Zfs::held`] and [`Zfs::tagged`].
pub const VANISHED_READS: u32 = 3;

/// The pool a dataset's or snapshot's `name` is in: the name up to its
/// first `/` or `@`.
fn pool_of(name: &str) -> &str {
    name.split(['/', '@']).next().unwrap_or(name)
}

/// The error a command that did not exit 0 gives, by its message.
fn failure(program: &str, args: &[&str], output: &Output, snapshot: bool) -> ZfsError {
    if says(&output.stderr, "no such pool") {
        return ZfsError::NoPool(command(program, args));
    }
    if snapshot && says(&output.stderr, "dataset does not exist") {
        return ZfsError::NoSnapshot;
    }
    let message = String::from_utf8_lossy(&output.stderr);
    let message = message.trim();
    let code = match output.code {
        Some(code) => format!("exit {code}"),
        None => "killed by a signal".into(),
    };
    ZfsError::Other(format!("{}: {code}: {message}", command(program, args)))
}

/// The classes of vdev in `zpool status`, by their headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Class {
    Data,
    Logs,
    Cache,
    Spares,
    Special,
    Dedup,
}

impl Class {
    fn header(word: &str) -> Option<Class> {
        Some(match word {
            "logs" => Class::Logs,
            "cache" => Class::Cache,
            "spares" => Class::Spares,
            "special" => Class::Special,
            "dedup" => Class::Dedup,
            _ => return None,
        })
    }
}

/// A vdev of `zpool status`, by the name it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) name: String,
    pub(crate) children: Vec<Entry>,
}

/// What `zpool status` shows of a pool's vdevs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Status {
    /// Each class's top-level vdevs, in the order shown, the data class
    /// first.
    pub(crate) classes: Vec<(Class, Vec<Entry>)>,
    /// A resilver, rebuild or removal under way, in its line's words.
    pub(crate) busy: Option<String>,
    /// The `remove:` line, under way or done.
    pub(crate) removed: Option<String>,
}

impl Status {
    fn same_shape(&self, other: &Status) -> bool {
        fn same(a: &[Entry], b: &[Entry]) -> bool {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(&a.children, &b.children))
        }
        self.classes.len() == other.classes.len()
            && self
                .classes
                .iter()
                .zip(&other.classes)
                .all(|((ca, a), (cb, b))| ca == cb && same(a, b))
    }
}

/// The kind of an interior vdev (or an indirect one) by its name in
/// `zpool status`: `mirror-0`, `raidz2-1`, `draid1:2d:6c:0s-0`,
/// `replacing-0`, `spare-0`, `indirect-0`.
fn interior(name: &str) -> Option<VdevKind> {
    let (stem, index) = name.rsplit_once('-')?;
    if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(match stem {
        "mirror" => VdevKind::Mirror,
        "replacing" => VdevKind::Replacing,
        "spare" => VdevKind::Spare,
        "indirect" => VdevKind::Indirect,
        _ if stem.starts_with("raidz") && stem[5..].bytes().all(|b| b.is_ascii_digit()) => {
            VdevKind::Raidz
        }
        _ if stem.starts_with("draid") => VdevKind::Draid,
        _ => return None,
    })
}

/// `zpool status -P [-g] POOL`'s text: the config's tree and the scan
/// and removal lines ([module docs](self)).
pub(crate) fn parse_status(text: &str, pool: &str) -> Result<Status, String> {
    let mut lines = text.lines();
    let mut named = None;
    let mut busy = None;
    let mut removed = None;
    for line in lines.by_ref() {
        let trimmed = line.trim_start();
        if let Some(name) = trimmed.strip_prefix("pool:") {
            named = Some(name.trim().to_string());
        } else if let Some(scan) = trimmed.strip_prefix("scan:") {
            let scan = scan.trim();
            if scan.contains("in progress")
                && (scan.contains("resilver") || scan.contains("rebuild"))
            {
                busy = Some(format!("scan: {scan}"));
            }
        } else if let Some(remove) = trimmed.strip_prefix("remove:") {
            let remove = remove.trim();
            if remove.contains("in progress") {
                busy = Some(format!("remove: {remove}"));
            }
            removed = Some(format!("remove: {remove}"));
        } else if trimmed == "config:" {
            break;
        }
    }
    match named {
        Some(name) if name == pool => {}
        Some(name) => return Err(format!("the status of `{name}`, not `{pool}`")),
        None => return Err("no `pool:` line".into()),
    }
    // A blank line, the column heads, then the tree to a blank line.
    let mut tree = lines
        .skip_while(|line| line.trim().is_empty())
        .take_while(|line| !line.trim().is_empty());
    match tree.next() {
        Some(heads) if heads.split_whitespace().next() == Some("NAME") => {}
        _ => return Err("no `NAME` heads under `config:`".into()),
    }
    let mut classes: Vec<(Class, Vec<Entry>)> = Vec::new();
    // The path from the class's list to the vdev last read, by depth.
    let mut root_seen = false;
    for line in tree {
        let body = line
            .strip_prefix('\t')
            .ok_or_else(|| format!("a line of the tree that is not indented by a tab: `{line}`"))?;
        let indent = body.len() - body.trim_start_matches(' ').len();
        let name = body
            .split_whitespace()
            .next()
            .ok_or_else(|| format!("an empty line of the tree: `{line}`"))?;
        if indent % 2 != 0 {
            return Err(format!("an odd indent in `{line}`"));
        }
        let depth = indent / 2;
        if depth == 0 {
            if !root_seen {
                if name != pool {
                    return Err(format!("the tree's root is `{name}`, not `{pool}`"));
                }
                root_seen = true;
                classes.push((Class::Data, Vec::new()));
            } else {
                let class =
                    Class::header(name).ok_or_else(|| format!("an unknown class `{name}`"))?;
                classes.push((class, Vec::new()));
            }
            continue;
        }
        let Some((_, top)) = classes.last_mut() else {
            return Err(format!("a vdev before the tree's root: `{line}`"));
        };
        // Walk down to the parent at depth - 1.
        let mut siblings = top;
        for level in 1..depth {
            siblings = &mut siblings
                .last_mut()
                .ok_or_else(|| format!("`{name}` at depth {depth} has no parent at {level}"))?
                .children;
        }
        siblings.push(Entry {
            name: name.to_string(),
            children: Vec::new(),
        });
    }
    if !root_seen {
        return Err("no tree under `config:`".into());
    }
    Ok(Status {
        classes,
        busy,
        removed,
    })
}

/// A GUID as ZFS writes it: unsigned, or signed and taken modulo 2^64.
pub(crate) fn parse_guid(text: &str) -> Option<u64> {
    text.parse::<u64>()
        .ok()
        .or_else(|| text.parse::<i64>().ok().map(|n| n as u64))
}

/// `zpool get -H -p -o property,value ...`'s text.
pub(crate) fn parse_props(text: &str) -> Result<Props, String> {
    let mut values = BTreeMap::new();
    for line in text.lines().filter(|l| !l.is_empty()) {
        let (property, value) = line
            .split_once('\t')
            .ok_or_else(|| format!("a line with no tab: `{line}`"))?;
        if values.insert(property, value).is_some() {
            return Err(format!("`{property}` given twice"));
        }
    }
    let get = |property: &str| {
        values
            .get(property)
            .copied()
            .ok_or_else(|| format!("no `{property}`"))
    };
    let number = |property: &str| {
        let value = get(property)?;
        parse_guid(value).ok_or_else(|| format!("`{property}` is `{value}`, not a number"))
    };
    let switch = |property: &str| match get(property)? {
        "on" => Ok(true),
        "off" => Ok(false),
        value => Err(format!("`{property}` is `{value}`, not on or off")),
    };
    let checkpoint = match get("checkpoint")? {
        "-" | "0" => false,
        value if value.parse::<u64>().is_ok() => true,
        value => return Err(format!("`checkpoint` is `{value}`")),
    };
    Ok(Props {
        guid: number("guid")?,
        load_guid: number("load_guid")?,
        multihost: switch("multihost")?,
        checkpoint,
        readonly: switch("readonly")?,
    })
}

/// The object under `return` in `zfs program -j`'s output, which must
/// be UTF-8.
fn readable(out: &[u8]) -> Result<serde_json::Map<String, Value>, String> {
    let text = std::str::from_utf8(out).map_err(|_| "its output is not UTF-8".to_string())?;
    returned(text)
}

/// The object under `return` in `zfs program -j`'s output.
pub(crate) fn returned(text: &str) -> Result<serde_json::Map<String, Value>, String> {
    let value: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    match value.get("return") {
        Some(Value::Object(map)) => Ok(map.clone()),
        Some(other) => Err(format!("it returned {other}, not a table")),
        None => Err("no `return` in its output".into()),
    }
}

/// A table of a channel program's table: an object, or an empty list for
/// an empty table.
fn table<'a>(
    map: &'a serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<&'a serde_json::Map<String, Value>>, String> {
    match map.get(key) {
        Some(Value::Object(table)) => Ok(Some(table)),
        Some(Value::Array(list)) if list.is_empty() => Ok(None),
        _ => Err(format!("`{key}` is not a table")),
    }
}

/// Root first, then sorted by name: the order of the manifest's members.
fn member_order(pool: &str, a: &str, b: &str) -> std::cmp::Ordering {
    (a != pool, a).cmp(&(b != pool, b))
}

/// The most bytes a property's value can have: `ZFS_MAXPROPLEN` less
/// its terminating NUL.
pub const VALUE_MOST: usize = 8191;

/// Properties that `zfs get -s local` lists but the manifest does not
/// carry. `filesystem_count` and `snapshot_count` are counters, not
/// settings: libzfs gives them the source `local` wherever ZFS keeps
/// them (`get_numeric_property` in `libzfs_dataset.c`: "not default,
/// must be local"), which it does once a limit is set on the dataset or
/// above it. `snapshot_count` grows with the take itself, so that the
/// reads before and after it would never agree (Codex on #157).
const COUNTERS: [&str; 2] = ["filesystem_count", "snapshot_count"];

/// Whether the manifest carries a property that `zfs get -s local`
/// lists: not the agent's own (it is not the pool's), not a counter.
fn carried(property: &str) -> bool {
    property != ASSIST_PROPERTY && !COUNTERS.contains(&property)
}

/// The columns of `zfs get` that list the local properties.
const PAIRS: &str = "name,property";

/// The columns of `zfs get` that read their values.
const VALUES: &str = "name,property,value";

/// `zfs list` of every dataset and snapshot under `pool`, with the
/// columns [`parse_listed`] reads.
fn list_words(pool: &str) -> [&str; 9] {
    [
        "list",
        "-r",
        "-H",
        "-p",
        "-t",
        "filesystem,volume,snapshot",
        "-o",
        "name,guid,type,createtxg",
        pool,
    ]
}

/// `zfs get` of every local property of the filesystems and volumes
/// under `pool`, with the columns `columns`.
fn get_words<'a>(pool: &'a str, columns: &'a str) -> [&'a str; 12] {
    [
        "get",
        "-r",
        "-H",
        "-p",
        "-s",
        "local",
        "-t",
        "filesystem,volume",
        "-o",
        columns,
        "all",
        pool,
    ]
}

/// The lines of a command's output: split at each newline, the empty
/// line after the last one left out.
fn output_lines(out: &[u8]) -> Vec<&[u8]> {
    let mut lines: Vec<&[u8]> = out.split(|&b| b == b'\n').collect();
    if lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines
}

/// `zfs list -r -H -p -t filesystem,volume,snapshot -o
/// name,guid,type,createtxg POOL`'s output: the filesystems and volumes
/// by name, each with its `@t` where it has one, and no property read
/// yet. The other snapshots are left out. A name cannot hold a tab or a
/// newline, so each line is split at its tabs.
pub(crate) fn parse_listed(
    out: &[u8],
    pool: &str,
    t: &str,
) -> Result<BTreeMap<String, Dataset>, String> {
    let text = std::str::from_utf8(out).map_err(|_| "its output is not UTF-8".to_string())?;
    let mut datasets = BTreeMap::new();
    let mut snaps = Vec::new();
    for line in text.split_terminator('\n') {
        let fields: Vec<&str> = line.split('\t').collect();
        let [name, guid, kind, createtxg] = fields[..] else {
            return Err(format!("a line of {} fields: `{line}`", fields.len()));
        };
        let guid = parse_guid(guid).ok_or_else(|| format!("`{name}`: its GUID is `{guid}`"))?;
        let createtxg = createtxg
            .parse::<u64>()
            .map_err(|_| format!("`{name}`: its createtxg is `{createtxg}`"))?;
        let kind = match kind {
            "filesystem" => Kind::Filesystem,
            "volume" => Kind::Volume,
            "snapshot" => {
                let (of, snap) = name
                    .split_once('@')
                    .ok_or_else(|| format!("the snapshot `{name}` has no `@`"))?;
                if snap == t {
                    snaps.push((of.to_string(), Snap { guid, createtxg }));
                }
                continue;
            }
            kind => return Err(format!("`{name}` is a {kind}")),
        };
        let dataset = Dataset {
            name: name.to_string(),
            kind,
            guid,
            createtxg,
            local: BTreeMap::new(),
            at: None,
        };
        if datasets.insert(name.to_string(), dataset).is_some() {
            return Err(format!("`{name}` is listed twice"));
        }
    }
    if !datasets.contains_key(pool) {
        return Err(format!("`{pool}` itself is not listed"));
    }
    for (of, snap) in snaps {
        let dataset = datasets
            .get_mut(&of)
            .ok_or_else(|| format!("`{of}@{t}` is listed and `{of}` is not"))?;
        dataset.at = Some(snap);
    }
    Ok(datasets)
}

/// `zfs get -r -H -p -s local -t filesystem,volume -o name,property all
/// POOL`'s output: each local property, by dataset, in the order listed.
/// Neither a name nor a property's name can hold a tab or a newline, so
/// each line is one tab between the two.
pub(crate) fn parse_pairs(out: &[u8]) -> Result<Vec<(String, String)>, String> {
    let text = std::str::from_utf8(out).map_err(|_| "its output is not UTF-8".to_string())?;
    let mut seen = BTreeSet::new();
    let mut pairs = Vec::new();
    for line in text.split_terminator('\n') {
        let pair = match line.split_once('\t') {
            Some((name, property))
                if !name.is_empty() && !property.is_empty() && !property.contains('\t') =>
            {
                (name.to_string(), property.to_string())
            }
            _ => {
                return Err(format!(
                    "a line that is not a name and a property: `{line}`"
                ))
            }
        };
        if !seen.insert(pair.clone()) {
            return Err(format!("`{}` of `{}` is listed twice", pair.1, pair.0));
        }
        pairs.push(pair);
    }
    Ok(pairs)
}

/// Values read, by dataset and property.
pub(crate) type Values = BTreeMap<(String, String), Vec<u8>>;

/// A line of `zfs get ... -o name,property,value`: split at its first
/// two tabs only, since a value may hold tabs. `None` where it is not
/// UTF-8 before its second tab or has fewer than two.
fn value_line(line: &[u8]) -> Option<(String, String, Vec<u8>)> {
    let mut parts = line.splitn(3, |&b| b == b'\t');
    let name = std::str::from_utf8(parts.next()?).ok()?;
    let property = std::str::from_utf8(parts.next()?).ok()?;
    let value = parts.next()?;
    Some((name.to_string(), property.to_string(), value.to_vec()))
}

/// The values read by `zfs get ... -o name,property,value` against the
/// pairs `zfs get ... -o name,property` listed ([module docs](self), "The
/// manifest's read"): each value whose line can be trusted, and the
/// datasets whose values must be read again one at a time.
///
/// A value may hold a newline, which `zfs get -H` prints as it is: the
/// value's line then ends early, and what follows it is a line of its
/// own. Where the lines name exactly the pairs listed, in their order,
/// no value held a newline (a newline adds a line), and every value is
/// taken. Otherwise a line is suspect where it does not split, or names
/// a pair not listed, or one that another line names too, or holds more
/// than [`VALUE_MOST`] bytes. Read again are the datasets that a suspect
/// line names; that of the nearest line before each suspect line that
/// splits (the line whose value went on); those of the
/// pairs listed that no line names; and, while the lines that are left
/// are not the pairs left in their order, the two datasets where they
/// first differ.
pub(crate) fn match_values(pairs: &[(String, String)], out: &[u8]) -> (Values, BTreeSet<String>) {
    let lines: Vec<_> = output_lines(out).into_iter().map(value_line).collect();
    let listed: BTreeSet<(&str, &str)> = pairs
        .iter()
        .map(|(name, property)| (name.as_str(), property.as_str()))
        .collect();
    let mut named: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for (name, property, _) in lines.iter().flatten() {
        *named.entry((name.as_str(), property.as_str())).or_default() += 1;
    }
    let suspect: Vec<bool> = lines
        .iter()
        .map(|line| match line {
            None => true,
            Some((name, property, value)) => {
                let pair = (name.as_str(), property.as_str());
                !listed.contains(&pair)
                    || named.get(&pair).copied().unwrap_or(0) != 1
                    || value.len() > VALUE_MOST
            }
        })
        .collect();
    let mut again = BTreeSet::new();
    for (i, line) in lines.iter().enumerate() {
        if !suspect[i] {
            continue;
        }
        if let Some((name, _, _)) = line {
            again.insert(name.clone());
        }
        // The line whose value went on into this one: the nearest before
        // it that split. (A run of suspect lines comes from the line
        // before the run's first, which that first line finds.)
        if let Some((name, _, _)) = lines[..i].iter().rev().flatten().next() {
            again.insert(name.clone());
        }
    }
    for (name, property) in pairs {
        if !named.contains_key(&(name.as_str(), property.as_str())) {
            again.insert(name.clone());
        }
    }
    // The lines left must be the pairs left, in their order: where they
    // first differ, both datasets are read again, until they agree.
    loop {
        let read_left = lines
            .iter()
            .zip(&suspect)
            .filter_map(|(line, suspect)| match line {
                Some((name, property, _)) if !*suspect && !again.contains(name) => {
                    Some((name.as_str(), property.as_str()))
                }
                _ => None,
            });
        let left = pairs
            .iter()
            .filter(|(name, _)| !again.contains(name))
            .map(|(name, property)| (name.as_str(), property.as_str()));
        let mut read_left = read_left.fuse();
        let mut left = left.fuse();
        let differ = loop {
            match (read_left.next(), left.next()) {
                (None, None) => break None,
                (a, b) if a == b => continue,
                (a, b) => break Some((a.map(|(n, _)| n), b.map(|(n, _)| n))),
            }
        };
        let Some((a, b)) = differ else { break };
        let names: Vec<String> = [a, b].into_iter().flatten().map(str::to_string).collect();
        again.extend(names);
    }
    let mut read = BTreeMap::new();
    for (line, suspect) in lines.into_iter().zip(suspect) {
        if let Some((name, property, value)) = line {
            if !suspect && !again.contains(&name) {
                read.insert((name, property), value);
            }
        }
    }
    (read, again)
}

/// What the members program returned: `None` where `pool@t` does not
/// exist.
pub(crate) fn parse_members(
    returned: &serde_json::Map<String, Value>,
    pool: &str,
) -> Result<Option<Vec<Member>>, String> {
    if returned.contains_key("absent") {
        return Ok(None);
    }
    let members = table(returned, "members")?.ok_or("no members, and pool@t not absent")?;
    let mut out = Vec::with_capacity(members.len());
    for (name, guid) in members {
        let text = guid
            .as_str()
            .ok_or_else(|| format!("`{name}`'s GUID is not a string"))?;
        out.push(Member {
            name: name.clone(),
            snapshot_guid: parse_guid(text)
                .ok_or_else(|| format!("`{name}`'s GUID is `{text}`"))?,
        });
    }
    out.sort_by(|a, b| member_order(pool, &a.name, &b.name));
    if out.first().map(|m| m.name.as_str()) != Some(pool) {
        return Err(format!(
            "pool@t exists and `{pool}` is not among the members"
        ));
    }
    Ok(Some(out))
}

#[cfg(test)]
mod recordings;
#[cfg(test)]
mod tests;
