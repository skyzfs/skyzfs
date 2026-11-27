//! A [`Host`] of the tests: each command's output given beforehand, by
//! its words, and every command run recorded.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{Host, Node, Output};

/// What a command gives: its output, or an error of the run itself.
pub(crate) type Given = Result<Output, io::ErrorKind>;

/// A command run: its words, its standard input and its limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ran {
    pub(crate) words: String,
    pub(crate) stdin: Vec<u8>,
    pub(crate) limit: Duration,
}

/// The fake ([module docs](self)).
#[derive(Debug, Default)]
pub(crate) struct Fake {
    /// By the command's words (`program arg...`), what each run of it
    /// gives, the first first; the last is given again once the others
    /// are used.
    pub(crate) commands: BTreeMap<String, Vec<Given>>,
    pub(crate) files: BTreeMap<PathBuf, Vec<u8>>,
    pub(crate) nodes: BTreeMap<PathBuf, Node>,
    /// Paths whose read fails with that kind of error (the review of
    /// #173: an error that is not `NotFound`).
    pub(crate) read_errors: BTreeMap<PathBuf, io::ErrorKind>,
    pub(crate) ran: Vec<Ran>,
}

impl Fake {
    /// `words` exits 0 with `stdout`.
    pub(crate) fn ok(&mut self, words: &str, stdout: &str) -> &mut Self {
        self.give(words, Ok(output(0, stdout, "")))
    }

    /// `words` exits `code` with `stderr`.
    pub(crate) fn fails(&mut self, words: &str, code: i32, stderr: &str) -> &mut Self {
        self.give(words, Ok(output(code, "", stderr)))
    }

    pub(crate) fn give(&mut self, words: &str, given: Given) -> &mut Self {
        self.commands.entry(words.into()).or_default().push(given);
        self
    }

    /// The words of each command run, in order.
    pub(crate) fn words(&self) -> Vec<&str> {
        self.ran.iter().map(|r| r.words.as_str()).collect()
    }
}

pub(crate) fn output(code: i32, stdout: &str, stderr: &str) -> Output {
    Output {
        code: Some(code),
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
    }
}

impl Host for Fake {
    fn run(
        &mut self,
        program: &str,
        args: &[&str],
        stdin: &[u8],
        limit: Duration,
    ) -> io::Result<Output> {
        let mut words = vec![program];
        words.extend(args);
        let words = words.join(" ");
        self.ran.push(Ran {
            words: words.clone(),
            stdin: stdin.to_vec(),
            limit,
        });
        let Some(given) = self.commands.get_mut(&words) else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("no output given for `{words}`"),
            ));
        };
        let next = if given.len() > 1 {
            given.remove(0)
        } else {
            given[0].clone()
        };
        next.map_err(io::Error::from)
    }

    fn read(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        if let Some(kind) = self.read_errors.get(path) {
            return Err(io::Error::new(
                *kind,
                format!("{}: refused", path.display()),
            ));
        }
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }

    fn stat(&mut self, path: &Path) -> io::Result<Node> {
        self.nodes
            .get(path)
            .copied()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }
}
