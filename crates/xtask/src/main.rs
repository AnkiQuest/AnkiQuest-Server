//! Repository tools, run with `cargo run -p ankiquest-xtask -- <command>`.
//!
//! - `android-contract --android-catalog <names.json> --baseline <dir> [--server-examples <dir>]`
//! - `art-coverage [--missing | --json | --since <companion_poses.json>]`
//! - `event-base`

mod art;
mod contract;
mod event;

use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage: xtask android-contract --android-catalog <names.json> \
--baseline <dir> [--server-examples <dir>]\n       \
xtask art-coverage [--missing | --json | --since <companion_poses.json>]\n       \
xtask event-base";

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let command = args.next();
    let rest: Vec<String> = args.collect();

    // Dispatch to the command; each parses its own flags.
    match command.as_deref() {
        Some("android-contract") => contract::run(&rest),
        Some("art-coverage") => art::coverage(&rest),
        Some("event-base") => event::run(&rest),
        _ => usage(),
    }
}

/// Prints the usage text and fails.
fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::FAILURE
}

/// The repository root.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
