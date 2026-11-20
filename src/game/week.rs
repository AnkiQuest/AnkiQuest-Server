//! The leaderboard week, and the week around its switch that a player's own day counts toward.

use super::*;

/// The leaderboard week shared by every player: Monday to Sunday in one time zone,
/// turning over at the same hour for everyone. The default board, [`Week::BOARD`], switches at
/// Monday 00:00 in UTC-12, the last time zone to reach Monday, which is Monday 12:00 UTC.
/// A player's own Anki days count toward the week of their own date; see [`monday_of`].
#[derive(Clone, Copy, Debug)]
pub struct Week {
    pub tz: Tz,
    pub rollover_hour: u32,
}

/// The week most tests are written against: UTC with a 04:00 rollover.
#[cfg(test)]
impl Default for Week {
    fn default() -> Self {
        Self {
            tz: Tz::UTC,
            rollover_hour: 4,
        }
    }
}

impl Week {
    /// The UTC hour on Mondays at which the default board switches weeks.
    pub const RESET_HOUR: u32 = 12;

    /// The default board: Monday 12:00 UTC, which is Monday 00:00 in UTC-12.
    pub const BOARD: Self = Self {
        tz: Tz::UTC,
        rollover_hour: Self::RESET_HOUR,
    };

    /// The common competition date, independent of any player's Anki clock.
    pub fn competition_date(&self, ms: i64) -> NaiveDate {
        self.shifted(ms).date()
    }

    /// Resolve a calendar rollover in the configured time zone. On a spring
    /// daylight-saving gap, use the first valid local minute after the gap;
    /// on a repeated fall-back hour, use its first occurrence.
    pub fn boundary_ms(&self, date: NaiveDate) -> i64 {
        let local = date.and_hms_opt(self.rollover_hour.min(23), 0, 0).unwrap();
        for minute in 0..=180 {
            if let Some(at) = self
                .tz
                .from_local_datetime(&(local + Duration::minutes(minute)))
                .earliest()
            {
                return at.timestamp_millis();
            }
        }
        local.and_utc().timestamp_millis()
    }

    pub(super) fn shifted(&self, ms: i64) -> NaiveDateTime {
        DateTime::from_timestamp_millis(ms)
            .unwrap_or_default()
            .with_timezone(&self.tz)
            .naive_local()
            - Duration::hours(i64::from(self.rollover_hour))
    }

    /// The Monday of the week the board shows at `ms`, as a day number like [`Clock::day`].
    pub(super) fn monday(&self, ms: i64) -> i64 {
        let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
        monday_of((self.shifted(ms).date() - epoch).num_days())
    }

    /// When the week containing `ms` began, in milliseconds since the epoch.
    pub fn start_of(&self, ms: i64) -> i64 {
        let date = self.shifted(ms).date();
        self.boundary_ms(date - Duration::days(i64::from(date.weekday().num_days_from_monday())))
    }

    /// When the week containing `ms` ends, in milliseconds since the epoch.
    pub fn end_after(&self, ms: i64) -> i64 {
        let date = self.shifted(ms).date();
        let monday = date + Duration::days(7 - i64::from(date.weekday().num_days_from_monday()));
        self.boundary_ms(monday)
    }
}

/// An Anki day counts toward the week its own date is in, on the player's clock, but the
/// board switches weeks for everyone at one instant. Around the switch a player's current
/// day can count toward the week before the board's or the week after it. Serialized as
/// `closing_week` or `next_week`.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnWeek {
    /// A Sunday still running after the switch (UTC-12 with a 04:00 rollover).
    #[serde(rename = "closing_week")]
    Closing(ClosingWeek),
    /// A Monday that began before the switch (Berlin on Monday morning).
    #[serde(rename = "next_week")]
    Next(NextWeek),
}

/// The week the board has just left, still counting the player's current day while
/// `week_xp` already shows the new one.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClosingWeek {
    /// XP that counts toward the closing week, today's included.
    pub xp: u64,
    /// When it closes for this player: the start of their own Monday (ms).
    pub ends_at: i64,
}

/// The week after the board's, which the player's current day already counts toward.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct NextWeek {
    /// XP that counts toward the next week, today's included.
    pub xp: u64,
    /// When the board switches to it (ms).
    pub starts_at: i64,
}

/// The Monday of the week `day` is in, as a day number. Day 0, 1970-01-01, was a Thursday.
pub(crate) fn monday_of(day: i64) -> i64 {
    day - (day + 3).rem_euclid(7)
}

pub(super) fn is_weekend(day: i64) -> bool {
    (day + 3).rem_euclid(7) >= 5
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::fixtures::*;

    #[test]
    fn week_is_shared_across_time_zones() {
        let berlin = chrono_tz::Europe::Berlin;
        let week = Week {
            tz: berlin,
            rollover_hour: 4,
        };
        let at_berlin = |d, h| {
            berlin
                .with_ymd_and_hms(2026, 9, d, h, 0, 0)
                .unwrap()
                .timestamp_millis()
        };
        let buenos_aires = Clock {
            offset_west_min: 180,
            rollover_hour: 4,
        };
        let review = |id| Review {
            id,
            cid: id,
            last_ivl: 0,
            ivl: None,
            ease: None,
            time_ms: 5_000,
            kind: 1,
        };

        assert_eq!(
            week.monday(at_berlin(21, 3)),
            week.monday(at_berlin(20, 12))
        );
        assert_ne!(
            week.monday(at_berlin(21, 5)),
            week.monday(at_berlin(20, 12))
        );
        assert_eq!(week.end_after(at_berlin(21, 10)), at_berlin(28, 4));
        assert_eq!(week.end_after(at_berlin(21, 3)), at_berlin(21, 4));
        assert_eq!(week.start_of(at_berlin(21, 10)), at_berlin(21, 4));
        assert_eq!(week.start_of(at_berlin(27, 23)), at_berlin(21, 4));
        assert_eq!(week.start_of(at_berlin(21, 3)), at_berlin(14, 4));

        let sunday = at_berlin(20, 20);
        let late_sunday = at_berlin(21, 8);
        let p = compute(
            "a",
            "a",
            &[review(sunday), review(late_sunday)],
            &buenos_aires,
            &week,
            late_sunday,
        );
        assert_eq!(
            p.week_xp, 0,
            "their Sunday started before the shared week did"
        );
        assert_eq!(p.today.reviews, 2);

        let monday = at_berlin(21, 10);
        let p = compute(
            "a",
            "a",
            &[review(sunday), review(monday)],
            &buenos_aires,
            &week,
            monday,
        );
        assert!(
            p.week_xp > 0,
            "their Monday started after the shared week did"
        );
    }

    #[test]
    fn closing_week_counts_until_the_players_own_day_ends() {
        // Production: the week resets for everyone on Monday at 00:00 UTC.
        let week = Week {
            tz: Tz::UTC,
            rollover_hour: 0,
        };
        let utc = |d, h| {
            Tz::UTC
                .with_ymd_and_hms(2026, 9, d, h, 0, 0)
                .unwrap()
                .timestamp_millis()
        };
        let reset = utc(21, 0);
        assert_eq!(week.start_of(reset), reset);
        // California (UTC-8 here) with a 04:00 rollover: Sunday runs to Monday 12:00 UTC.
        let california = Clock {
            offset_west_min: 480,
            rollover_hour: 4,
        };
        // Berlin in summer (UTC+2) with a 04:00 rollover: Sunday ends Monday 02:00 UTC.
        let berlin = Clock {
            offset_west_min: -120,
            rollover_hour: 4,
        };
        let review = |id| Review {
            id,
            cid: id,
            last_ivl: 0,
            ivl: None,
            ease: None,
            time_ms: 5_000,
            kind: 1,
        };
        let saturday = utc(19, 20);
        let sunday_evening = utc(21, 5);

        let reviews = [review(saturday), review(sunday_evening)];
        let p = compute("a", "a", &reviews, &california, &week, sunday_evening);
        assert_eq!(p.week_xp, 0, "the board has already reset");
        let closing = closing_of(&p).expect("their Sunday still counts");
        assert_eq!(closing.ends_at, utc(21, 12));
        assert_eq!(closing.ends_at, p.day_ends_at);
        let before = compute("a", "a", &reviews, &california, &week, reset - 1);
        assert_eq!(closing_of(&before), None, "nothing has reset yet");
        assert_eq!(closing.xp, before.week_xp);
        assert!(closing.xp > 0);
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["closing_week"]["xp"], closing.xp);
        assert_eq!(json["closing_week"]["ends_at"], utc(21, 12));

        let monday = utc(21, 12);
        let p = compute("a", "a", &reviews, &california, &week, monday);
        assert_eq!(closing_of(&p), None, "gone once their Sunday ends");
        assert!(
            serde_json::to_value(&p)
                .unwrap()
                .get("closing_week")
                .is_none()
        );

        // At 05:00 UTC Berlin's Monday began three hours ago, inside the new week.
        let p = compute(
            "b",
            "b",
            &[review(saturday)],
            &berlin,
            &week,
            sunday_evening,
        );
        assert_eq!(closing_of(&p), None);
    }

    #[test]
    fn weekly_board_fixture() {
        use weekly_fixture::CountsToward;

        let week = Week::BOARD;
        for case in weekly_fixture::cases() {
            let name = &case.name;
            let expected = &case.expected;
            assert_eq!(
                week.start_of(case.now_ms),
                expected.board_starts_at,
                "{name}"
            );
            assert_eq!(
                week.end_after(case.now_ms),
                expected.board_ends_at,
                "{name}"
            );

            // One review now: its XP shows where the player's current day counts.
            let review = Review {
                id: case.now_ms,
                cid: 1,
                last_ivl: 0,
                ivl: None,
                ease: None,
                time_ms: 5_000,
                kind: 1,
            };
            let p = compute("a", "a", &[review], &case.clock, &week, case.now_ms);
            let json = serde_json::to_value(&p).unwrap();
            let today = p.periods.day;
            assert!(today > 0, "{name}");
            let (board, closing, next) = match expected.counts_toward {
                CountsToward::Board => (today, None, None),
                CountsToward::Closing => (0, Some(expected.own_week_ends_at), None),
                CountsToward::Next => (0, None, Some(expected.board_ends_at)),
            };
            assert_eq!(p.week_xp, board, "{name}: week_xp");
            assert_eq!(
                json.get("closing_week").map(|c| c["ends_at"].as_i64()),
                closing.map(Some),
                "{name}: closing_week"
            );
            if closing.is_some() {
                assert_eq!(json["closing_week"]["xp"], today, "{name}");
            }
            assert_eq!(
                json.get("next_week").map(|n| n["starts_at"].as_i64()),
                next.map(Some),
                "{name}: next_week"
            );
            if next.is_some() {
                assert_eq!(json["next_week"]["xp"], today, "{name}");
            }
        }
    }
}

/// The cases in `tests/fixtures/weekly_board.json`, shared by the game and reminder tests.
#[cfg(test)]
pub(crate) mod weekly_fixture {
    use super::Clock;
    use serde::Deserialize;

    /// Which week the player's current Anki day counts toward, next to the board's.
    #[derive(Deserialize, Clone, Copy)]
    pub(crate) enum CountsToward {
        #[serde(rename = "board_week")]
        Board,
        #[serde(rename = "closing_week")]
        Closing,
        #[serde(rename = "next_week")]
        Next,
    }

    #[derive(Deserialize)]
    pub(crate) struct Expected {
        pub board_starts_at: i64,
        pub board_ends_at: i64,
        pub counts_toward: CountsToward,
        pub own_week_ends_at: i64,
    }

    #[derive(Deserialize)]
    pub(crate) struct Case {
        pub name: String,
        pub now_ms: i64,
        pub clock: Clock,
        pub expected: Expected,
    }

    #[derive(Deserialize)]
    struct Fixture {
        cases: Vec<Case>,
    }

    pub(crate) fn cases() -> Vec<Case> {
        let fixture: Fixture =
            serde_json::from_str(include_str!("../../tests/fixtures/weekly_board.json")).unwrap();
        fixture.cases
    }
}
