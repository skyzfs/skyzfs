//! The `skyzfsd` binary. See the library's documentation.

use std::process::ExitCode;

use skyzfs_ublk::lifecycle::Control;
use skyzfsd::cli::{self, Command, Exit};
use skyzfsd::commands::{self, Now, Output};

/// The kernel's control interface. It is not built yet: the binding
/// follows the libublk spike (work package M1.3b).
fn kernel() -> Result<Box<dyn Control>, String> {
    Err("the ublk control interface is not built yet".into())
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
        Command::Status(volume) => {
            let mut control = match kernel() {
                Ok(control) => control,
                Err(why) => return failed(why),
            };
            match Now::read() {
                Ok(now) => commands::status(volume, control.as_mut(), &now),
                Err(e) => failed(format!("reading the boot id and the uptime: {e}")),
            }
        }
        Command::Detach(volume) => {
            let mut control = match kernel() {
                Ok(control) => control,
                Err(why) => return failed(why),
            };
            match Now::read() {
                Ok(now) => commands::detach(volume, control.as_mut(), now.boot),
                Err(e) => failed(format!("reading the boot id: {e}")),
            }
        }
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
        // The other commands are built on the transport, which is not
        // here yet.
        other => failed(format!("{} is not built yet", other.name())),
    }
}

fn main() -> ExitCode {
    let command = match cli::parse(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("skyzfsd: {error}");
            eprintln!("{}", cli::USAGE);
            return Exit::Usage.code();
        }
    };
    let output = run(&command);
    print!("{}", output.out);
    eprint!("{}", output.err);
    output.exit.code()
}
