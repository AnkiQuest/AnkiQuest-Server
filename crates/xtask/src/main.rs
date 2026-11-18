//! Repository tools, run with `cargo run -p ankiquest-xtask -- <command>`.
//!
//! - `android-contract --android-resources <dir> [--server-examples <dir>]`
//! - `export-android-art [--check] <android-checkout>`
//! - `export-addon-art [--check] <desktop-checkout>`
//! - `art-coverage [--missing | --since <companion_poses.json>]`
//! - `banner [--social] [--out <file>]`

mod art;
mod contract;

use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage: xtask android-contract --android-resources <dir> \
[--server-examples <dir>]\n       xtask export-android-art [--check] <android-checkout>\n       \
xtask export-addon-art [--check] <desktop-checkout>\n       xtask art-coverage [--missing | --since <companion_poses.json>]\n       \
xtask banner [--social] [--out <file>]";

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let command = args.next();
    let rest: Vec<String> = args.collect();

    // Dispatch to the command; each parses its own flags.
    match command.as_deref() {
        Some("android-contract") => contract::run(&rest),
        Some("export-android-art") => art::run(&rest),
        Some("export-addon-art") => art::addon(&rest),
        Some("art-coverage") => art::coverage(&rest),
        Some("banner") => art::banner(&rest),
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
