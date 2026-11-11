//! The `skyzfsd` binary. See the library's documentation.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use skyzfs_ublk::lifecycle::Control;
use skyzfsd::cli::{self, Command, Exit};
use skyzfsd::commands::{self, Now, Output};

/// The kernel's control interface, opened when it is first asked
/// something: `status` and `detach` of a volume with nothing attached
/// in this boot do not need the driver, and work before it is loaded.
fn kernel() -> impl Control {
    commands::Lazy::new(|| -> std::io::Result<Box<dyn Control>> {
        #[cfg(target_os = "linux")]
        {
            Ok(Box::new(skyzfs_ublk_sys::control::Kernel::new()?))
        }
        #[cfg(not(target_os = "linux"))]
        Err(std::io::Error::other(
            "ublk is a Linux driver: nothing to ask on this system",
        ))
    })
}

fn run(command: &Command) -> Output {
    let failed = |why: String| Output {
        exit: Exit::Error,
        out: String::new(),
        err: format!("skyzfsd: {why}\n"),
    };
    match command {
        Command::Help => Output {
            exit: Exit::Done,
            out: format!("{}\n", cli::HELP),
            err: String::new(),
        },
        Command::Status(volume) => match Now::read() {
            Ok(now) => commands::status(volume, &mut kernel(), &now),
            Err(e) => failed(format!("reading the boot id and the uptime: {e}")),
        },
        Command::Detach(volume) => match Now::read() {
            Ok(now) => commands::detach(volume, &mut kernel(), now.boot),
            Err(e) => failed(format!("reading the boot id: {e}")),
        },
        Command::Create(create) => {
            let mut id = [0u8; 16];
            let random = std::fs::File::open("/dev/urandom")
                .and_then(|mut source| std::io::Read::read_exact(&mut source, &mut id));
            if let Err(e) = random {
                return failed(format!("reading /dev/urandom for the volume's id: {e}"));
            }
            // Without the boot id there is no warning of a failure
            // note; the volume is made all the same.
            commands::create(create, id, Now::read().ok().map(|now| now.boot))
        }
        // `serve` prints for itself: see `main`.
        Command::Serve(_) => failed("serve is run by main".into()),
    }
}

fn main() -> ExitCode {
    let command = match cli::parse(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(error) => {
            skyzfsd::to_stderr(&format!("skyzfsd: {error}\n{}\n", cli::USAGE));
            return Exit::Usage.code();
        }
    };
    #[cfg(target_os = "linux")]
    if let Command::Serve(serve) = &command {
        return skyzfsd::serve::serve(serve).code();
    }
    let output = run(&command);
    skyzfsd::to_stdout(&output.out);
    skyzfsd::to_stderr(&output.err);
    output.exit.code()
}
