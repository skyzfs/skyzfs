//! The command line of `skyzfsd`, and its exit statuses.
//!
//! ```text
//! skyzfsd create --dir DIR --size BYTES [--run-dir DIR] [--zero-fill]
//!                [--endpoint URL --bucket BUCKET --volume NAME]
//! skyzfsd serve  --dir DIR [--run-dir DIR] [--acknowledge-failure] [--kernel-has-async-delete]
//!                [--queues 1] [--depth N] [--io-threads N] [--hold-cuts]
//!                [--intent-region-mib N] [--no-intent-header-sync]
//!                [--holder NAME] [--lease-ttl-s N] [--serve-without-upload]
//! skyzfsd detach --dir DIR [--run-dir DIR] [--kernel-has-async-delete]
//! skyzfsd status --dir DIR [--run-dir DIR]
//! skyzfsd help
//! ```
//!
//! `--queues` takes only 1: this milestone serves one queue.
//!
//! There is no choice of a store: M1.3t removed the log store, and
//! M1.3t-d the option `--store` that was left of the choice. Given, it
//! is an option serve does not have.
//!
//! An option is `--name value` or `--name=value`, and is given at most
//! once. Numbers are decimal. Anything else is a usage error, which is
//! exit status 2 and changes nothing.
//!
//! # Options that are for tests
//!
//! Two overrides of the deployment rules (design §4.1), named so that
//! nobody uses them in production by accident:
//! `--unsafe-allow-unlocked-memory` and `--unsafe-allow-any-filesystem`,
//! on `serve`.
//!
//! And the fault hooks of the kernel rig, which exist only in a build
//! with the cargo feature `test-hooks`. In a default build each of them
//! is a usage error:
//!
//! - `create --base-pattern SEED`: a base that is not zeros;
//! - `serve --fail-log-after N`: the volume fails, as after a failed
//!   sync of its image, once N requests that change it are completed:
//!   writes, write-zeroes and discards. Reads and flushes do not count.
//!   (The name is the log store's, which the rigs still use.);
//! - `serve --hold-one-request SECONDS:read:OFFSET`,
//!   `SECONDS:write:OFFSET` or `SECONDS:flush`: the first request of
//!   that kind that covers that byte is held for so long;
//! - `serve --boot-id-file PATH`: read the boot id from this file and
//!   not from the kernel;
//! - `serve --commit-delay-ms N`: every request's result is given to
//!   the kernel N ms after the store completed it. Nothing else waits;
//! - `serve --request-log PATH`: every request the kernel hands over
//!   and every answer it is given is appended to `PATH`, one line each
//!   (the format is in `record.rs`). With
//!   `--request-log-content whole`, every write's bytes are appended to
//!   `PATH.data` as well; `hash`, the default, keeps only a hash of each
//!   block a write covers;
//! - `serve` takes `SIGUSR2` as a command about held cuts, read from the
//!   volume's run directory (`cut_commands.rs`).

use std::path::PathBuf;
use std::time::Duration;

use crate::remote::Remote;
use skyzfs_local::inplace::IoMode;
use skyzfs_ublk::features::ASYNC_DELETE_OPTION;
use std::process::ExitCode;

/// Where the daemon keeps its notes and records unless told otherwise.
pub const DEFAULT_RUN_DIR: &str = "/run/skyzfsd";

/// The depth of a queue unless told otherwise: the contract's bound on
/// requests in flight.
pub const DEFAULT_DEPTH: u16 = 64;

/// The most a ublk queue takes (`UBLK_MAX_QUEUE_DEPTH`).
pub const MAX_DEPTH: u16 = 4096;

/// The most queues `serve` takes: one. M1 serves one queue; the option
/// is there so that it need not be added later, and any other number
/// is a usage error until more are served.
pub const MAX_QUEUES: u16 = 1;

/// The most I/O threads `--io-threads` gives the in-place store: the
/// store's own bound (`IoMode::MAX_THREADS`).
pub const MAX_IO_THREADS: u16 = io_threads(IoMode::MAX_THREADS);
/// The in-place store's I/O threads unless `--io-threads` is given: the
/// store's own default (`IoMode::DEFAULT_THREADS`). It rests on the
/// benchmark's slow-sync machines only and may change after a round on
/// the merged daemon; it is that one constant, and the help says it
/// (a test holds the two together).
pub const DEFAULT_IO_THREADS: u16 = io_threads(IoMode::DEFAULT_THREADS);
/// The longest holder's name `--holder` takes, in bytes: what the
/// status file's text holds.
pub const MAX_HOLDER: usize = 128;
/// A region of the intent map unless `--intent-region-mib` is given, in
/// MiB: the store's default.
pub const DEFAULT_INTENT_REGION_MIB: u64 = skyzfs_local::inplace::DEFAULT_INTENT_REGION >> 20;
/// The smallest region `--intent-region-mib` takes: 1 MiB, above the
/// store's own smallest.
pub const MIN_INTENT_REGION_MIB: u64 = 1;
/// The largest region `--intent-region-mib` takes: the store's.
pub const MAX_INTENT_REGION_MIB: u64 = skyzfs_local::inplace::MAX_INTENT_REGION >> 20;

/// A count of the store's I/O threads as the command line takes it;
/// one that does not fit does not compile.
const fn io_threads(n: usize) -> u16 {
    assert!(n <= u16::MAX as usize, "more I/O threads than a u16");
    n as u16
}

/// One mebibyte: a volume's size is a multiple of it.
const MIB: u64 = 1 << 20;

/// What is printed with a usage error ([`usage`]).
pub const USAGE: &str = "usage:
  skyzfsd create --dir DIR --size BYTES [--run-dir DIR] [--zero-fill]
                 [--endpoint URL --bucket BUCKET --volume NAME]
  skyzfsd serve  --dir DIR [--run-dir DIR] [--acknowledge-failure] [--queues 1] [--depth N]
                 [--io-threads N] [--hold-cuts] [--intent-region-mib N] [--no-intent-header-sync]
                 [--holder NAME] [--lease-ttl-s N] [--serve-without-upload]
                 [--kernel-has-async-delete]
                 (one queue is all this version serves)
  skyzfsd detach --dir DIR [--run-dir DIR] [--kernel-has-async-delete]
  skyzfsd status --dir DIR [--run-dir DIR]
  skyzfsd cuts   --dir DIR [--run-dir DIR]
  skyzfsd publish --dir DIR --name T --attachment HEX --state HEX --after C --group G
                 [--manifest FILE] [--expiry-s N] [--run-dir DIR]
  skyzfsd publish-progress --dir DIR --name T [--run-dir DIR]
  skyzfsd withdraw --dir DIR --name T [--run-dir DIR]
  skyzfsd help";

/// The test hooks, which a build with the feature `test-hooks` takes and
/// lists after [`USAGE`] and [`HELP`], and a default build refuses (see
/// the module documentation). The kernel rig reads this list to see
/// what a build takes (`serve --help`, a usage error, prints it).
pub const HOOKS_USAGE: &str = "test hooks of this build (feature test-hooks):
  skyzfsd create ... [--base-pattern SEED]
  skyzfsd serve  ... [--fail-log-after N] [--hold-one-request SECONDS:read|write:OFFSET]
                 [--hold-one-request SECONDS:flush] [--boot-id-file PATH]
                 [--commit-delay-ms N] [--request-log PATH] [--request-log-content hash|whole]
                 [--refuse-one-commit read|write:OFFSET] [--refuse-one-commit flush]
                 [--refuse-one-fetch start|recovery]
                 [--kill-at-upload put:N|swap:N|manifest:N|publish:N]";

/// What a usage error prints: [`USAGE`], and in a build with the test
/// hooks [`HOOKS_USAGE`] after it.
pub fn usage() -> String {
    with_hooks(USAGE)
}

/// What `help` prints: [`HELP`], and in a build with the test hooks
/// [`HOOKS_USAGE`] after it.
pub fn help() -> String {
    with_hooks(HELP)
}

fn with_hooks(text: &str) -> String {
    if cfg!(feature = "test-hooks") {
        format!("{text}\n\n{HOOKS_USAGE}")
    } else {
        text.to_string()
    }
}

/// What `help` prints: the usage, and what an operator has to know
/// before using a command.
pub const HELP: &str = "skyzfsd: one volume of the local store, served as a ublk device.

  It needs Linux 6.11 or later with the ublk driver: the driver's
  features UBLK_F_USER_RECOVERY, UBLK_F_USER_RECOVERY_REISSUE and
  UBLK_F_CMD_IOCTL_ENCODE, its command UBLK_U_CMD_GET_FEATURES (6.5),
  and its asynchronous delete UBLK_U_CMD_DEL_DEV_ASYNC, without which
  the daemon cannot give up a failed volume's device. The kernel's
  header names that command from 6.9, but its driver executes it from
  6.11 (the drivers of 6.9 and 6.10 answer it ENOTSUPP).
  serve, before it locks the volume, opens its store or adds or takes
  over a device, and detach, once it has the volume's lock and has
  found a record of a device of this boot and before it looks at or
  deletes that device, ask the driver for its features and read the
  kernel's release (uname -r). They refuse, naming what is missing, on
  a kernel that lacks a feature or whose release is below 6.11 or
  does not begin with a major and a minor number. A refused serve
  leaves a device of the volume that waits for a server as it is.
  The asynchronous delete has no feature bit. A kernel whose ublk
  driver executes it under an older release (a distribution kernel
  with the driver backported, or a 6.9.y or 6.10.y that carries the
  fix) is served when the operator says so with
  --kernel-has-async-delete, given to serve and to detach: the release
  is then not read, and the features are still checked. Say it only of
  a kernel whose driver is known to execute the command: nothing
  checks it before a delete needs it, and a delete it does not know
  fails and leaves the device.

  skyzfsd create --dir DIR --size BYTES [--run-dir DIR] [--zero-fill]
                 [--endpoint URL --bucket BUCKET --volume NAME]
      Make a new, empty volume in DIR. DIR is made if it is missing
      (its parent must exist) and must hold no volume and no file but
      what an interrupted create leaves (lock, home.new, home.img,
      volume.new: files under exactly these names are removed, whoever
      put them there); a create that fails removes a DIR it made. The
      file volume, made last, says what the volume is. BYTES is a multiple of 1 MiB. One
      line is printed: the size, the volume's id and the store's
      identity.
      --zero-fill writes the new image with zeros and syncs it, so that
      no later write of the in-place store allocates, and says how long
      that took. Without it the image is a hole, and the first write to
      each block allocates it.
      If a volume in DIR failed in this boot, create warns: the note
      of that failure stays, and counts for the new volume.
      --endpoint URL --bucket BUCKET --volume NAME (all three or none)
      make the volume NAME in object storage first, empty, and write
      its place in the file remote in DIR: serve then uploads the
      pool's cuts to it. NAME is 1 to 63 characters of A-Z, a-z, 0-9,
      '.', '-' and '_', beginning with a letter or a digit. Where the
      volume NAME is there already, create takes it only as a create
      leaves it (the same size, empty, not a clone, never served), and
      otherwise refuses (status 1) and makes nothing in DIR; a create
      of the name under way, or one that stopped before the volume's
      head, counts as a volume of its size, and the refusal names the
      object whose deletion frees the name. The keys
      are AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY where both are
      set and AWS_SESSION_TOKEN is not, else the AWS SDK's own chain
      (which takes all three); they are never written. The region is
      AWS_REGION, or us-east-1 where it is unset. The line
      printed names the volume in object storage. A volume created
      without these options is served with no upload, and cannot be
      given a place later.

  skyzfsd serve --dir DIR [--run-dir DIR] [--acknowledge-failure] [--queues 1] [--depth N]
                [--io-threads N] [--hold-cuts] [--intent-region-mib N] [--no-intent-header-sync]
                [--holder NAME] [--lease-ttl-s N] [--serve-without-upload]
                [--kernel-has-async-delete]
      Serve the volume until SIGTERM. When the device serves, one line
      is printed: ready /dev/ublkbN.
      If another instance holds the volume, serve tries again for 2 s
      (20 times, 100 ms apart) and then exits with status 5.
      If the volume's own device is still shown as live by the kernel
      (the server before this one was killed a moment ago), serve asks
      again for up to 5 s before it gives up with status 5.
      A volume that failed in this boot (a write or sync of its image)
      is not served again (status 4). If no client held its device,
      --acknowledge-failure serves it: what was acknowledged to nobody
      may be gone. If a client held the device, the volume is not
      served again before a reboot, whatever is given.
      One queue is all this version serves.
      The volume is served from its home image alone: each write is
      written where it belongs, and a flush syncs the image. At a power
      cut it keeps what the block contract asks (a volatile-cache
      disk): what a completed flush covered is there, and any other
      block may hold its content as of the last flush, any version
      written since, or a mix of those within the block. A failure is
      final. The start syncs the image and drops the kernel's cache of
      it, so that it serves only what the disk holds: it refuses the
      volume where pages stay cached or the kernel cannot count them
      (cachestat, Linux 6.5), and the first reads after it are cold.
      The image is read and written by direct I/O (O_DIRECT), past the
      kernel's cache: the start refuses a file system that offers none,
      or whose unit for it (STATX_DIOALIGN) is above 4 KiB.
      --io-threads N (1 to 64, 8 unless given) does the reads and
      writes on N threads of their own, so that up to N reach the disk
      at once.
      A volume made by a daemon with a log store (before M1.3t) is
      refused, as is a directory whose file volume is missing or
      damaged: none of these fails the volume. Create it again.
      --hold-cuts holds the pool's newest cuts as they were (design
      §5.2): before a write changes a block a held cut reads, the block
      is copied into copies.img beside the image, up to a bound of
      1/256 of the volume (at least 256 MiB). The cuts' state is kept
      in the run directory's volume directory (cuts.state, on tmpfs),
      allocated and locked in memory at the start: 146 MiB for each
      TiB of the volume, and at least 1 MiB. A start that cannot allocate or lock it is refused (exit
      1) before a device is touched, and says what limit refused it.
      A later start in the same boot takes the state up if the copies
      file syncs and no failure is acknowledged; otherwise it begins a
      new one. Nothing lets a held cut go yet but the test hooks: it is
      off unless given.
      With --hold-cuts the volume also keeps intent.map beside the
      image: a bit for each region, set and synced before the first
      write into a region that was clean since the newest cut
      committed, so that after a restart of the machine, or a failure
      that is acknowledged, the next cut holds only the regions it
      names. --intent-region-mib N (a power of two from 1 to 1024, 4
      unless given) sets the size of a region; only with --hold-cuts.
      A start with another size than the map's counts the whole volume
      once. A start that cannot write and sync the map is refused
      (exit 1); a write that cannot fails the volume. When a cut is
      committed the map's header names it, and a sync of the map begins
      once the header is written, beside the device's requests. The
      release does not wait for it, but one sync of the map runs at a
      time: a write into a region the release has just cleared waits
      for it to end, then for a sync of its own. One that fails fails
      the volume. With --no-intent-header-sync (only with --hold-cuts)
      the header is written without a sync: a power cut that loses it
      costs one whole upload. The status says which (intent_header_sync).
      A volume created with --endpoint (it has the file remote) is
      uploaded: its cuts are held as with --hold-cuts, and a thread
      uploads each to the volume in object storage and lets it go. The
      upload holds the volume's lease there, as NAME (--holder; unless
      given, the host's name, a colon and /etc/machine-id, and a start
      with no machine id is refused), for N seconds (--lease-ttl-s, 3
      to 3600, 30 unless given), renewed every third of that. A NAME
      must mean one daemon: the lease cannot tell two of one name
      apart. Never serve a copy of a volume's files under the name of
      the original, a copy in a cloned VM image included (a clone
      copies the machine id): give the copy its own --holder. The
      lease is taken after the store opens and before a device is
      added or taken over, within 10 s. Where another holder has it,
      serve refuses (status 10) and adds or takes over no device;
      --serve-without-upload serves the volume all the same, with no
      upload. Where object
      storage cannot be reached in time, the volume is served and the
      lease taken when it can. A lease lost while serving stops the
      uploads, and the volume is served on. SIGTERM lets the lease go;
      any other end keeps it until its time runs out. status shows the
      upload. The three options are refused (status 1) for a volume
      with no file remote.

  skyzfsd detach --dir DIR [--run-dir DIR] [--kernel-has-async-delete]
      Delete the volume's device if no server serves it. It never stops
      a daemon: stop the daemon first (SIGTERM).
      Export the pool first. A pool whose device is deleted under it
      stays suspended, and the device keeps its number for as long as
      the pool holds it: detach then waits up to 5 s for the number to
      come free before it says so, with exit status 6.

  skyzfsd status --dir DIR [--run-dir DIR]
      Print the volume's state and the serving daemon's counters, one
      `key value` per line. It opens no store and asks no daemon.
      `cuts` counts the pool's transaction groups whose uberblock the
      daemon wrote and then saw a flush cover, from 0 at each start, and
      `cut_group` is the newest one's group. `cut_opens` says whether an
      import of the device as it was at that cut would open that group
      (`itself`), or open only with a rewind to it (`rewind`), as the
      uberblocks on the device were: the daemon reads them at its start.
      A pool made on a partition of the device, not on the whole device,
      makes no cuts.

  skyzfsd cuts --dir DIR [--run-dir DIR]
  skyzfsd publish --dir DIR --name T --attachment HEX --state HEX --after C --group G
                  [--manifest FILE] [--expiry-s N] [--run-dir DIR]
  skyzfsd publish-progress --dir DIR --name T [--run-dir DIR]
  skyzfsd withdraw --dir DIR --name T [--run-dir DIR]
      Ask the daemon serving the volume, through its channel (the socket
      control in the volume's run directory, made before the daemon
      prints ready; a daemon answers only its own user and root), and print
      its answer, one line. Status 0 for an answer, 1 for a refusal
      (refused CODE TEXT) or no answer.
      cuts prints `cuts attachment=A state=S count=C group=G opens=O`:
      the attachment the daemon serves, the store's cut state (named
      from the daemon's start), its count of cuts, and the newest cut's
      group and whether it opens as itself or with a rewind (none
      before the first).
      publish asks that the first cut of state S counted after C whose
      group is G or later and that opens as itself be published as the
      snapshot snap/T in object storage, with FILE's bytes (at most
      1 MiB; none unless given) as its ZFS manifest. Read A, S and C with
      cuts before taking the pool's snapshot T, and give its createtxg
      as G. It waits at most N seconds (1 to 3600, 600 unless given) for
      such a cut. T is 1 to 128 of A-Z, a-z, 0-9, '.', '-' and '_'.
      `accepted` says only that it pends: publish-progress says how it
      stands (pending, done commit=H, expired, withdrawn, unknown, or
      refused), and withdraw ends it unless it is made already. Asking
      the same publish again is asking it once. A daemon with no place
      in object storage, or serving without upload, refuses each
      request no-upload.

Exit statuses: 0 done; 1 an error; 2 the command line is wrong;
3 the volume failed while it was served, or at its start; 4 refused, the volume failed in this boot; 5 refused,
another instance or a live server holds the volume; 6 detach deleted
the device and a client still holds it open; 7 the kernel's driver broke its rule
(it handed over a tag the queue does not have, or one whose request
was still out):
serve answered nothing more, the volume is not failed, and the device
waits for the next server, as after a kill;
8 the kernel's driver refused an answer of serve's (a commit): serve
answered nothing more, the volume is not failed, and the device waits
for the next server, which is handed again what was not answered;
9 serve gave the device's start up: the kernel refused a first fetch
of the queue, or the start had no answer within 30 s; the volume is not
failed; a device this serve added is deleted, and one it took over is
left waiting for the next server, as after a kill. A supervisor should
retry the serve once; a second 9 is for a person;
10 refused, another holder has the volume's lease in object storage:
no device was added or taken over, and the refusal does not fail the
volume.
After a detach only 0 says that nothing of the volume is left attached
on this node.";

/// The exit statuses. The kernel rig depends on each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// 0: `serve` ended after SIGTERM, or another command did its work.
    Done,
    /// 1: anything not named below.
    Error,
    /// 2: the command line is wrong. Nothing was done.
    Usage,
    /// 3: the volume failed while this instance served it.
    Failed,
    /// 4: refused: the volume failed in this boot.
    RefusedFailed,
    /// 5: refused: another instance holds the volume.
    RefusedHeld,
    /// 6: `detach` deleted the volume's device and a client still holds
    /// it open: the device keeps its number until the client lets go.
    DeletedHeld,
    /// 7: `serve` ended because the kernel's driver broke its rule: it
    /// handed over a tag the queue does not have, or one whose request
    /// was still out, when a tag is handed over again only after its
    /// commit (M1.3ac). The
    /// breach is the driver's: nothing more was answered, the volume is
    /// not failed, and the device waits for the next server, as after
    /// a kill.
    BrokenRule,
    /// 8: `serve` ended because the kernel's driver refused one of its
    /// answers (a commit) (M1.3ac part 3). The refused answer took no
    /// effect, so its request is still unanswered: nothing more was
    /// answered, the volume is not failed, and the device waits for the
    /// next server, which the kernel hands that request again.
    CommitRefused,
    /// 9: `serve` gave the device's start up (M1.3ah): the kernel's
    /// driver refused a first fetch of the queue, and the start waits
    /// for every tag's fetch with no limit, or the start had no answer
    /// within its limit. The volume is not failed. A device this serve
    /// added is deleted; one it took over is left waiting for the next
    /// server, as after a kill. Retry the serve once; a second 9 is for
    /// a person.
    NotStarted,
    /// 10: `serve` refused: another holder has the volume's lease in
    /// object storage (M2.4c), and `--serve-without-upload` was not
    /// given. The store was opened and closed again; no device was added
    /// or taken over, and the refusal does not fail the volume (an
    /// acknowledging start has deleted the failed volume's waiting device
    /// before the open, as every acknowledging start does).
    LeaseHeld,
}

impl Exit {
    /// The number.
    pub fn status(self) -> u8 {
        match self {
            Exit::Done => 0,
            Exit::Error => 1,
            Exit::Usage => 2,
            Exit::Failed => 3,
            Exit::RefusedFailed => 4,
            Exit::RefusedHeld => 5,
            Exit::DeletedHeld => 6,
            Exit::BrokenRule => 7,
            Exit::CommitRefused => 8,
            Exit::NotStarted => 9,
            Exit::LeaseHeld => 10,
        }
    }

    /// As the process's exit code.
    pub fn code(self) -> ExitCode {
        ExitCode::from(self.status())
    }
}

/// A command line that cannot be understood.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UsageError {
    /// No command, or one that does not exist.
    #[error(
        "expected a command: create, serve, detach, status, cuts, publish, publish-progress or \
         withdraw"
    )]
    Command,
    /// An argument that is not an option of the command.
    #[error("{0}: not an option of this command")]
    Unknown(String),
    /// An option that exists only in a build with the test hooks.
    #[error("{0}: only in a build with the feature test-hooks")]
    TestOnly(String),
    /// An option given twice.
    #[error("{0}: given more than once")]
    Twice(String),
    /// An option without its value, or a flag with one.
    #[error("{0}: {1}")]
    Value(String, &'static str),
    /// A required option is missing.
    #[error("{0}: required")]
    Missing(&'static str),
    /// A value of an option that is refused, and why.
    #[error("{0}")]
    Refused(String),
}

/// `create`: make a new, empty volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Create {
    /// The volume's directory.
    pub dir: PathBuf,
    /// The size in bytes: a multiple of 1 MiB, not zero.
    pub size: u64,
    /// The daemon's run directory: looked at for a failure note, and
    /// not written.
    pub run_dir: PathBuf,
    /// Test hook: a base that is not zeros, from this seed.
    pub base_pattern: Option<u64>,
    /// `--zero-fill`: write the new image with zeros, so that no write
    /// of the in-place store to it allocates.
    pub zero_fill: bool,
    /// `--endpoint --bucket --volume`: the volume's place in object
    /// storage (M2.4c), made there and written in the file `remote`.
    pub remote: Option<Remote>,
}

/// `serve`: serve the volume until SIGTERM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Serve {
    /// The volume's directory.
    pub dir: PathBuf,
    /// The daemon's run directory.
    pub run_dir: PathBuf,
    /// Serve a volume whose failure the operator acknowledges.
    pub acknowledge_failure: bool,
    /// `--kernel-has-async-delete`: the operator says that the kernel's
    /// ublk driver has `UBLK_U_CMD_DEL_DEV_ASYNC` whatever its release
    /// ([`skyzfs_ublk::features::ASYNC_DELETE_OPTION`]).
    pub async_delete_asserted: bool,
    /// Queues of a device this instance adds: 1, the only number this
    /// milestone serves ([`MAX_QUEUES`]). A device that is taken over
    /// keeps its own.
    pub queues: u16,
    /// Depth of each queue of a device this instance adds.
    pub depth: u16,
    /// `--io-threads`: how the store reads and writes (direct I/O on
    /// that many threads).
    pub in_place_io: skyzfs_local::inplace::IoMode,
    /// `--hold-cuts`: the store holds cuts, with their state in the
    /// volume's run directory (M2.2b). Off until the uploader (M2.4)
    /// lets held cuts go.
    pub hold_cuts: bool,
    /// `--intent-region-mib`: the size of a region of the intent map,
    /// in MiB, where it is given (only with `--hold-cuts`; M2.3).
    pub intent_region_mib: Option<u64>,
    /// Whether the intent map's header is synced at each release (M2.4d,
    /// for Q7 of M2.4b): on unless `--no-intent-header-sync` is given,
    /// which is taken only with `--hold-cuts` (the lead's decision after
    /// the bench showed the sync costs nothing that shows).
    pub intent_header_sync: bool,
    /// `--holder`: whose the lease in object storage is, where given
    /// (the host's name and the machine's id otherwise; M2.4c).
    pub holder: Option<String>,
    /// `--lease-ttl-s`: the lease's time in seconds, where given
    /// ([`MIN_LEASE_TTL_S`](crate::upload::MIN_LEASE_TTL_S) to
    /// [`MAX_LEASE_TTL_S`](crate::upload::MAX_LEASE_TTL_S)).
    pub lease_ttl_s: Option<u64>,
    /// `--serve-without-upload`: serve a volume whose lease another
    /// holder has, with no upload.
    pub serve_without_upload: bool,
    /// Run although the memory cannot be locked. For tests.
    pub allow_unlocked_memory: bool,
    /// Take a volume directory on any file system. For tests.
    pub allow_any_filesystem: bool,
    /// The fault hooks.
    pub hooks: Hooks,
}

/// The fault hooks of `serve`: all off in a default build.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hooks {
    /// The volume fails once this many requests that change it
    /// (writes, write-zeroes, discards) are completed.
    pub fail_log_after: Option<u64>,
    /// One request to hold.
    pub hold: Option<Hold>,
    /// Where to read the boot id from.
    pub boot_id_file: Option<PathBuf>,
    /// Every commit is delayed by so many milliseconds.
    pub commit_delay_ms: Option<u64>,
    /// Where the requests and their answers are written down.
    pub request_log: Option<PathBuf>,
    /// `--request-log-content whole`: every write's bytes go to
    /// `PATH.data` beside the request log, and its `applied` line names
    /// where. `hash` (the default): only each block's hash.
    pub request_log_whole: bool,
    /// The request whose commit is sent with no buffer, for the driver
    /// to refuse: the first of its kind ([`crate::refuse`]).
    pub refuse: Option<Held>,
    /// The start at which tag 0's first fetch is sent with no buffer,
    /// for the driver to refuse.
    pub refuse_fetch: Option<FetchRefusal>,
    /// Where the uploader's thread kills the daemon ([`crate::kill`]).
    pub kill_at: Option<KillAt>,
}

/// `--kill-at-upload`: the point of an upload at which the uploader's
/// thread kills the daemon, as `kill -9` would ([`crate::kill`]). N
/// counts from 1, over the serve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillAt {
    /// `put:N`: once the Nth PUT of a segment has landed.
    Put(u64),
    /// `swap:N`: once the Nth commit of a cut has landed (head and the
    /// cut's row swapped), before the cut is let go.
    Swap(u64),
    /// `manifest:N`: once the Nth publish's framed manifest has landed,
    /// before the transaction that makes its ref.
    Manifest(u64),
    /// `publish:N`: once the Nth publish's transaction has landed, before
    /// the uploader knows it.
    Publish(u64),
}

/// `--refuse-one-fetch`: at which start tag 0's first fetch is sent with
/// no buffer. Whether a serve adds its device or takes it over is known
/// only once the lifecycle has found the device: a serve whose start is
/// the other kind refuses no fetch and says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchRefusal {
    /// At the start of a device this serve added (START_DEV).
    Start,
    /// At a take-over (END_USER_RECOVERY).
    Recovery,
}

impl FetchRefusal {
    /// The option's value.
    pub fn name(self) -> &'static str {
        match self {
            FetchRefusal::Start => "start",
            FetchRefusal::Recovery => "recovery",
        }
    }
}

/// One request to hold for a while.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hold {
    /// For how long.
    pub seconds: u64,
    /// Which request.
    pub what: Held,
}

/// The request a [`Hold`] waits for: the first of its kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// A read that covers this byte.
    Read(u64),
    /// A write that covers this byte.
    Write(u64),
    /// A flush.
    Flush,
}

/// `detach` and `status`: which volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    /// The volume's directory.
    pub dir: PathBuf,
    /// The daemon's run directory.
    pub run_dir: PathBuf,
}

/// `detach`: which volume, and what the operator says of the kernel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detach {
    /// The volume.
    pub volume: Volume,
    /// `--kernel-has-async-delete`, as for [`Serve`].
    pub async_delete_asserted: bool,
}

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `create`.
    Create(Create),
    /// `serve`.
    Serve(Serve),
    /// `detach`.
    Detach(Detach),
    /// `status`.
    Status(Volume),
    /// `cuts`, `publish`, `publish-progress` or `withdraw`.
    Ask(Ask),
    /// `help`, `--help` or `-h`: print [`HELP`].
    Help,
}

/// `cuts`, `publish`, `publish-progress` and `withdraw`: a request to
/// the channel of the daemon that serves a volume ([`crate::control`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    /// The volume whose daemon is asked.
    pub volume: Volume,
    /// The request. A publish's manifest length is 0 here: it is the
    /// length of the file `manifest`, read when the request is sent.
    pub request: crate::control::Request,
    /// `--manifest FILE` of `publish`: the manifest's bytes. None: an
    /// empty manifest.
    pub manifest: Option<PathBuf>,
}

impl Command {
    /// The command's name.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Create(_) => "create",
            Command::Serve(_) => "serve",
            Command::Detach(_) => "detach",
            Command::Status(_) => "status",
            Command::Ask(ask) => match ask.request {
                crate::control::Request::Cuts => "cuts",
                crate::control::Request::Publish(_) => "publish",
                crate::control::Request::Progress(_) => "publish-progress",
                crate::control::Request::Withdraw(_) => "withdraw",
            },
            Command::Help => "help",
        }
    }
}

/// Parse the arguments behind the program's name. The test hooks are
/// taken only in a build with the feature `test-hooks`.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, UsageError> {
    parse_with(args, cfg!(feature = "test-hooks"))
}

/// An option of a command: its name, whether it takes a value, and
/// whether it is a test hook.
struct Opt {
    name: &'static str,
    value: bool,
    hook: bool,
}

const fn opt(name: &'static str, value: bool, hook: bool) -> Opt {
    Opt { name, value, hook }
}

const CREATE: &[Opt] = &[
    opt("--dir", true, false),
    opt("--size", true, false),
    opt("--run-dir", true, false),
    opt("--base-pattern", true, true),
    opt("--zero-fill", false, false),
    opt("--endpoint", true, false),
    opt("--bucket", true, false),
    opt("--volume", true, false),
];

const SERVE: &[Opt] = &[
    opt("--dir", true, false),
    opt("--run-dir", true, false),
    opt("--acknowledge-failure", false, false),
    opt(ASYNC_DELETE_OPTION, false, false),
    opt("--queues", true, false),
    opt("--depth", true, false),
    opt("--io-threads", true, false),
    opt("--hold-cuts", false, false),
    opt("--intent-region-mib", true, false),
    opt("--no-intent-header-sync", false, false),
    opt("--holder", true, false),
    opt("--lease-ttl-s", true, false),
    opt("--serve-without-upload", false, false),
    opt("--unsafe-allow-unlocked-memory", false, false),
    opt("--unsafe-allow-any-filesystem", false, false),
    opt("--fail-log-after", true, true),
    opt("--hold-one-request", true, true),
    opt("--boot-id-file", true, true),
    opt("--commit-delay-ms", true, true),
    opt("--request-log", true, true),
    opt("--request-log-content", true, true),
    opt("--refuse-one-commit", true, true),
    opt("--refuse-one-fetch", true, true),
    opt("--kill-at-upload", true, true),
];

const VOLUME: &[Opt] = &[opt("--dir", true, false), opt("--run-dir", true, false)];

const NAMED: &[Opt] = &[
    opt("--dir", true, false),
    opt("--run-dir", true, false),
    opt("--name", true, false),
];

const PUBLISH: &[Opt] = &[
    opt("--dir", true, false),
    opt("--run-dir", true, false),
    opt("--name", true, false),
    opt("--attachment", true, false),
    opt("--state", true, false),
    opt("--after", true, false),
    opt("--group", true, false),
    opt("--manifest", true, false),
    opt("--expiry-s", true, false),
];

const DETACH: &[Opt] = &[
    opt("--dir", true, false),
    opt("--run-dir", true, false),
    opt(ASYNC_DELETE_OPTION, false, false),
];

/// The options given, by name. A flag's value is empty.
struct Given(Vec<(&'static str, String)>);

impl Given {
    fn collect(
        known: &[Opt],
        args: impl Iterator<Item = String>,
        hooks: bool,
    ) -> Result<Self, UsageError> {
        let mut given: Vec<(&'static str, String)> = Vec::new();
        let mut args = args;
        while let Some(arg) = args.next() {
            let (name, inline) = match arg.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (arg.as_str(), None),
            };
            let Some(opt) = known.iter().find(|o| o.name == name) else {
                return Err(UsageError::Unknown(arg));
            };
            if opt.hook && !hooks {
                return Err(UsageError::TestOnly(opt.name.into()));
            }
            if given.iter().any(|(n, _)| *n == opt.name) {
                return Err(UsageError::Twice(opt.name.into()));
            }
            let value = match (opt.value, inline) {
                (true, Some(value)) => value,
                (true, None) => args
                    .next()
                    .ok_or(UsageError::Value(opt.name.into(), "needs a value"))?,
                (false, None) => String::new(),
                (false, Some(_)) => {
                    return Err(UsageError::Value(opt.name.into(), "takes no value"))
                }
            };
            if opt.value && value.is_empty() {
                return Err(UsageError::Value(opt.name.into(), "needs a value"));
            }
            given.push((opt.name, value));
        }
        Ok(Self(given))
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, value)| value.as_str())
    }

    fn flag(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    fn path(&self, name: &'static str) -> Result<PathBuf, UsageError> {
        self.get(name)
            .map(PathBuf::from)
            .ok_or(UsageError::Missing(name))
    }

    fn number(&self, name: &'static str) -> Result<Option<u64>, UsageError> {
        self.get(name).map(|text| number(name, text)).transpose()
    }
}

/// A decimal number: digits only, no sign, no unit.
fn number(name: &'static str, text: &str) -> Result<u64, UsageError> {
    let digits = !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    digits
        .then(|| text.parse().ok())
        .flatten()
        .ok_or(UsageError::Value(name.into(), "not a decimal number"))
}

fn hold(text: &str) -> Result<Hold, UsageError> {
    let name = "--hold-one-request";
    let bad = UsageError::Value(
        name.into(),
        "expected SECONDS:read:OFFSET, SECONDS:write:OFFSET or SECONDS:flush",
    );
    let mut parts = text.split(':');
    let seconds = parts.next().ok_or(bad.clone())?;
    let seconds = number(name, seconds).map_err(|_| bad.clone())?;
    let kind = parts.next().ok_or(bad.clone())?;
    let offset = parts.next().map(|o| number(name, o)).transpose();
    let offset = offset.map_err(|_| bad.clone())?;
    if parts.next().is_some() {
        return Err(bad);
    }
    let what = match (kind, offset) {
        ("read", Some(offset)) => Held::Read(offset),
        ("write", Some(offset)) => Held::Write(offset),
        ("flush", None) => Held::Flush,
        _ => return Err(bad),
    };
    Ok(Hold { seconds, what })
}

/// `--refuse-one-commit`: `read:OFFSET`, `write:OFFSET` or `flush`.
fn refuse(text: &str) -> Result<Held, UsageError> {
    let name = "--refuse-one-commit";
    let bad = UsageError::Value(name.into(), "expected read:OFFSET, write:OFFSET or flush");
    let (kind, offset) = match text.split_once(':') {
        Some((kind, offset)) => (kind, Some(number(name, offset).map_err(|_| bad.clone())?)),
        None => (text, None),
    };
    match (kind, offset) {
        ("read", Some(offset)) => Ok(Held::Read(offset)),
        ("write", Some(offset)) => Ok(Held::Write(offset)),
        ("flush", None) => Ok(Held::Flush),
        _ => Err(bad),
    }
}

/// `--refuse-one-fetch`: `start` or `recovery`.
/// `--request-log-content hash|whole`: whether every write's bytes are
/// kept. It needs `--request-log`.
fn request_log_content(given: &Given) -> Result<bool, UsageError> {
    let name = "--request-log-content";
    let whole = match given.get(name) {
        None | Some("hash") => false,
        Some("whole") => true,
        Some(_) => return Err(UsageError::Value(name.into(), "hash or whole")),
    };
    if given.get(name).is_some() && given.get("--request-log").is_none() {
        return Err(UsageError::Value(name.into(), "needs --request-log"));
    }
    Ok(whole)
}

fn refuse_fetch(text: &str) -> Result<FetchRefusal, UsageError> {
    match text {
        "start" => Ok(FetchRefusal::Start),
        "recovery" => Ok(FetchRefusal::Recovery),
        _ => Err(UsageError::Value(
            "--refuse-one-fetch".into(),
            "expected start or recovery",
        )),
    }
}

/// `--kill-at-upload`: `put:N`, `swap:N`, `manifest:N` or `publish:N`, N
/// from 1.
fn kill_at(text: &str) -> Result<KillAt, UsageError> {
    let name = "--kill-at-upload";
    let bad = || {
        UsageError::Value(
            name.into(),
            "expected put:N, swap:N, manifest:N or publish:N, N from 1",
        )
    };
    let (kind, n) = text.split_once(':').ok_or_else(bad)?;
    let n = number(name, n).ok().filter(|&n| n >= 1).ok_or_else(bad)?;
    match kind {
        "put" => Ok(KillAt::Put(n)),
        "swap" => Ok(KillAt::Swap(n)),
        "manifest" => Ok(KillAt::Manifest(n)),
        "publish" => Ok(KillAt::Publish(n)),
        _ => Err(bad()),
    }
}

/// `--endpoint --bucket --volume`: all three or none.
fn remote(given: &Given) -> Result<Option<Remote>, UsageError> {
    match (
        given.get("--endpoint"),
        given.get("--bucket"),
        given.get("--volume"),
    ) {
        (None, None, None) => Ok(None),
        (Some(endpoint), Some(bucket), Some(name)) => Remote::new(endpoint, bucket, name)
            .map(Some)
            .map_err(|e| UsageError::Refused(e.to_string())),
        _ => Err(UsageError::Refused(
            "--endpoint, --bucket and --volume: all three or none".into(),
        )),
    }
}

/// Whether `holder` can name the lease's holder: 1 to [`MAX_HOLDER`]
/// bytes with no space or control character. Also the rule for the
/// default holder (the host's name and the machine's id) when
/// `--holder` is not given.
pub fn check_holder(holder: &str) -> Result<(), String> {
    if holder.is_empty()
        || holder.len() > MAX_HOLDER
        || holder.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(format!(
            "the holder {holder:?}: 1 to {MAX_HOLDER} bytes with no space or control character"
        ));
    }
    Ok(())
}

fn run_dir(given: &Given) -> PathBuf {
    given
        .get("--run-dir")
        .map_or_else(|| PathBuf::from(DEFAULT_RUN_DIR), PathBuf::from)
}

/// `--name`: a snapshot's name, 1 to 128 of `[A-Za-z0-9._-]`.
fn snapshot_name(given: &Given) -> Result<String, UsageError> {
    let name = given.get("--name").ok_or(UsageError::Missing("--name"))?;
    if !skyzfs_upload::publish::valid_name(name) {
        return Err(UsageError::Value(
            "--name".into(),
            "not 1 to 128 of A-Z, a-z, 0-9, '.', '-' and '_'",
        ));
    }
    Ok(name.to_string())
}

fn volume(given: &Given) -> Result<Volume, UsageError> {
    Ok(Volume {
        dir: given.path("--dir")?,
        run_dir: run_dir(given),
    })
}

fn parse_with(args: impl IntoIterator<Item = String>, hooks: bool) -> Result<Command, UsageError> {
    let mut args = args.into_iter();
    let command = args.next().ok_or(UsageError::Command)?;
    match command.as_str() {
        "help" | "--help" | "-h" => match args.next() {
            None => Ok(Command::Help),
            Some(more) => Err(UsageError::Unknown(more)),
        },
        "create" => {
            let given = Given::collect(CREATE, args, hooks)?;
            let dir = given.path("--dir")?;
            let size = given
                .number("--size")?
                .ok_or(UsageError::Missing("--size"))?;
            if size == 0 || size % MIB != 0 {
                return Err(UsageError::Value(
                    "--size".into(),
                    "not a multiple of 1 MiB above zero",
                ));
            }
            Ok(Command::Create(Create {
                dir,
                size,
                run_dir: run_dir(&given),
                base_pattern: given.number("--base-pattern")?,
                zero_fill: given.flag("--zero-fill"),
                remote: remote(&given)?,
            }))
        }
        "serve" => {
            let given = Given::collect(SERVE, args, hooks)?;
            let Volume { dir, run_dir } = volume(&given)?;
            let small = |name: &'static str, default: u16, most: u16| {
                let Some(n) = given.number(name)? else {
                    return Ok(default);
                };
                u16::try_from(n)
                    .ok()
                    .filter(|n| (1..=most).contains(n))
                    .ok_or(UsageError::Value(name.into(), "out of range"))
            };
            let threads = small("--io-threads", DEFAULT_IO_THREADS, MAX_IO_THREADS)?;
            let in_place_io = IoMode::with_threads(usize::from(threads))
                .ok_or(UsageError::Value("--io-threads".into(), "out of range"))?;
            let hold_cuts = given.flag("--hold-cuts");
            let intent_region_mib = given.number("--intent-region-mib")?;
            if let Some(mib) = intent_region_mib {
                let name = || "--intent-region-mib".to_owned();
                if !hold_cuts {
                    return Err(UsageError::Value(name(), "only with --hold-cuts"));
                }
                if !mib.is_power_of_two()
                    || !(MIN_INTENT_REGION_MIB..=MAX_INTENT_REGION_MIB).contains(&mib)
                {
                    return Err(UsageError::Value(name(), "out of range"));
                }
            }
            // The header sync is on unless turned off, and turning it off
            // is taken only with `--hold-cuts`, as `--intent-region-mib`
            // is. A volume that uploads holds its cuts without
            // `--hold-cuts`; it is turned off there only with
            // `--hold-cuts` given as well (the conservative reading of
            // "it still needs --hold-cuts").
            let no_header_sync = given.flag("--no-intent-header-sync");
            if no_header_sync && !hold_cuts {
                return Err(UsageError::Value(
                    "--no-intent-header-sync".into(),
                    "only with --hold-cuts",
                ));
            }
            let intent_header_sync = !no_header_sync;
            let holder = given.get("--holder").map(str::to_owned);
            if let Some(holder) = &holder {
                check_holder(holder).map_err(UsageError::Refused)?;
            }
            let lease_ttl_s = given.number("--lease-ttl-s")?;
            let ttls = crate::upload::MIN_LEASE_TTL_S..=crate::upload::MAX_LEASE_TTL_S;
            if lease_ttl_s.is_some_and(|s| !ttls.contains(&s)) {
                return Err(UsageError::Value("--lease-ttl-s".into(), "out of range"));
            }
            Ok(Command::Serve(Serve {
                dir,
                run_dir,
                acknowledge_failure: given.flag("--acknowledge-failure"),
                async_delete_asserted: given.flag(ASYNC_DELETE_OPTION),
                queues: small("--queues", 1, MAX_QUEUES)?,
                depth: small("--depth", DEFAULT_DEPTH, MAX_DEPTH)?,
                in_place_io,
                hold_cuts,
                intent_region_mib,
                intent_header_sync,
                holder,
                lease_ttl_s,
                serve_without_upload: given.flag("--serve-without-upload"),
                allow_unlocked_memory: given.flag("--unsafe-allow-unlocked-memory"),
                allow_any_filesystem: given.flag("--unsafe-allow-any-filesystem"),
                hooks: Hooks {
                    fail_log_after: given.number("--fail-log-after")?,
                    hold: given.get("--hold-one-request").map(hold).transpose()?,
                    boot_id_file: given.get("--boot-id-file").map(PathBuf::from),
                    commit_delay_ms: given.number("--commit-delay-ms")?,
                    request_log: given.get("--request-log").map(PathBuf::from),
                    request_log_whole: request_log_content(&given)?,
                    refuse: given.get("--refuse-one-commit").map(refuse).transpose()?,
                    refuse_fetch: given
                        .get("--refuse-one-fetch")
                        .map(refuse_fetch)
                        .transpose()?,
                    kill_at: given.get("--kill-at-upload").map(kill_at).transpose()?,
                },
            }))
        }
        "detach" => {
            let given = Given::collect(DETACH, args, hooks)?;
            Ok(Command::Detach(Detach {
                volume: volume(&given)?,
                async_delete_asserted: given.flag(ASYNC_DELETE_OPTION),
            }))
        }
        "status" => Ok(Command::Status(volume(&Given::collect(
            VOLUME, args, hooks,
        )?)?)),
        "cuts" => Ok(Command::Ask(Ask {
            volume: volume(&Given::collect(VOLUME, args, hooks)?)?,
            request: crate::control::Request::Cuts,
            manifest: None,
        })),
        "publish-progress" | "withdraw" => {
            let given = Given::collect(NAMED, args, hooks)?;
            let name = snapshot_name(&given)?;
            Ok(Command::Ask(Ask {
                volume: volume(&given)?,
                request: match command.as_str() {
                    "withdraw" => crate::control::Request::Withdraw(name),
                    _ => crate::control::Request::Progress(name),
                },
                manifest: None,
            }))
        }
        "publish" => {
            let given = Given::collect(PUBLISH, args, hooks)?;
            let id = |name: &'static str| {
                let text = given.get(name).ok_or(UsageError::Missing(name))?;
                crate::control::unhex::<16>(text)
                    .ok_or(UsageError::Value(name.into(), "not 32 hexadecimal digits"))
            };
            let number = |name: &'static str| given.number(name)?.ok_or(UsageError::Missing(name));
            let expiry = given.number("--expiry-s")?;
            let most = skyzfs_upload::publish::EXPIRY_MAX.as_secs();
            if expiry.is_some_and(|s| !(1..=most).contains(&s)) {
                return Err(UsageError::Value("--expiry-s".into(), "out of range"));
            }
            Ok(Command::Ask(Ask {
                volume: volume(&given)?,
                request: crate::control::Request::Publish(crate::control::Publish {
                    name: snapshot_name(&given)?,
                    attachment: id("--attachment")?,
                    state: id("--state")?,
                    after: number("--after")?,
                    group: number("--group")?,
                    manifest: 0,
                    expiry: expiry.map(Duration::from_secs),
                }),
                manifest: given.get("--manifest").map(PathBuf::from),
            }))
        }
        _ => Err(UsageError::Command),
    }
}

#[cfg(test)]
mod tests;
