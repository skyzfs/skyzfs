//! The `skyzfsd` binary. See the library's documentation.

use std::process::ExitCode;

use skyzfsd::cli::{self, Exit};

fn main() -> ExitCode {
    let command = match cli::parse(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("skyzfsd: {error}");
            eprintln!("{}", cli::USAGE);
            return Exit::Usage.code();
        }
    };
    // The commands are built on the transport, which is not here yet.
    eprintln!("skyzfsd: {} is not built yet", command.name());
    Exit::Error.code()
}
