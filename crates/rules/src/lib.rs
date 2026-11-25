//! The rules AnkiQuest's server and clients compute alike, written once.
//!
//! Pure functions and plain types: no IO, no async, no server code. The server uses the crate
//! directly; the Android app and the desktop add-on use it through the UniFFI bindings (the `ffi`
//! feature), and the add-on's web views through the WebAssembly module (`crates/rules-wasm`). `tests/fixtures/*.json` holds the cases every implementation is tested against; see
//! `docs/shared-rules.md`.

mod day;
mod heatmap;

pub use day::{
    DayLabel, DayVerdict, MINUTE_MS, PendingReview, ServerDay, ServerToday, StreakState,
    TodayVerdict, meets_minimum, today_verdict,
};
pub use heatmap::heatmap_intensity;

#[cfg(feature = "ffi")]
uniffi::setup_scaffolding!();
