//! Every case in `tests/fixtures/day_verdicts.json`, through [`today_verdict`].

use ankiquest_rules::{
    DayLabel, DayVerdict, PendingReview, ServerDay, ServerToday, StreakState, TodayVerdict,
    today_verdict,
};
use serde_json::{Value, json};
use std::path::PathBuf;

fn fixture() -> Value {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/day_verdicts.json");
    let text = std::fs::read_to_string(&path).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn int(v: &Value) -> i64 {
    v.as_i64().unwrap()
}

fn small(v: &Value) -> u32 {
    u32::try_from(v.as_u64().unwrap()).unwrap()
}

fn state(v: &Value) -> StreakState {
    match v.as_str().unwrap() {
        "studied" => StreakState::Studied,
        "unverified" => StreakState::Unverified,
        "pending" => StreakState::Pending,
        "protected" => StreakState::Protected,
        other => panic!("unknown streak_state {other}"),
    }
}

fn state_name(s: StreakState) -> &'static str {
    match s {
        StreakState::Studied => "studied",
        StreakState::Unverified => "unverified",
        StreakState::Pending => "pending",
        StreakState::Protected => "protected",
    }
}

fn label_name(l: DayLabel) -> &'static str {
    match l {
        DayLabel::Studied => "studied",
        DayLabel::Unverified => "unverified",
        DayLabel::Protected => "protected",
        DayLabel::Short => "short",
        DayLabel::Pending => "pending",
        DayLabel::Unknown => "unknown",
    }
}

fn server(v: &Value) -> ServerDay {
    let today = &v["today"];
    ServerDay {
        streak: small(&v["streak"]),
        streak_state: state(&v["streak_state"]),
        day_ends_at: int(&v["day_ends_at"]),
        last_review_id: int(&v["last_review_id"]),
        today: ServerToday {
            starts_at: int(&today["starts_at"]),
            reviews: small(&today["reviews"]),
            observed: small(&today["observed"]),
            time_ms: int(&today["time_ms"]),
            minutes: small(&today["minutes"]),
            streak_minutes: small(&today["streak_minutes"]),
        },
    }
}

fn pending(v: &Value) -> Vec<PendingReview> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|r| PendingReview {
            id: int(&r["id"]),
            time_ms: r.get("time_ms").map(int),
        })
        .collect()
}

/// The verdict in the fixture's own shape: null for a day that has ended.
fn shape(verdict: DayVerdict) -> Value {
    let DayVerdict::Today { verdict } = verdict else {
        return Value::Null;
    };
    let TodayVerdict {
        met,
        streak_state,
        streak,
        reviews,
        observed,
        time_ms,
        minutes,
        streak_minutes,
        unsent,
        label,
    } = verdict;
    json!({
        "met": met,
        "streak_state": streak_state.map(state_name),
        "streak": streak,
        "reviews": reviews,
        "observed": observed,
        "time_ms": time_ms,
        "minutes": minutes,
        "streak_minutes": streak_minutes,
        "unsent": unsent,
        "label": label_name(label),
    })
}

#[test]
fn every_fixture_case_matches() {
    let fixture = fixture();
    let cases = fixture["cases"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let verdict = today_verdict(
            server(&case["server"]),
            pending(&case["pending"]),
            int(&case["now_ms"]),
        );
        assert_eq!(shape(verdict), case["expected"], "{}", case["name"]);
    }
}
