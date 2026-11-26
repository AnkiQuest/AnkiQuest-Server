//! The rules AnkiQuest's server and clients compute alike, written once.
//!
//! Pure functions and plain types: no IO, no async, no server code. The server uses the crate
//! directly; the Android app and the desktop add-on use it through the UniFFI bindings (the `ffi`
//! feature). `tests/fixtures/*.json` holds the cases every implementation is tested against; see
//! `docs/shared-rules.md`.

mod day;
mod optimize;

pub use day::{
    DayLabel, DayVerdict, MINUTE_MS, PendingReview, ServerDay, ServerToday, StreakState,
    TodayVerdict, meets_minimum, today_verdict,
};
pub use optimize::{
    LAST_ANSWER_TYPE, OPTIMIZE_DAYS, OPTIMIZE_REVIEWS, OPTIMIZE_SNOOZE_DAYS, OptimizeEvent,
    OptimizeInputs, OptimizeSince, OptimizeState, OptimizeVerdict, OptimizeWording, PresetFlags,
    PresetScheduler, answers_start_ms, is_answer, optimize_next, optimize_verdict,
    preset_scheduler,
};

#[cfg(feature = "ffi")]
uniffi::setup_scaffolding!();
