//! The other operating systems: no region yet (see the crate's
//! documentation).

use std::io;
use std::path::Path;

use crate::Opened;

/// A region of words: none can be opened on this operating system yet.
#[derive(Debug)]
pub struct Mapped {
    never: std::convert::Infallible,
}

impl Mapped {
    /// Fails: see the crate's documentation.
    pub fn open(path: &Path, words: usize) -> io::Result<(Self, Opened)> {
        crate::bytes_of(words)?;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "{}: a region kept while the machine is up is not made on this operating system yet",
                path.display()
            ),
        ))
    }

    /// The region's length, in words.
    pub fn words(&self) -> usize {
        match self.never {}
    }

    /// The word at `at`.
    pub fn load(&self, _at: usize) -> u64 {
        match self.never {}
    }

    /// Store `value` at `at`.
    pub fn store(&mut self, _at: usize, _value: u64) {
        match self.never {}
    }
}
