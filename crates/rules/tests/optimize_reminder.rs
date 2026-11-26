//! Every case in `tests/fixtures/optimize_reminder.json`, through the optimize reminder rule, and
//! the `optimize_reminder` row of `tests/fixtures/synced_settings.json`.

use ankiquest_rules::{
    OPTIMIZE_DAYS, OPTIMIZE_REVIEWS, OPTIMIZE_SNOOZE_DAYS, OptimizeEvent, OptimizeInputs,
    OptimizeSince, OptimizeState, OptimizeVerdict, OptimizeWording, PresetFlags, PresetScheduler,
    answers_start_ms, is_answer, optimize_next, optimize_verdict, preset_scheduler,
};
use serde_json::{Value, json};
use std::path::PathBuf;

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name);
    let text = std::fs::read_to_string(&path).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn reminder() -> Value {
    fixture("optimize_reminder.json")
}

fn cases<'a>(fixture: &'a Value, section: &str) -> &'a Vec<Value> {
    let cases = fixture[section].as_array().unwrap();
    assert!(!cases.is_empty(), "{section}");
    cases
}

fn day(v: &Value) -> i32 {
    i32::try_from(v.as_i64().unwrap()).unwrap()
}

fn last(case: &Value) -> Option<i32> {
    case["last_optimized_day"]
        .as_i64()
        .map(|n| i32::try_from(n).unwrap())
}

fn state(v: &Value) -> OptimizeState {
    serde_json::from_value(v.clone()).unwrap()
}

fn verdict_json(verdict: OptimizeVerdict) -> Value {
    let quiet = match verdict {
        OptimizeVerdict::Off => "off",
        OptimizeVerdict::NoFsrs => "no_fsrs",
        OptimizeVerdict::Snoozed => "snoozed",
        OptimizeVerdict::Dismissed => "dismissed",
        OptimizeVerdict::TooSoon => "too_soon",
        OptimizeVerdict::TooFewReviews => "too_few_reviews",
        OptimizeVerdict::Remind { since, wording } => {
            return json!({"verdict": "remind", "since": since, "scheduler": wording});
        }
    };
    json!({ "verdict": quiet })
}

#[test]
fn the_limits_are_the_fixtures() {
    let limits = &reminder()["limits"];
    assert_eq!(day(&limits["days"]), OPTIMIZE_DAYS);
    assert_eq!(
        limits["reviews"].as_u64(),
        Some(u64::from(OPTIMIZE_REVIEWS))
    );
    assert_eq!(day(&limits["snooze_days"]), OPTIMIZE_SNOOZE_DAYS);
}

#[test]
fn schedulers_follow_the_cases() {
    let fixture = reminder();
    for case in cases(&fixture, "schedulers") {
        let flag = |key: &str| case[key].as_bool().unwrap();
        let found = preset_scheduler(PresetFlags {
            fsrs: flag("fsrs"),
            rwkv_review: flag("rwkvReviewEnabled"),
            rwkv_instant_order: flag("rwkvReviewInstantOrderEnabled"),
        });
        assert_eq!(json!(found), case["expected"], "{}", case["name"]);
    }
}

#[test]
fn answers_since_follow_the_cases() {
    let fixture = reminder();
    for case in cases(&fixture, "reviews") {
        let start = answers_start_ms(
            day(&case["today"]),
            case["day_cutoff"].as_i64().unwrap(),
            last(case),
        );
        let counted = case["reviews"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|review| {
                review["id"].as_i64().unwrap() >= start
                    && is_answer(day(&review["type"]), day(&review["ease"]))
            })
            .count();
        assert_eq!(json!(counted), case["expected"], "{}", case["name"]);
    }
}

#[test]
fn verdicts_follow_the_cases() {
    let fixture = reminder();
    for case in cases(&fixture, "cases") {
        let presets: Vec<PresetScheduler> =
            serde_json::from_value(case["presets"].clone()).unwrap();
        let inputs = OptimizeInputs {
            today: day(&case["today"]),
            last_optimized_day: last(case),
            reviews_since: u32::try_from(case["reviews_since"].as_u64().unwrap()).unwrap(),
            presets,
        };
        let found = optimize_verdict(inputs, state(&case["state"]));
        assert_eq!(verdict_json(found), case["expected"], "{}", case["name"]);
    }
}

#[test]
fn transitions_follow_the_cases() {
    let fixture = reminder();
    for case in cases(&fixture, "transitions") {
        let event: OptimizeEvent = serde_json::from_value(case["event"].clone()).unwrap();
        let found = optimize_next(
            state(&case["state"]),
            event,
            day(&case["today"]),
            last(case),
        );
        assert_eq!(json!(found), case["expected"], "{}", case["name"]);
    }
}

#[test]
fn every_verdict_and_wording_has_a_case() {
    let fixture = reminder();
    let verdicts: Vec<&Value> = cases(&fixture, "cases")
        .iter()
        .map(|case| &case["expected"])
        .collect();
    for quiet in [
        OptimizeVerdict::Off,
        OptimizeVerdict::NoFsrs,
        OptimizeVerdict::Snoozed,
        OptimizeVerdict::Dismissed,
        OptimizeVerdict::TooSoon,
        OptimizeVerdict::TooFewReviews,
    ] {
        let wanted = verdict_json(quiet);
        assert!(verdicts.contains(&&wanted), "{wanted}");
    }
    for since in [OptimizeSince::Stale, OptimizeSince::Never] {
        for wording in [
            OptimizeWording::Fsrs,
            OptimizeWording::Rwkv,
            OptimizeWording::Mixed,
        ] {
            let wanted = verdict_json(OptimizeVerdict::Remind { since, wording });
            assert!(verdicts.contains(&&wanted), "{wanted}");
        }
    }
}

#[test]
fn the_wording_names_the_limits() {
    let fixture = reminder();
    let wording = &fixture["wording"];

    // The text states the limits in words; a change to them changes the text too.
    assert_eq!((OPTIMIZE_DAYS, OPTIMIZE_REVIEWS), (30, 1000));
    for key in ["stale", "setting_summary"] {
        let text = wording[key].as_str().unwrap();
        assert!(
            text.contains("30 days") && text.contains("1,000 reviews"),
            "{key}"
        );
    }
    assert!(wording["never"].as_str().unwrap().contains("1,000 reviews"));
    assert_eq!(OPTIMIZE_SNOOZE_DAYS, 7, "snooze says \"in a week\"");
    assert!(wording["snooze"].as_str().unwrap().ends_with("in a week"));
}

#[test]
fn the_synced_setting_holds_the_state() {
    let settings = fixture("synced_settings.json");
    let row = settings["keys"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["key"] == "optimize_reminder")
        .unwrap();
    assert_eq!(row["type"], "optimize_reminder");
    assert_eq!(state(&row["default"]), OptimizeState::On);

    // Every state the cases name is one the setting stores, and reads back the same.
    let fixture = reminder();
    let named = cases(&fixture, "cases")
        .iter()
        .map(|case| &case["state"])
        .chain(
            cases(&fixture, "transitions")
                .iter()
                .map(|case| &case["expected"]),
        );
    for found in named {
        let read = state(found);
        assert!(read.valid(), "{found}");
        assert_eq!(&json!(read), found);
    }
}

#[test]
fn a_stored_state_has_the_fixtures_shape() {
    let refused = [
        json!({"kind": "later"}),
        json!({"kind": "snoozed"}),
        json!({"kind": "snoozed", "until_day": "3"}),
        json!({"kind": "snoozed", "until_day": 3.5}),
        json!({"kind": "snoozed", "until_day": 2_147_483_648_i64}),
        json!({"until_day": 3}),
        json!(true),
    ];
    for value in refused {
        assert!(
            serde_json::from_value::<OptimizeState>(value.clone()).is_err(),
            "{value}"
        );
    }
    for value in [
        json!({"kind": "snoozed", "until_day": -1}),
        json!({"kind": "dismissed", "optimized_day": 0}),
    ] {
        assert!(!state(&value).valid(), "{value}");
    }
    assert!(state(&json!({"kind": "snoozed", "until_day": 0})).valid());
}
