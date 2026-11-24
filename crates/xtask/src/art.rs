//! Companion art tools: the coverage report the gallery and CI read, and
//! the guard that keeps art out of this repository. The masters are in the
//! AnkiQuest-Art release, staged into `art/`; AnkiQuest-Art's own tools export the
//! clients' copies.

use std::process::ExitCode;

use crate::root;

mod coverage;
#[cfg(test)]
mod guard;

/// Runs the art coverage report.
pub fn coverage(args: &[String]) -> ExitCode {
    coverage::run(args)
}
