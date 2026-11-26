//! Checks `tests/fixtures/optimize_reminder.json` against its own rule, so a case
//! cannot contradict the rule the clients implement. The server does not remind
//! anyone to optimize; the Android app and the desktop add-on do, from the
//! collection's deck config and review log.

use serde::Deserialize;
use serde_json::{Value, json};

const DAY_SECS: i64 = 86_400;
const SECOND_MS: i64 = 1_000;
/// Revlog types from learning (0) to filtered (3); manual (4) and rescheduled (5) are not answers.
const LAST_ANSWER_TYPE: i64 = 3;

#[derive(Deserialize)]
struct Limits {
    days: i64,
    reviews: i64,
    snooze_days: i64,
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Scheduler {
    Sm2,
    Fsrs,
    RwkvCurve,
    RwkvInstant,
}

/// The reminder's state on one device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum State {
    On,
    Off,
    Snoozed { until_day: i64 },
    Dismissed { optimized_day: Option<i64> },
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Event {
    Snooze,
    Dismiss,
    TurnOff,
    TurnOn,
}

fn state_json(state: State) -> Value {
    match state {
        State::On => json!({"kind": "on"}),
        State::Off => json!({"kind": "off"}),
        State::Snoozed { until_day } => json!({"kind": "snoozed", "until_day": until_day}),
        State::Dismissed { optimized_day } => {
            json!({"kind": "dismissed", "optimized_day": optimized_day})
        }
    }
}

fn scheduler(case: &Value) -> &'static str {
    let flag = |key: &str| case[key].as_bool().unwrap();
    if !flag("fsrs") {
        return "sm2";
    }
    if flag("rwkvReviewInstantOrderEnabled") {
        return "rwkv_instant";
    }
    if flag("rwkvReviewEnabled") {
        return "rwkv_curve";
    }
    "fsrs"
}

fn answers_since(case: &Value) -> i64 {
    // Answers on the days after the last optimization, or every answer.
    let start_ms = case["last_optimized_day"].as_i64().map_or(0, |last| {
        let days = case["today"].as_i64().unwrap() - last;
        (case["day_cutoff"].as_i64().unwrap() - days * DAY_SECS) * SECOND_MS
    });
    let counted = case["reviews"].as_array().unwrap().iter().filter(|review| {
        review["id"].as_i64().unwrap() >= start_ms
            && review["ease"].as_i64().unwrap() > 0
            && review["type"].as_i64().unwrap() <= LAST_ANSWER_TYPE
    });
    i64::try_from(counted.count()).unwrap()
}

fn verdict(case: &Value, limits: &Limits) -> Value {
    let today = case["today"].as_i64().unwrap();
    let last = case["last_optimized_day"].as_i64();
    let state: State = serde_json::from_value(case["state"].clone()).unwrap();
    let presets: Vec<Scheduler> = serde_json::from_value(case["presets"].clone()).unwrap();
    let optimized: Vec<Scheduler> = presets
        .into_iter()
        .filter(|preset| *preset != Scheduler::Sm2)
        .collect();

    // The first rule that holds names the verdict.
    let quiet = match state {
        State::Off => Some("off"),
        _ if optimized.is_empty() => Some("no_fsrs"),
        State::Snoozed { until_day } if today < until_day => Some("snoozed"),
        State::Dismissed { optimized_day } if optimized_day == last => Some("dismissed"),
        _ if last.is_some_and(|last| today - last < limits.days) => Some("too_soon"),
        _ if case["reviews_since"].as_i64().unwrap() < limits.reviews => Some("too_few_reviews"),
        _ => None,
    };
    if let Some(quiet) = quiet {
        return json!({"verdict": quiet});
    }

    let fsrs = optimized
        .iter()
        .filter(|preset| **preset == Scheduler::Fsrs)
        .count();
    let scheduler = match fsrs {
        0 => "rwkv",
        all if all == optimized.len() => "fsrs",
        _ => "mixed",
    };
    let since = if last.is_some() { "stale" } else { "never" };
    json!({"verdict": "remind", "since": since, "scheduler": scheduler})
}

fn transition(case: &Value, limits: &Limits) -> State {
    let state: State = serde_json::from_value(case["state"].clone()).unwrap();
    let event: Event = serde_json::from_value(case["event"].clone()).unwrap();
    let today = case["today"].as_i64().unwrap();
    match (state, event) {
        (_, Event::TurnOff) => State::Off,
        (State::Off, Event::TurnOn) => State::On,
        (State::Off, _) | (_, Event::TurnOn) => state,
        (_, Event::Snooze) => State::Snoozed {
            until_day: today + limits.snooze_days,
        },
        (_, Event::Dismiss) => State::Dismissed {
            optimized_day: case["last_optimized_day"].as_i64(),
        },
    }
}

#[test]
fn optimize_reminder_cases_follow_the_rule() {
    let fixture: Value =
        serde_json::from_str(include_str!("../tests/fixtures/optimize_reminder.json")).unwrap();
    let limits: Limits = serde_json::from_value(fixture["limits"].clone()).unwrap();

    for (section, check) in [
        (
            "schedulers",
            &(|case: &Value| json!(scheduler(case))) as &dyn Fn(&Value) -> Value,
        ),
        ("reviews", &|case: &Value| json!(answers_since(case))),
        ("cases", &|case: &Value| verdict(case, &limits)),
        ("transitions", &|case: &Value| {
            state_json(transition(case, &limits))
        }),
    ] {
        let cases = fixture[section].as_array().unwrap();
        assert!(!cases.is_empty(), "{section}");
        for case in cases {
            assert_eq!(check(case), case["expected"], "{section}: {}", case["name"]);
        }
    }

    // Every verdict and every wording choice has a case.
    let verdicts: Vec<Value> = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| case["expected"].clone())
        .collect();
    for verdict in [
        "off",
        "no_fsrs",
        "snoozed",
        "dismissed",
        "too_soon",
        "too_few_reviews",
    ] {
        assert!(
            verdicts.iter().any(|found| found["verdict"] == verdict),
            "{verdict}"
        );
    }
    for since in ["stale", "never"] {
        for scheduler in ["fsrs", "rwkv", "mixed"] {
            let wanted = json!({"verdict": "remind", "since": since, "scheduler": scheduler});
            assert!(verdicts.contains(&wanted), "{wanted}");
        }
    }
}

#[test]
fn optimize_reminder_wording_names_the_limits() {
    let fixture: Value =
        serde_json::from_str(include_str!("../tests/fixtures/optimize_reminder.json")).unwrap();
    let limits: Limits = serde_json::from_value(fixture["limits"].clone()).unwrap();
    let wording = &fixture["wording"];

    // The text states the limits in words; a change to them changes the text too.
    assert_eq!((limits.days, limits.reviews), (30, 1000));
    for key in ["stale", "setting_summary"] {
        let text = wording[key].as_str().unwrap();
        assert!(
            text.contains("30 days") && text.contains("1,000 reviews"),
            "{key}"
        );
    }
    assert!(wording["never"].as_str().unwrap().contains("1,000 reviews"));
    assert_eq!(limits.snooze_days, 7, "snooze says \"in a week\"");
    assert!(wording["snooze"].as_str().unwrap().ends_with("in a week"));
}
