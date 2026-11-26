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
//! - [`Zfs::members`] and [`Zfs::tagged`]: read-only channel programs
//!   (`zfs program -n -j`), `members.lua` and `tagged.lua`, beside this
//!   file, on their standard input.
//! - [`Zfs::datasets`]: `members.lua` in full (`zfs program -n -j POOL -
//!   POOL T full skyzfs:assist`).
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
//! The manifest is read after the take, by the members program in full
//! ([`Zfs::datasets`]), which only reads: every dataset's type, GUID,
//! `createtxg` and locally set properties, and its `@t`'s GUID and
//! `createtxg`. The properties are read from the datasets, as they are
//! at the read: ZFS keeps on a snapshot only the properties set on the
//! snapshot itself (`dsl_prop_get_ds` reads any other through the
//! dataset's directory).
//!
//! The programs' values are all strings (`tostring`), and its tables
//! are keyed by name: `zfs program -j` writes a Lua number as a signed
//! 64-bit one (a GUID above 2^63 negative) and a list as an object keyed
//! by position, so names and strings are what reads the same on every
//! version. A GUID is read as unsigned, or as signed and taken modulo
//! 2^64.
//!
//! A property's value that is not printable ASCII comes in hex: `{
//! "encoding": "hex", "value": "..." }`, lower case, as the manifest
//! writes it ([`crate::manifest`]). `zfs program -j` prints strings
//! through `mbrtowc`, which in the locale `C` stops at a byte above 0x7f:
//! it cuts the output there, and still exits 0 (B2 of the review of
//! #147). Names are ASCII by ZFS's rules; values are not.
//!
//! Output that cannot be read after the program exited 0 (cut off, not
//! UTF-8, or not a whole result) is an error: the programs only read.
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
//! - The members and tagged programs run and return what the parsers
//!   read, on the rig's pool and on a mirror of two files.
//! - The channel-program sandbox has no `pcall` (a take program of
//!   `366b217`, since dropped, failed on it on both images). The members
//!   program in full has not run on ZFS, nor has the count of the
//!   datasets (`zfs list -r -H -o name -t filesystem,volume`).
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
//! - Property values that are not printable ASCII, in hex as above.
//! - The time limits ([`Limits`]): a read 60 s plus 180 ms, a change
//!   120 s plus 100 ms, for each of the pool's datasets as last counted,
//!   at most 35 minutes; the count itself 60 s, and a count that fails
//!   leaves the bases. A read's 180 ms puts its limit at 10 001
//!   datasets (1860.2 s) above twice the full read measured there on
//!   Ubuntu 26.04 (911.8 s, run 45720225770; the lead's ruling on N2 of
//!   #153's review), and the cap above that. They are provisional (the
//!   lead's rule on M2.6c-2b, from testing's figures in #152: at 10 001
//!   file systems on a slow runner `zfs snapshot -r` took 391 s under a
//!   heavy write load, and `tagged.lua` 68.7 s with none). Correctness
//!   does not rest on them: a take killed at its limit is adopted at a
//!   later round (above).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use super::{Dataset, Held, Layout, Props, Snap, Vdev, VdevKind, Zfs, ZfsError, ASSIST_PROPERTY};
use crate::host::{Host, Node, Output};
use crate::manifest::{self, Kind};
use crate::record::Member;

/// The module parameter that says whether ZFS sends cache flushes.
pub const NOCACHEFLUSH: &str = "/sys/module/zfs/parameters/zfs_nocacheflush";

/// The channel program that reads each member's `@t`, or in full every
/// dataset ([module docs](self)).
pub const MEMBERS_PROGRAM: &str = include_str!("cli/members.lua");

/// The channel program that finds the snapshots with a hold.
pub const TAGGED_PROGRAM: &str = include_str!("cli/tagged.lua");

/// The most instructions a channel program is let run: the default of
/// `zfs_lua_max_instrlim`.
pub const PROGRAM_INSTRUCTIONS: u64 = 100_000_000;

/// The most memory a channel program is let use: the default of
/// `zfs_lua_max_memlimit`.
pub const PROGRAM_MEMORY: u64 = 100 << 20;

/// How long a command may run before it is killed. A read takes
/// [`Limits::read_per_dataset`] more, and a change
/// [`Limits::per_dataset`] more, for each of the pool's datasets as
/// [`Zfs::count`] last read them, up to [`Limits::most`] (the lead's rule
/// on M2.6c-2b; provisional).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// A command that reads, on a pool of no datasets: `zpool status`,
    /// `zpool get`, `zfs holds`, a read-only program.
    pub read: Duration,
    /// A command that changes the pool and waits for a group to be
    /// written, on a pool of no datasets: `zfs snapshot -r`, `zfs
    /// destroy -r`, `zfs hold`, `zfs release`, `zfs set`, `zpool sync`.
    pub change: Duration,
    /// What each of the pool's datasets adds to a change.
    pub per_dataset: Duration,
    /// What each of the pool's datasets adds to a read: rounded up so
    /// that a read's limit at 10 001 datasets (1860.2 s) is at least
    /// twice the full read measured there (testing's run 45720225770 on
    /// Ubuntu 26.04: `members.lua` in full 911.8 s; the lead's ruling on
    /// N2 of #153's review).
    pub read_per_dataset: Duration,
    /// The most a read or a change is let run, however many datasets: at
    /// least a read's limit at 10 001 datasets (1860.2 s), which the
    /// first cap of 20 minutes was not.
    pub most: Duration,
    /// The count of the datasets itself ([`Zfs::count`]), whatever their
    /// number.
    pub count: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            read: Duration::from_secs(60),
            change: Duration::from_secs(120),
            per_dataset: Duration::from_millis(100),
            read_per_dataset: Duration::from_millis(180),
            most: Duration::from_secs(35 * 60),
            count: Duration::from_secs(60),
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
    /// The pool's datasets as last counted.
    Datasets(u64),
}

/// [`Zfs`] on the commands ([module docs](self)).
#[derive(Debug)]
pub struct ZfsCli<H> {
    host: H,
    limits: Limits,
    scale: Scale,
}

impl<H: Host> ZfsCli<H> {
    /// The commands on `host`, each within `limits`.
    pub fn new(host: H, limits: Limits) -> Self {
        ZfsCli {
            host,
            limits,
            scale: Scale::Base,
        }
    }

    /// The limit of a command whose base is `base`, `per` more for each
    /// dataset counted ([`Limits`]).
    fn scaled(&self, base: Duration, per: Duration) -> Duration {
        let limit = match self.scale {
            Scale::Base => base,
            Scale::Datasets(n) => {
                let more = u32::try_from(n)
                    .ok()
                    .and_then(|n| per.checked_mul(n))
                    .unwrap_or(self.limits.most);
                base.saturating_add(more)
            }
        };
        limit.min(self.limits.most)
    }

    /// The limit of a command that reads.
    fn read_limit(&self) -> Duration {
        self.scaled(self.limits.read, self.limits.read_per_dataset)
    }

    /// The limit of a command that changes the pool.
    fn change_limit(&self) -> Duration {
        self.scaled(self.limits.change, self.limits.per_dataset)
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

    /// Run `program` with `args` within `limit`; its standard output
    /// where it exits 0, and otherwise the error its message says.
    fn run(
        &mut self,
        program: &str,
        args: &[&str],
        stdin: &[u8],
        limit: Duration,
        snapshot: bool,
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
        Err(failure(program, args, &output, snapshot))
    }

    fn zpool(&mut self, args: &[&str], limit: Duration) -> Result<String, ZfsError> {
        self.run("zpool", args, &[], limit, false)
    }

    fn zfs(&mut self, args: &[&str], limit: Duration, snapshot: bool) -> Result<String, ZfsError> {
        self.run("zfs", args, &[], limit, snapshot)
    }

    /// A channel program's returned value, the object under `return`.
    fn program(
        &mut self,
        pool: &str,
        text: &str,
        args: &[&str],
        read_only: bool,
    ) -> Result<serde_json::Map<String, Value>, ZfsError> {
        let out = self.program_output(pool, text, args, read_only)?;
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
        match failure("zfs", &args, &output, true) {
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
        let holds = match self.zfs(&["holds", "-H", "-r", snapshot], self.read_limit(), true) {
            Ok(holds) => holds,
            Err(ZfsError::NoSnapshot) => return Ok(None),
            Err(e) => return Err(e),
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

    /// A channel program's standard output, where it exits 0.
    fn program_output(
        &mut self,
        pool: &str,
        text: &str,
        args: &[&str],
        read_only: bool,
    ) -> Result<Vec<u8>, ZfsError> {
        let instructions = PROGRAM_INSTRUCTIONS.to_string();
        let memory = PROGRAM_MEMORY.to_string();
        let mut all = vec!["program"];
        if read_only {
            all.push("-n");
        }
        all.extend(["-j", "-t", &instructions, "-m", &memory, pool, "-"]);
        all.extend(args);
        let limit = if read_only {
            self.read_limit()
        } else {
            self.change_limit()
        };
        let output = self.exec("zfs", &all, text.as_bytes(), limit)?;
        if output.code == Some(0) {
            Ok(output.stdout)
        } else {
            Err(failure("zfs", &all, &output, false))
        }
    }

    /// The tree of `zpool status -P` (`guids` false) or `-P -g` (true).
    fn status(&mut self, pool: &str, guids: bool) -> Result<Status, ZfsError> {
        let mut args = vec!["status", "-P"];
        if guids {
            args.push("-g");
        }
        args.push(pool);
        let out = self.zpool(&args, self.read_limit())?;
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
            let error = failure("zpool", &args, &output, false);
            // ZFS 2.2's `zpool get` takes a name that is no pool's for no
            // pool named at all ([module docs](self)): `zpool status`
            // says whether the pool is there.
            if matches!(error, ZfsError::Other(_)) && says(&output.stderr, "missing pool name") {
                if let Err(ZfsError::NoPool) =
                    self.zpool(&["status", "-P", pool], self.read_limit())
                {
                    return Err(ZfsError::NoPool);
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
            .run("zfs", &args, &[], self.limits.count, false)
            .map(|out| out.lines().filter(|line| !line.is_empty()).count() as u64);
        self.scale = match counted {
            Ok(n) => Scale::Datasets(n),
            Err(_) => Scale::Base,
        };
        counted
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
        Err(failure("zfs", &args, &output, false))
    }

    fn datasets(&mut self, pool: &str, t: &str) -> Result<Vec<Dataset>, ZfsError> {
        let returned = self.program(
            pool,
            MEMBERS_PROGRAM,
            &[pool, t, "full", ASSIST_PROPERTY],
            true,
        )?;
        parse_datasets(&returned)
            .map_err(|why| ZfsError::Other(format!("zfs program (members, full): {why}")))
    }

    fn destroy(&mut self, pool: &str, t: &str) -> Result<(), ZfsError> {
        self.destroy_r(pool, t)
    }

    fn members(&mut self, pool: &str, t: &str) -> Result<Option<Vec<Member>>, ZfsError> {
        let returned = self.program(pool, MEMBERS_PROGRAM, &[pool, t], true)?;
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
            return Err(failure("zfs", &args, &output, true));
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
            return match failure("zfs", &args, &output, true) {
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
        let Some(tagged) = self.tagged_snapshots(&snapshot, tag)? else {
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
        let returned = self.program(pool, TAGGED_PROGRAM, &[pool, tag], true)?;
        match returned.get("tagged") {
            Some(Value::Object(names)) => Ok(names.keys().cloned().collect()),
            // An empty table: ZFS may write it as an empty list.
            Some(Value::Array(names)) if names.is_empty() => Ok(BTreeSet::new()),
            _ => Err(ZfsError::Other(
                "zfs program (tagged): no `tagged` object returned".into(),
            )),
        }
    }

    fn assist(&mut self, pool: &str, value: u64) -> Result<(), ZfsError> {
        let set = format!("{ASSIST_PROPERTY}={value}");
        self.zfs(&["set", &set, pool], self.change_limit(), false)?;
        self.zpool(&["sync", pool], self.change_limit())?;
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

/// The error a command that did not exit 0 gives, by its message.
fn failure(program: &str, args: &[&str], output: &Output, snapshot: bool) -> ZfsError {
    if says(&output.stderr, "no such pool") {
        return ZfsError::NoPool;
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

/// A string of a channel program's table.
fn string<'a>(map: &'a serde_json::Map<String, Value>, key: &str) -> Result<&'a str, String> {
    map.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("`{key}` is not a string"))
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

/// A local property's value as the members program gives it ([module
/// docs](self)): printable ASCII as a string, any other value as its
/// lower-case hex under `encoding`. Anything the program never gives (a
/// string that is not printable ASCII, hex of the wrong case or of a
/// printable value) is refused.
fn local_value(value: &Value) -> Result<Vec<u8>, String> {
    match value {
        Value::String(text) if manifest::plain(text.as_bytes()) => Ok(text.as_bytes().to_vec()),
        Value::String(_) => Err("is a string that is not printable ASCII".into()),
        Value::Object(map) => {
            if map.len() != 2 || string(map, "encoding")? != "hex" {
                return Err("is not a value in hex".into());
            }
            let hex = string(map, "value")?;
            match manifest::unhex(hex) {
                Some(bytes) if manifest::hex(&bytes) == hex && !manifest::plain(&bytes) => {
                    Ok(bytes)
                }
                _ => Err(format!(
                    "is `{hex}`, not the lower-case hex of a value that is not printable"
                )),
            }
        }
        _ => Err("is not a string or a value in hex".into()),
    }
}

/// Root first, then sorted by name: the order of the manifest's members.
fn member_order(pool: &str, a: &str, b: &str) -> std::cmp::Ordering {
    (a != pool, a).cmp(&(b != pool, b))
}

/// What the members program returned in full ([`Zfs::datasets`]), by
/// name.
pub(crate) fn parse_datasets(
    returned: &serde_json::Map<String, Value>,
) -> Result<Vec<Dataset>, String> {
    let datasets = table(returned, "datasets")?.ok_or("no datasets")?;
    let mut out = Vec::with_capacity(datasets.len());
    for (name, value) in datasets {
        let Value::Object(dataset) = value else {
            return Err(format!("the dataset `{name}` is not a table"));
        };
        let number = |key: &str, guid: bool| {
            let text = string(dataset, key)?;
            let n = if guid {
                parse_guid(text)
            } else {
                text.parse::<u64>().ok()
            };
            n.ok_or_else(|| format!("`{name}`: `{key}` is `{text}`"))
        };
        let kind = match string(dataset, "type")? {
            "filesystem" => Kind::Filesystem,
            "volume" => Kind::Volume,
            kind => return Err(format!("`{name}` is a {kind}")),
        };
        let mut local = BTreeMap::new();
        if let Some(set) = table(dataset, "local")? {
            for (property, value) in set {
                let value =
                    local_value(value).map_err(|why| format!("`{name}`: `{property}` {why}"))?;
                local.insert(property.clone(), value);
            }
        }
        // The agent's own property, should a program let it through.
        local.remove(ASSIST_PROPERTY);
        let at = match (
            dataset.contains_key("snapshot_guid"),
            dataset.contains_key("snapshot_createtxg"),
        ) {
            (false, false) => None,
            (true, true) => Some(Snap {
                guid: number("snapshot_guid", true)?,
                createtxg: number("snapshot_createtxg", false)?,
            }),
            _ => return Err(format!("`{name}`: its snapshot is read in part")),
        };
        out.push(Dataset {
            name: name.clone(),
            kind,
            guid: number("guid", true)?,
            createtxg: number("createtxg", false)?,
            local,
            at,
        });
    }
    Ok(out)
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
