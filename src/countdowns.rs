//! Countdowns to a player's events and exams, kept in the synced setting `countdowns`.
//!
//! The server stores the list and checks only that it is well formed; the limits are the key's
//! entry in `tests/fixtures/synced_settings.json`. Both clients compute days left, the nearest
//! event and the cards a day from the cases in `tests/fixtures/countdowns.json`.

use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::Value;

/// The only date format a countdown accepts, such as `2026-11-25`.
const DATE_FORMAT: &str = "%Y-%m-%d";

/// `YYYY-MM-DD` is ten characters; chrono alone would also accept `2026-1-5`.
const DATE_LEN: usize = 10;

/// How long a countdown list and its fields may be.
#[derive(Debug, Deserialize)]
pub(crate) struct Limits {
    max_events: usize,
    max_name_chars: usize,
    max_deck_chars: usize,
}

/// One event: a name, a date and optionally the full name of a deck to finish by then.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Countdown {
    name: String,
    date: String,
    #[serde(default)]
    deck: Option<String>,
}

impl Limits {
    /// Whether `value` is a countdown list within these limits.
    pub(crate) fn accepts(&self, value: &Value) -> bool {
        let Ok(events) = serde_json::from_value::<Vec<Countdown>>(value.clone()) else {
            return false;
        };
        events.len() <= self.max_events && events.iter().all(|event| self.allows(event))
    }

    fn allows(&self, event: &Countdown) -> bool {
        let name_ok = !event.name.trim().is_empty() && chars(&event.name) <= self.max_name_chars;
        let deck_ok = match &event.deck {
            None => true,
            Some(deck) => !deck.trim().is_empty() && chars(deck) <= self.max_deck_chars,
        };
        name_ok && deck_ok && valid_date(&event.date)
    }
}

/// Length in Unicode code points, as both clients count it.
fn chars(text: &str) -> usize {
    text.chars().count()
}

fn valid_date(date: &str) -> bool {
    date.len() == DATE_LEN && NaiveDate::parse_from_str(date, DATE_FORMAT).is_ok()
}

#[cfg(test)]
mod tests {
    //! A reference version of the client rules, so a mistake in the shared cases fails here
    //! before both clients copy it.

    use super::*;
    use chrono::{Duration, NaiveDateTime};

    const FIXTURE: &str = include_str!("../tests/fixtures/countdowns.json");

    #[derive(Deserialize)]
    struct Fixture {
        days: Vec<DaysCase>,
        nearest: Vec<NearestCase>,
        order: Vec<OrderCase>,
        search: Vec<SearchCase>,
        pace: Vec<PaceCase>,
    }

    #[derive(Deserialize)]
    struct DaysCase {
        name: String,
        now: String,
        day_start_hour: i64,
        date: String,
        expected: Days,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Days {
        state: State,
        days: i64,
    }

    #[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
    #[serde(rename_all = "snake_case")]
    enum State {
        Upcoming,
        Tomorrow,
        Today,
        Past,
    }

    #[derive(Deserialize)]
    struct Event {
        name: String,
        date: String,
    }

    #[derive(Deserialize)]
    struct NearestCase {
        name: String,
        today: String,
        events: Vec<Event>,
        expected: Option<String>,
    }

    #[derive(Deserialize)]
    struct OrderCase {
        name: String,
        events: Vec<Event>,
        expected: Vec<String>,
    }

    #[derive(Deserialize)]
    struct SearchCase {
        name: String,
        deck: String,
        days: i64,
        expected: String,
    }

    #[derive(Deserialize)]
    struct PaceCase {
        name: String,
        days: i64,
        remaining: Option<i64>,
        expected: Pace,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(tag = "state", rename_all = "snake_case")]
    enum Pace {
        PerDay { cards: i64 },
        Done,
        None,
    }

    fn fixture() -> Fixture {
        serde_json::from_str(FIXTURE).expect("tests/fixtures/countdowns.json is valid")
    }

    fn date(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, DATE_FORMAT).unwrap()
    }

    /// The Anki day of a local time: its date once the day start hour is taken off.
    fn anki_today(now: &str, day_start_hour: i64) -> NaiveDate {
        let now = NaiveDateTime::parse_from_str(now, "%Y-%m-%dT%H:%M").unwrap();
        (now - Duration::hours(day_start_hour)).date()
    }

    fn days(today: NaiveDate, event: NaiveDate) -> Days {
        let days = (event - today).num_days();
        let state = match days {
            ..0 => State::Past,
            0 => State::Today,
            1 => State::Tomorrow,
            _ => State::Upcoming,
        };
        Days { state, days }
    }

    fn escape(deck: &str) -> String {
        let mut out = String::new();
        for c in deck.chars() {
            if matches!(c, '\\' | '"' | '*' | '_') {
                out.push('\\');
            }
            out.push(c);
        }
        out
    }

    fn pace(days: i64, remaining: Option<i64>) -> Pace {
        let Some(remaining) = remaining else {
            return Pace::None;
        };
        if days < 1 {
            return Pace::None;
        }
        if remaining == 0 {
            return Pace::Done;
        }
        Pace::PerDay {
            cards: (remaining + days - 1) / days,
        }
    }

    #[test]
    fn days_follow_the_shared_cases() {
        for case in fixture().days {
            let today = anki_today(&case.now, case.day_start_hour);
            assert_eq!(
                days(today, date(&case.date)),
                case.expected,
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn the_nearest_event_follows_the_shared_cases() {
        for case in fixture().nearest {
            let today = date(&case.today);
            let nearest = case
                .events
                .iter()
                .map(|event| (days(today, date(&event.date)), &event.name))
                .filter(|(days, _)| days.state != State::Past)
                .min_by_key(|(days, _)| days.days)
                .map(|(_, name)| name.clone());
            assert_eq!(nearest, case.expected, "{}", case.name);
        }
    }

    #[test]
    fn the_list_order_follows_the_shared_cases() {
        for case in fixture().order {
            let mut events = case.events;
            events.sort_by_key(|event| date(&event.date));
            let names: Vec<_> = events.into_iter().map(|event| event.name).collect();
            assert_eq!(names, case.expected, "{}", case.name);
        }
    }

    #[test]
    fn searches_follow_the_shared_cases() {
        for case in fixture().search {
            let search = format!(
                "deck:\"{}\" -is:suspended (is:new OR is:learn OR prop:due<{})",
                escape(&case.deck),
                case.days
            );
            assert_eq!(search, case.expected, "{}", case.name);
        }
    }

    #[test]
    fn pace_follows_the_shared_cases() {
        for case in fixture().pace {
            assert_eq!(
                pace(case.days, case.remaining),
                case.expected,
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn every_example_event_is_a_valid_countdown() {
        let settings: Value =
            serde_json::from_str(include_str!("../tests/fixtures/synced_settings.json")).unwrap();
        let key = settings["keys"]
            .as_array()
            .unwrap()
            .iter()
            .find(|key| key["key"] == "countdowns")
            .unwrap();
        let limits: Limits = serde_json::from_value(key.clone()).unwrap();
        let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
        for section in ["nearest", "order"] {
            for case in fixture[section].as_array().unwrap() {
                assert!(limits.accepts(&case["events"]), "{}", case["name"]);
            }
        }
    }
}
