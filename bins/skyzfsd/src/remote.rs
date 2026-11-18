//! The volume's place in object storage (M2.4c): the file [`REMOTE`] in
//! the volume's directory.
//!
//! `create --endpoint E --bucket B --volume NAME` makes the volume in
//! object storage and writes the file; `serve` reads it, uploads to that
//! volume, and takes no option of object storage of its own. A volume
//! created without the options has no file and is served with no upload,
//! as before M2.4. (A volume created so cannot be given a place later:
//! open, the lead's decision 1 on M2.4c's plan.)
//!
//! Why a file and not options at each serve: a serve given the wrong
//! volume's name would upload a whole cut onto that volume's head, since
//! a whole cut needs no base (`skyzfs_upload::check`, rule 3). The name
//! is given once, where the volume is made, and `create` refuses a name
//! whose volume in object storage holds anything but its first, empty
//! commit.
//!
//! # The file
//!
//! UTF-8 text, five lines, each ending in a line feed:
//!
//! ```text
//! skyzfs-remote 1
//! endpoint https://objects.example:9000
//! bucket volumes
//! volume pg-data
//! id 4C8DB5WVJ4ZQ9Q5N3V2M1T0R8S
//! ```
//!
//! The first line names the version; a file of another version, or that
//! is not exactly these lines in this order, is refused, and the daemon
//! does not guess. The id is the volume's in object storage, derived from
//! the name as `skynbd-api`'s provisioner and node B derive it
//! ([`volume_id`]); it is written so that whoever reads the file sees it,
//! and a file whose id is not its name's is refused (one line edited by
//! hand). Credentials are never in it: they come from the environment
//! (`AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`, or else the ambient
//! chain of the AWS SDK).
//!
//! The file is written whole under [`REMOTE_NEW`], synced, renamed, and
//! the directory synced.

use std::fmt;
use std::io::{self, Write};
use std::path::Path;

use skynbd_types::VolumeId;

/// The file's name in the volume's directory.
pub const REMOTE: &str = "remote";

/// What the file is written under before it is renamed.
pub const REMOTE_NEW: &str = "remote.new";

/// The first line of the version this daemon writes and reads.
const FIRST_LINE: &str = "skyzfs-remote 1";

/// The longest endpoint or bucket, in bytes.
const MAX_VALUE: usize = 255;

/// The volume's place in object storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    /// The endpoint's URL.
    pub endpoint: String,
    /// The bucket.
    pub bucket: String,
    /// The volume's name in object storage.
    pub name: String,
    /// Its id there: [`volume_id`] of the name.
    pub id: VolumeId,
}

/// A value of the file or of `create`'s options that is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RemoteError {
    /// The file is not this version's five lines.
    #[error("the file {REMOTE} is not a place in object storage this daemon reads: {0}")]
    Unreadable(String),
    /// A value is not one the file can hold.
    #[error("{0}")]
    Value(String),
}

/// The id of the volume named `name` in object storage: the first sixteen
/// bytes of the name's BLAKE3 hash, as a ULID. The provisioner of
/// `skynbd-api` (`provision::volume_id`) and node B derive it so, so that
/// each finds the volume the others name.
pub fn volume_id(name: &str) -> VolumeId {
    let hash = blake3::hash(name.as_bytes());
    let mut first = [0u8; 16];
    first.copy_from_slice(&hash.as_bytes()[..16]);
    VolumeId(ulid::Ulid::from(u128::from_be_bytes(first)))
}

/// Whether `name` is a volume's name: 1 to 63 characters of `A-Z`, `a-z`,
/// `0-9`, `.`, `-` and `_`, beginning with a letter or a digit. The rule
/// of `skynbd-api`'s provisioner (`validate_name`).
pub fn check_name(name: &str) -> Result<(), RemoteError> {
    let chars = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    let first = name.starts_with(|c: char| c.is_ascii_alphanumeric());
    if !name.is_empty() && name.len() <= 63 && chars && first {
        Ok(())
    } else {
        Err(RemoteError::Value(format!(
            "--volume {name:?}: 1 to 63 characters of A-Z, a-z, 0-9, '.', '-' and '_', beginning \
             with a letter or a digit"
        )))
    }
}

/// Whether `value` can be the file's `key`: not empty, at most
/// [`MAX_VALUE`] bytes, and no space or control character, so that it is
/// one word of one line.
fn check_value(key: &str, value: &str) -> Result<(), RemoteError> {
    if value.is_empty()
        || value.len() > MAX_VALUE
        || value.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(RemoteError::Value(format!(
            "--{key} {value:?}: 1 to {MAX_VALUE} bytes with no space or control character"
        )));
    }
    Ok(())
}

impl Remote {
    /// The place of the volume `name` in `bucket` at `endpoint`, with its
    /// id; refused where a value cannot be written in the file.
    pub fn new(endpoint: &str, bucket: &str, name: &str) -> Result<Self, RemoteError> {
        check_value("endpoint", endpoint)?;
        check_value("bucket", bucket)?;
        check_name(name)?;
        Ok(Self {
            endpoint: endpoint.into(),
            bucket: bucket.into(),
            name: name.into(),
            id: volume_id(name),
        })
    }

    /// The file's bytes.
    pub fn encode(&self) -> String {
        format!(
            "{FIRST_LINE}\nendpoint {}\nbucket {}\nvolume {}\nid {}\n",
            self.endpoint, self.bucket, self.name, self.id.0
        )
    }

    /// The file's bytes, read back: refused unless they are exactly what
    /// [`Remote::encode`] writes for values [`Remote::new`] takes.
    pub fn decode(bytes: &[u8]) -> Result<Self, RemoteError> {
        let unreadable = |why: &str| RemoteError::Unreadable(why.to_string());
        let text = std::str::from_utf8(bytes).map_err(|_| unreadable("not UTF-8"))?;
        let body = text
            .strip_suffix('\n')
            .ok_or_else(|| unreadable("its last line has no line feed"))?;
        let mut lines = body.split('\n');
        match lines.next() {
            Some(FIRST_LINE) => {}
            Some(first) if first.starts_with("skyzfs-remote ") => {
                return Err(RemoteError::Unreadable(format!(
                    "a version this daemon does not know ({first:?})"
                )))
            }
            _ => return Err(unreadable("its first line is not \"skyzfs-remote 1\"")),
        }
        let mut value = |key: &str| {
            let line = lines
                .next()
                .ok_or_else(|| RemoteError::Unreadable(format!("no line {key:?}")))?;
            line.strip_prefix(key)
                .and_then(|rest| rest.strip_prefix(' '))
                .ok_or_else(|| {
                    RemoteError::Unreadable(format!("{line:?} where the line {key:?} belongs"))
                })
        };
        let (endpoint, bucket, name, id) = (
            value("endpoint")?,
            value("bucket")?,
            value("volume")?,
            value("id")?,
        );
        if lines.next().is_some() {
            return Err(unreadable("a line after the id"));
        }
        let remote = Self::new(endpoint, bucket, name)
            .map_err(|e| RemoteError::Unreadable(e.to_string()))?;
        if id != remote.id.0.to_string() {
            return Err(RemoteError::Unreadable(format!(
                "the id {id} is not the volume {name:?}'s ({})",
                remote.id.0
            )));
        }
        Ok(remote)
    }
}

impl fmt::Display for Remote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "volume {} ({}) in bucket {} at {}",
            self.name, self.id.0, self.bucket, self.endpoint
        )
    }
}

/// The volume's place, read from the directory `dir`: `Ok(None)` where it
/// has no file [`REMOTE`].
pub fn read(dir: &Path) -> Result<Option<Remote>, String> {
    let path = dir.join(REMOTE);
    match std::fs::read(&path) {
        Ok(bytes) => Remote::decode(&bytes)
            .map(Some)
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Write `remote` into the directory `dir`: whole under [`REMOTE_NEW`],
/// synced, renamed over [`REMOTE`], and the directory synced.
pub fn write(dir: &Path, remote: &Remote) -> io::Result<()> {
    let new = dir.join(REMOTE_NEW);
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&new)?;
        file.write_all(remote.encode().as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&new, dir.join(REMOTE))?;
    std::fs::File::open(dir)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Remote {
        Remote::new("http://127.0.0.1:9000", "volumes", "pg-data").expect("a place")
    }

    /// The id is the provisioner's: the same name gives the same id as
    /// `skynbd-api` and node B, and two names two ids.
    #[test]
    fn the_id_is_the_names_as_the_provisioner_derives_it() {
        let hash = blake3::hash(b"pg-data");
        let mut first = [0u8; 16];
        first.copy_from_slice(&hash.as_bytes()[..16]);
        assert_eq!(
            volume_id("pg-data").0,
            ulid::Ulid::from(u128::from_be_bytes(first))
        );
        assert_ne!(volume_id("pg-data"), volume_id("pg-data2"));
        assert_eq!(sample().id, volume_id("pg-data"));
    }

    /// The file is the five lines of the module's documentation, and is
    /// read back as it was written.
    #[test]
    fn the_file_is_five_lines_and_reads_back() {
        let remote = sample();
        let text = remote.encode();
        assert_eq!(
            text,
            format!(
                "skyzfs-remote 1\nendpoint http://127.0.0.1:9000\nbucket volumes\nvolume \
                 pg-data\nid {}\n",
                volume_id("pg-data").0
            )
        );
        assert_eq!(Remote::decode(text.as_bytes()), Ok(remote));
    }

    /// Anything but exactly those lines is refused, with what is wrong:
    /// another version, a missing or extra line, a key out of its place,
    /// no last line feed, an id that is not the name's, a value the
    /// options would refuse.
    #[test]
    fn a_file_that_is_not_exactly_the_five_lines_is_refused() {
        let good = sample().encode();
        let id = volume_id("pg-data").0.to_string();
        let other = volume_id("other").0.to_string();
        let cases: Vec<(String, &str)> = vec![
            (
                good.replace("skyzfs-remote 1", "skyzfs-remote 2"),
                "version",
            ),
            (good.replace("skyzfs-remote 1", "remote 1"), "first line"),
            (good.trim_end().to_string(), "line feed"),
            (good.replace(&format!("id {id}\n"), ""), "no line \"id\""),
            (format!("{good}more\n"), "after the id"),
            (
                good.replace(
                    "bucket volumes\nvolume pg-data",
                    "volume pg-data\nbucket volumes",
                ),
                "where the line \"bucket\" belongs",
            ),
            (good.replace(&id, &other), "is not the volume"),
            (good.replace("pg-data", "-pg"), "--volume"),
            (
                good.replace("bucket volumes", "bucket two words"),
                "--bucket \"two words\": 1 to 255 bytes with no space",
            ),
            (good.replace("bucket volumes", "bucket "), "--bucket"),
            (
                String::from_utf8_lossy(&[0xff, b'\n']).into_owned(),
                "first line",
            ),
        ];
        for (text, said) in cases {
            let refused = Remote::decode(text.as_bytes()).expect_err(&text);
            assert!(refused.to_string().contains(said), "{text:?}: {refused}");
        }
        assert!(Remote::decode(&[0xff, b'\n'][..]).is_err());
    }

    /// The options are refused where the file could not hold them: a
    /// name outside the provisioner's rule, an endpoint or bucket that is
    /// empty, too long, or has a space or a control character.
    #[test]
    fn values_the_file_cannot_hold_are_refused() {
        for name in ["", "-a", ".a", "a/b", "a b", &"a".repeat(64), "é"] {
            assert!(Remote::new("http://e", "b", name).is_err(), "{name:?}");
        }
        assert!(Remote::new("http://e", "b", &"a".repeat(63)).is_ok());
        assert!(Remote::new("http://e", "b", "a.b-c_9").is_ok());
        for value in ["", "a b", "a\nb", "a\tb", &"x".repeat(256)] {
            assert!(Remote::new(value, "b", "n").is_err(), "endpoint {value:?}");
            assert!(
                Remote::new("http://e", value, "n").is_err(),
                "bucket {value:?}"
            );
        }
    }

    /// The file is written whole and read back; a directory with none
    /// reads as no place, and a damaged one is an error naming the file.
    #[test]
    fn the_file_is_written_and_read_in_the_directory() {
        let dir = tempfile::tempdir().expect("a directory");
        assert_eq!(read(dir.path()), Ok(None));
        write(dir.path(), &sample()).expect("written");
        assert_eq!(read(dir.path()), Ok(Some(sample())));
        assert!(!dir.path().join(REMOTE_NEW).exists());
        std::fs::write(dir.path().join(REMOTE), "skyzfs-remote 9\n").expect("damaged");
        let refused = read(dir.path()).expect_err("refused");
        assert!(refused.contains("remote: the file remote"), "{refused}");
    }
}
