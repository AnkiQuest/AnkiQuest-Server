//! Day verdicts: whether a day counts toward the streak, and today's verdict on a client that
//! holds reviews it has not uploaded yet. The cases are `tests/fixtures/day_verdicts.json`.

/// Milliseconds in a minute.
pub const MINUTE_MS: i64 = 60_000;

/// Where the streak stands today, as the server's profile says (`streak_state`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum StreakState {
    /// Today reached the minimum.
    Studied,
    /// Today has not reached the minimum yet.
    Pending,
    /// A freeze or grant kept yesterday, and today has not reached the minimum yet.
    Protected,
}

/// The words a client shows for today (`label`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum DayLabel {
    /// Today counts, or will once unsent reviews arrive.
    Studied,
    /// A freeze or grant kept yesterday and nothing has been studied today.
    Protected,
    /// Reviews today, short of the minimum.
    Short,
    /// No reviews today.
    Pending,
    /// An unsent review from an earlier day leaves today open.
    Unknown,
}

/// What the profile's `today` says.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ServerToday {
    /// When today began, in ms since the epoch.
    pub starts_at: i64,
    pub reviews: u32,
    pub time_ms: i64,
    pub minutes: u32,
    /// The minutes today needs to count; 0 when any review counts.
    pub streak_minutes: u32,
}

/// What a client reads of the server's profile to judge today.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ServerDay {
    pub streak: u32,
    pub streak_state: StreakState,
    /// When the day the answer is about ends, in ms since the epoch.
    pub day_ends_at: i64,
    /// The newest review the server has. Reviews up to it are ones the server already counted.
    pub last_review_id: i64,
    pub today: ServerToday,
}

/// A review the client holds and has not uploaded. Its id is when it was answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PendingReview {
    pub id: i64,
    pub time_ms: i64,
}

/// Today's verdict with unsent reviews added to what the server said.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TodayVerdict {
    /// Today counts toward the streak.
    pub met: bool,
    /// `None` while today is short and an unsent review belongs to an earlier day.
    pub streak_state: Option<StreakState>,
    /// `None` once an unsent review belongs to an earlier day.
    pub streak: Option<u32>,
    pub reviews: u32,
    pub time_ms: i64,
    pub minutes: u32,
    pub streak_minutes: u32,
    /// Unsent reviews that count toward today.
    pub unsent: u32,
    pub label: DayLabel,
}

/// The verdict on today, or that the server's answer is about a day that has ended.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum DayVerdict {
    /// The answer is from a day that has ended and says nothing about today.
    Ended,
    Today {
        verdict: TodayVerdict,
    },
}

/// A day with `reviews` taking `time_ms` counts toward the streak once it reaches `needed_ms`.
/// A day without reviews never counts, even when nothing is needed.
pub fn meets_minimum(reviews: u64, time_ms: i64, needed_ms: i64) -> bool {
    reviews > 0 && time_ms >= needed_ms
}

/// Today's verdict at `now_ms`: the server's answer with the `pending` reviews it has not
/// counted yet, judged exactly as the server will once they arrive.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn today_verdict(server: ServerDay, pending: Vec<PendingReview>, now_ms: i64) -> DayVerdict {
    let today = &server.today;
    let starts = today.starts_at;
    let ends = server.day_ends_at;
    if now_ms >= ends {
        return DayVerdict::Ended;
    }

    // Reviews up to the server's newest are ones it has; of the rest, those in today count.
    let unsent: Vec<&PendingReview> = pending
        .iter()
        .filter(|r| r.id > server.last_review_id)
        .collect();
    let counted: Vec<&PendingReview> = unsent
        .iter()
        .copied()
        .filter(|r| (starts..ends).contains(&r.id))
        .collect();
    let earlier = unsent.iter().any(|r| r.id < starts);
    let added = count(counted.len());
    let reviews = today.reviews.saturating_add(added);
    let time_ms = today.time_ms + counted.iter().map(|r| r.time_ms).sum::<i64>();
    let needed_ms = i64::from(today.streak_minutes) * MINUTE_MS;

    // Today counts once the minimum is met; a day that already counted adds nothing.
    let was = server.streak_state;
    let met = was == StreakState::Studied || meets_minimum(u64::from(reviews), time_ms, needed_ms);
    let grows = met && was != StreakState::Studied;
    let streak = server.streak.saturating_add(u32::from(grows));

    // The words for the day: reviews today outrank a protected streak, which only says that
    // nothing has been studied yet.
    let label = if met {
        DayLabel::Studied
    } else if earlier {
        DayLabel::Unknown
    } else if reviews > 0 {
        DayLabel::Short
    } else if was == StreakState::Protected {
        DayLabel::Protected
    } else {
        DayLabel::Pending
    };

    // A review from an earlier day changes that day on the server, which the client cannot
    // judge: the streak, and a short day's state, stay unknown.
    let streak_state = if met {
        Some(StreakState::Studied)
    } else if earlier {
        None
    } else {
        Some(was)
    };
    DayVerdict::Today {
        verdict: TodayVerdict {
            met,
            streak_state,
            streak: (!earlier).then_some(streak),
            reviews,
            time_ms,
            minutes: count(time_ms.max(0) / MINUTE_MS),
            streak_minutes: today.streak_minutes,
            unsent: added,
            label,
        },
    }
}

/// A count as the bindings carry it, saturating far beyond any real day.
fn count<T: TryInto<u32>>(n: T) -> u32 {
    n.try_into().unwrap_or(u32::MAX)
}
