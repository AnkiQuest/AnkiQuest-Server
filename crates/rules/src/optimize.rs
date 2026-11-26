//! When to remind the player to run Anki's Optimize All Presets, and the reminder's state. The
//! cases are `tests/fixtures/optimize_reminder.json`; the state is the synced setting
//! `optimize_reminder` (`tests/fixtures/synced_settings.json`).
//!
//! Days are the collection's day numbers (Anki's `sched.today`). `last_optimized_day` is the
//! collection config key `lastFsrsOptimize`, with 0 or a missing key read as `None` (never).

/// Days since the last optimization before a reminder.
pub const OPTIMIZE_DAYS: i32 = 30;
/// Answers since the last optimization before a reminder.
pub const OPTIMIZE_REVIEWS: u32 = 1000;
/// How long "Remind me in a week" hides the reminder.
pub const OPTIMIZE_SNOOZE_DAYS: i32 = 7;
/// The last revlog type that is an answer: learning (0), review (1), relearning (2) and
/// filtered (3). Manual (4) and rescheduled (5) entries are not answers.
pub const LAST_ANSWER_TYPE: i32 = 3;

const DAY_SECS: i64 = 86_400;
const SECOND_MS: i64 = 1_000;

/// What a preset schedules with, as far as Optimize All Presets is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum PresetScheduler {
    /// FSRS is off: nothing to optimize.
    Sm2,
    Fsrs,
    /// RWKV-Curve: only its FSRS fallback is optimized.
    RwkvCurve,
    /// RWKV-Instant: only its FSRS fallback is optimized.
    RwkvInstant,
}

/// The switches a preset's scheduler is read from: the collection config key `fsrs` and the
/// preset's deck config keys `rwkvReviewEnabled` and `rwkvReviewInstantOrderEnabled` (missing
/// means false).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct PresetFlags {
    pub fsrs: bool,
    pub rwkv_review: bool,
    pub rwkv_instant_order: bool,
}

/// The reminder's state, one for all the player's devices (the synced setting
/// `optimize_reminder`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "kind", rename_all = "snake_case"))]
pub enum OptimizeState {
    On,
    /// Turned off by the settings switch.
    Off,
    /// Hidden while today is before `until_day`.
    Snoozed {
        until_day: i32,
    },
    /// Hidden until Optimize All Presets runs again: while it last ran on `optimized_day`.
    Dismissed {
        optimized_day: Option<i32>,
    },
}

impl OptimizeState {
    /// Whether the days are ones a collection has: `until_day` from 0, and `optimized_day` from
    /// 1, since 0 is never and stored as `None`.
    pub fn valid(&self) -> bool {
        match *self {
            OptimizeState::On | OptimizeState::Off => true,
            OptimizeState::Snoozed { until_day } => until_day >= 0,
            OptimizeState::Dismissed { optimized_day } => optimized_day.is_none_or(|day| day > 0),
        }
    }
}

/// What the player does to the reminder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum OptimizeEvent {
    /// Remind me in a week.
    Snooze,
    /// Dismiss until Optimize All Presets runs again.
    Dismiss,
    /// The settings switch.
    TurnOff,
    TurnOn,
}

/// The reminder's first line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum OptimizeSince {
    /// It ran, at least the limits ago.
    Stale,
    /// It never ran.
    Never,
}

/// What optimizing does for the presets in use: the reminder's second line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum OptimizeWording {
    /// Every preset that optimizes is FSRS.
    Fsrs,
    /// None is: only the RWKV presets' FSRS fallback is fitted.
    Rwkv,
    /// Both.
    Mixed,
}

/// What a client reads from the collection.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct OptimizeInputs {
    pub today: i32,
    pub last_optimized_day: Option<i32>,
    /// Answers since the last optimization, from [`answers_start_ms`] and [`is_answer`].
    pub reviews_since: u32,
    /// Every preset's scheduler.
    pub presets: Vec<PresetScheduler>,
}

/// Whether to remind, and if not, why not: the first rule that holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum OptimizeVerdict {
    Off,
    /// No preset uses FSRS, RWKV-Curve or RWKV-Instant.
    NoFsrs,
    Snoozed,
    Dismissed,
    /// It ran fewer than [`OPTIMIZE_DAYS`] ago.
    TooSoon,
    /// Fewer than [`OPTIMIZE_REVIEWS`] answers since it ran.
    TooFewReviews,
    Remind {
        since: OptimizeSince,
        wording: OptimizeWording,
    },
}

/// A preset's scheduler. With FSRS off every preset is SM-2, since Optimize All Presets needs
/// FSRS; RWKV-Instant wins over RWKV-Curve.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn preset_scheduler(flags: PresetFlags) -> PresetScheduler {
    if !flags.fsrs {
        return PresetScheduler::Sm2;
    }
    if flags.rwkv_instant_order {
        return PresetScheduler::RwkvInstant;
    }
    if flags.rwkv_review {
        return PresetScheduler::RwkvCurve;
    }
    PresetScheduler::Fsrs
}

/// The first revlog id that counts: the start of the day after the last optimization, or 0
/// (every answer) when it never ran. `day_cutoff` is Anki's `sched.day_cutoff` in seconds, the
/// end of today.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn answers_start_ms(today: i32, day_cutoff: i64, last_optimized_day: Option<i32>) -> i64 {
    let Some(last) = last_optimized_day else {
        return 0;
    };
    let days = i64::from(today) - i64::from(last);
    (day_cutoff - days * DAY_SECS) * SECOND_MS
}

/// Whether a revlog entry is an answer: a button pressed (`ease` > 0) on a learning, review,
/// relearning or filtered card.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn is_answer(revlog_type: i32, ease: i32) -> bool {
    ease > 0 && (0..=LAST_ANSWER_TYPE).contains(&revlog_type)
}

/// Whether to remind the player now.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn optimize_verdict(inputs: OptimizeInputs, state: OptimizeState) -> OptimizeVerdict {
    let today = inputs.today;
    let last = inputs.last_optimized_day;
    let optimized: Vec<PresetScheduler> = inputs
        .presets
        .into_iter()
        .filter(|preset| *preset != PresetScheduler::Sm2)
        .collect();

    // The first rule that holds names the verdict.
    match state {
        OptimizeState::Off => return OptimizeVerdict::Off,
        _ if optimized.is_empty() => return OptimizeVerdict::NoFsrs,
        OptimizeState::Snoozed { until_day } if today < until_day => {
            return OptimizeVerdict::Snoozed;
        }
        OptimizeState::Dismissed { optimized_day } if optimized_day == last => {
            return OptimizeVerdict::Dismissed;
        }
        _ => {}
    }
    if last.is_some_and(|last| today - last < OPTIMIZE_DAYS) {
        return OptimizeVerdict::TooSoon;
    }
    if inputs.reviews_since < OPTIMIZE_REVIEWS {
        return OptimizeVerdict::TooFewReviews;
    }

    // The wording follows what optimizing fits for the presets in use.
    let fsrs = optimized
        .iter()
        .filter(|preset| **preset == PresetScheduler::Fsrs)
        .count();
    let wording = match fsrs {
        0 => OptimizeWording::Rwkv,
        all if all == optimized.len() => OptimizeWording::Fsrs,
        _ => OptimizeWording::Mixed,
    };
    let since = match last {
        Some(_) => OptimizeSince::Stale,
        None => OptimizeSince::Never,
    };
    OptimizeVerdict::Remind { since, wording }
}

/// The state after `event`. While off, snooze and dismiss change nothing; turning on changes
/// only the off state, so a snooze or dismissal stays.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn optimize_next(
    state: OptimizeState,
    event: OptimizeEvent,
    today: i32,
    last_optimized_day: Option<i32>,
) -> OptimizeState {
    match (state, event) {
        (_, OptimizeEvent::TurnOff) => OptimizeState::Off,
        (OptimizeState::Off, OptimizeEvent::TurnOn) => OptimizeState::On,
        (OptimizeState::Off, _) | (_, OptimizeEvent::TurnOn) => state,
        (_, OptimizeEvent::Snooze) => OptimizeState::Snoozed {
            until_day: today.saturating_add(OPTIMIZE_SNOOZE_DAYS),
        },
        (_, OptimizeEvent::Dismiss) => OptimizeState::Dismissed {
            optimized_day: last_optimized_day,
        },
    }
}
