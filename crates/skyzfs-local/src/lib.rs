//! The SkyZFS local store (design §5.1).
//!
//! So far this crate holds the ground the write log stands on:
//!
//! - [`io`]: the few file and directory operations the store uses, as
//!   traits, so that the same code runs on real files and on a disk that
//!   fails on purpose;
//! - [`sim`]: `SimDisk`, that disk, in memory and reproducible from a
//!   seed;
//! - [`fs`]: the same operations on real files.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod fs;
pub mod io;
pub mod sim;
