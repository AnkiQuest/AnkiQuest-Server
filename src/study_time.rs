//! Checks `tests/fixtures/study_time.json` against its own rule, so a case
//! cannot contradict the rule the clients implement. The server does not
//! compute study time; the Android app and the desktop add-on do, from the
//! local review log.

use crate::game::Clock;
use chrono::{Datelike, Days, NaiveDate};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Revlog types from learning (0) to filtered (3); manual (4) and rescheduled (5) are not study.
const LAST_STUDY_TYPE: i64 = 3;
const MINUTE_MS: i64 = 60_000;
const HOUR_MINUTES: i64 = 60;

fn date(day: i64) -> NaiveDate {
    NaiveDate::from_ymd_opt(1970, 1, 1)
        .unwrap()
        .checked_add_days(Days::new(u64::try_from(day).unwrap()))
        .unwrap()
}

/// Study time per study day up to `today`, from the counted reviews.
fn days(case: &Value, clock: &Clock, today: i64) -> BTreeMap<i64, i64> {
    let mut days = BTreeMap::new();
    for review in case["reviews"].as_array().unwrap() {
        let id = review["id"].as_i64().unwrap();
        let counted = id >= 0
            && review["ease"].as_i64().unwrap() > 0
            && review["type"].as_i64().unwrap() <= LAST_STUDY_TYPE;
        if !counted {
            continue;
        }

        // A clock moved back can leave reviews on a later day than now's.
        let day = clock.day(id);
        if day > today {
            continue;
        }
        *days.entry(day).or_insert(0) += review["time_ms"].as_i64().unwrap();
    }
    days
}

fn totals(case: &Value) -> Value {
    let clock: Clock = serde_json::from_value(case["clock"].clone()).unwrap();
    let today = clock.day(case["now_ms"].as_i64().unwrap());
    let days = days(case, &clock, today);

    // Weeks start on Monday; the month is today's calendar month.
    let shown = date(today);
    let monday = today - i64::from(shown.weekday().num_days_from_monday());
    let month = (shown.year(), shown.month());
    let sum = |keep: &dyn Fn(i64) -> bool| -> i64 {
        days.iter()
            .filter(|(day, _)| keep(**day))
            .map(|(_, ms)| ms)
            .sum()
    };
    let total = sum(&|_| true);
    let study_days = i64::try_from(days.len()).unwrap();
    json!({
        "today": shown.to_string(),
        "today_ms": sum(&|day| day == today),
        "week_ms": sum(&|day| day >= monday),
        "month_ms": sum(&|day| (date(day).year(), date(day).month()) == month),
        "total_ms": total,
        "study_days": study_days,
        "average_ms": if study_days > 0 { total / study_days } else { 0 },
    })
}

#[test]
fn study_time_cases_follow_the_rule() {
    let fixture: Value =
        serde_json::from_str(include_str!("../tests/fixtures/study_time.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        assert_eq!(totals(case), case["expected"], "{}", case["name"]);
    }

    // Minutes round down; hours show from 60 minutes on.
    for case in fixture["durations"].as_array().unwrap() {
        let minutes = case["ms"].as_i64().unwrap() / MINUTE_MS;
        assert_eq!(
            (minutes / HOUR_MINUTES, minutes % HOUR_MINUTES),
            (
                case["hours"].as_i64().unwrap(),
                case["minutes"].as_i64().unwrap()
            ),
            "{case}"
        );
    }
}
