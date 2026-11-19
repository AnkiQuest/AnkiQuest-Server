//! Reviews, clocks and profiles shared by the tests of the game modules.

use super::*;

pub(super) const NOON: i64 = 12 * 3_600_000;

/// One card's answers a day apart: (kind, last_ivl, ivl).
pub(super) fn card(cid: i64, answers: &[(u8, i64, Option<i64>)]) -> Vec<Review> {
    answers
        .iter()
        .enumerate()
        .map(|(i, &(kind, last_ivl, ivl))| Review {
            id: (cid * 1000 + i as i64) * DAY_MS,
            cid,
            last_ivl,
            ivl,
            time_ms: 5000,
            kind,
        })
        .collect()
}

/// A review failed into relearning, then the relearning step.
pub(super) fn lapse(last_ivl: i64, ivl: Option<i64>) -> [(u8, i64, Option<i64>); 2] {
    [(1, last_ivl, ivl.map(|_| -600)), (2, -600, ivl.map(|_| 1))]
}

pub(super) fn utc() -> Clock {
    Clock::default()
}

pub(super) fn reviews_on(day: i64, count: i64, cid_base: i64) -> Vec<Review> {
    (0..count)
        .map(|i| Review {
            id: day * DAY_MS + NOON + i * 10_000,
            cid: cid_base + i,
            last_ivl: 0,
            ivl: None,
            time_ms: 5_000,
            kind: 1,
        })
        .collect()
}

pub(super) fn history(days: &[i64]) -> Vec<Review> {
    days.iter()
        .flat_map(|d| reviews_on(*d, 5, d * 1000))
        .collect()
}

pub(super) fn at(day: i64) -> i64 {
    day * DAY_MS + NOON + 3_600_000
}

pub(super) fn protection(at: i64, enabled: bool) -> FreezePreference {
    FreezePreference { at, enabled }
}

pub(super) fn protected(reviews: &[Review], now: i64, changes: &[FreezePreference]) -> Profile {
    compute_with_freezes(
        "a",
        "a",
        reviews,
        &utc(),
        &Week::default(),
        now,
        &FreezePolicy {
            preferences: changes.to_vec(),
            ..FreezePolicy::default()
        },
    )
}

/// Finish every possible quest metric, with a second session before noon.
pub(super) fn perfect_day(prior: &[Review], day: i64, clock: &Clock) -> Vec<Review> {
    let (days, _, _) = collect_days_with_quests(prior, &ClockHistory::single(*clock), seed_of("a"));
    let recent = days
        .values()
        .rev()
        .take(RECENT_DAYS)
        .cloned()
        .rev()
        .collect();
    let quests = gen_quests(seed_of("a"), day, &recent);
    let count = quests.iter().map(|q| q.target).max().unwrap().max(2) as i64 + 1;
    let start = clock.day_start_ms(day) + 2 * 3_600_000;
    (0..count)
        .map(|i| {
            let id = start + i * 10_000 + if i == count - 1 { SESSION_GAP_MS } else { 0 };
            Review {
                id,
                cid: id,
                last_ivl: MATURE_IVL,
                ivl: None,
                time_ms: 60_000,
                kind: 0,
            }
        })
        .collect()
}

pub(super) fn perfect_history(days: &[i64]) -> Vec<Review> {
    let mut reviews = Vec::new();
    for day in days {
        reviews.extend(perfect_day(&reviews, *day, &utc()));
    }
    reviews
}

pub(super) fn first_completion(reviews: &[Review], day: i64) -> i64 {
    collect_days_with_quests(reviews, &ClockHistory::single(utc()), seed_of("a"))
        .0
        .get(&day)
        .unwrap()
        .quests_completed_at
        .expect("fixture completes every quest")
}

pub(super) fn closing_of(p: &Profile) -> Option<ClosingWeek> {
    match p.own_week {
        Some(OwnWeek::Closing(closing)) => Some(closing),
        _ => None,
    }
}

pub(super) const BERLIN: Clock = Clock {
    offset_west_min: -120,
    rollover_hour: 4,
};

pub(super) const CALIFORNIA: Clock = Clock {
    offset_west_min: 420,
    rollover_hour: 4,
};

pub(super) fn moment(at: i64, cid: i64) -> Review {
    Review {
        id: at,
        cid,
        last_ivl: 0,
        ivl: None,
        time_ms: 5_000,
        kind: 1,
    }
}

pub(super) fn moments(times: &[i64]) -> Vec<Review> {
    times
        .iter()
        .enumerate()
        .map(|(index, at)| moment(*at, index as i64))
        .collect()
}

pub(super) fn kept(clock: &ClockHistory, reviews: &[Review], now: i64) -> Profile {
    compute_with_history(
        "a",
        "a",
        reviews,
        clock,
        &Week::default(),
        now,
        &FreezePolicy::default(),
    )
}

/// One answer per minute, each taking the full minute.
pub(super) fn minutes_on(day: i64, minutes: i64) -> Vec<Review> {
    (0..minutes)
        .map(|i| {
            let id = day * DAY_MS + NOON + i * 60_000;
            Review {
                id,
                cid: id,
                last_ivl: 0,
                ivl: None,
                time_ms: 60_000,
                kind: 1,
            }
        })
        .collect()
}

pub(super) fn study(days: &[(i64, i64)]) -> Vec<Review> {
    days.iter()
        .flat_map(|(day, minutes)| minutes_on(*day, *minutes))
        .collect()
}

pub(super) fn ruled(reviews: &[Review], now: i64, rule: &[(i64, u32)]) -> Profile {
    compute_with_rules(
        "a",
        "a",
        reviews,
        &ClockHistory::single(utc()),
        &Week::default(),
        now,
        &FreezePolicy::default(),
        &StreakRule {
            changes: rule
                .iter()
                .map(|(at, minutes)| StreakChange {
                    at: *at,
                    minutes: *minutes,
                })
                .collect(),
            ..StreakRule::default()
        },
    )
}
