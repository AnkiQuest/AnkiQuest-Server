//! Study a client observed instead of reading the review log.
//!
//! AnkiQuest Lite reads stock AnkiDroid through its content provider, which has no review log. It
//! sees each card's state (`reps`, `type`, `interval`, `last_review_time_secs`) and how many cards a
//! day's `prop:rated` search finds, but never how long an answer took. The server keeps the last
//! state it was sent for each card and turns the difference into observed answers; a day's count
//! fills in cards no state covered. Exact review rows always win: they replace every observed answer
//! to the same card on the same day. The rules are the cases in `tests/fixtures/observed_reviews.json`.

use super::*;

/// The most answers one card can add between two polls; more are not counted.
pub const MAX_ANSWERS_PER_POLL: u32 = 50;

/// A card's type, as Anki stores it in `cards.type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "u8", into = "u8")]
pub enum CardType {
    New,
    Learning,
    Review,
    Relearning,
}

impl TryFrom<u8> for CardType {
    type Error = String;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(CardType::New),
            1 => Ok(CardType::Learning),
            2 => Ok(CardType::Review),
            3 => Ok(CardType::Relearning),
            other => Err(format!("unknown card type {other}")),
        }
    }
}

impl From<CardType> for u8 {
    fn from(kind: CardType) -> Self {
        match kind {
            CardType::New => 0,
            CardType::Learning => 1,
            CardType::Review => 2,
            CardType::Relearning => 3,
        }
    }
}

impl CardType {
    /// The revlog kind of an answer given to a card of this type.
    fn answer_kind(self) -> u8 {
        match self {
            CardType::New | CardType::Learning => 0,
            CardType::Review => 1,
            CardType::Relearning => 2,
        }
    }
}

/// What AnkiDroid's provider says of one card, as the client read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct CardState {
    pub reps: u32,
    #[serde(rename = "type")]
    pub kind: CardType,
    /// Days for a review card; Anki keeps 0 for new and learning cards.
    pub interval: i64,
    /// When the card was last answered; null for cards not answered since Anki began storing it.
    pub last_review_time_secs: Option<i64>,
}

impl CardState {
    fn last_review_ms(&self) -> Option<i64> {
        self.last_review_time_secs
            .filter(|secs| *secs > 0)
            .map(|secs| secs * 1000)
    }
}

/// How well an observed answer's time is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Timing {
    /// The card's last answer before the poll: its time is the card's last review time.
    Known,
    /// An earlier answer to the same card on the same day since the previous poll. When it
    /// happened is not known, so it sits just before the last one.
    Placed,
}

/// One answer the server worked out from two states of a card. Its duration is never known.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ObservedAnswer {
    pub cid: i64,
    pub at: i64,
    pub kind: u8,
    pub last_ivl: i64,
    pub ivl: Option<i64>,
    pub timing: Timing,
}

impl ObservedAnswer {
    fn review(&self) -> Review {
        Review {
            id: self.at,
            cid: self.cid,
            last_ivl: self.last_ivl,
            ivl: self.ivl,
            time_ms: 0,
            kind: self.kind,
        }
    }
}

/// What a card's new state says happened since the state before it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "change")]
pub enum Change {
    /// Nothing to count: the same state, or a card never answered.
    Unchanged,
    /// New answers, oldest first.
    Answered { answers: Vec<ObservedAnswer> },
    /// `reps` went down: the newest `count` answers to the card were undone.
    Undone { count: u32 },
    /// The card was reset to new. Its answers stay; nothing new is counted.
    Reset,
    /// More answers, but the card's last review time does not date them (it is missing or
    /// not later than before). The day's count from `prop:rated` covers them.
    Undated,
}

/// The last state the server kept of a card, and when the client read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Baseline {
    #[serde(flatten)]
    pub state: CardState,
    pub read_at_ms: i64,
}

/// What `after` says happened to card `cid` since `before`. `clock` dates the answers: earlier
/// answers are counted only when `before` was read on the day of the last answer, since otherwise
/// some of them may belong to the days in between, which their days' counts cover.
pub fn observe(
    cid: i64,
    before: Option<&Baseline>,
    after: &CardState,
    clock: &ClockHistory,
) -> Change {
    // A card reset to new has nothing new to count, whatever its counts say.
    if after.kind == CardType::New {
        return match before {
            Some(before) if before.state.kind != CardType::New => Change::Reset,
            _ => Change::Unchanged,
        };
    }

    // First sight: the card had at least its last answer.
    let Some(Baseline {
        state: before,
        read_at_ms,
    }) = before.copied()
    else {
        let Some(at) = after.last_review_ms().filter(|_| after.reps > 0) else {
            return Change::Unchanged;
        };
        return Change::Answered {
            answers: vec![ObservedAnswer {
                cid,
                at,
                kind: after.kind.answer_kind(),
                last_ivl: 0,
                ivl: (after.kind == CardType::Review).then_some(after.interval),
                timing: Timing::Known,
            }],
        };
    };

    if after.reps < before.reps {
        return Change::Undone {
            count: before.reps - after.reps,
        };
    }
    if after.reps == before.reps {
        return Change::Unchanged;
    }
    let Some(last_at) = after
        .last_review_ms()
        .filter(|at| before.last_review_ms().is_none_or(|prior| *at > prior))
    else {
        return Change::Undated;
    };

    // Answers between the two states, the last one dated by the card. The first is given to the
    // card as it was before; the later ones are steps of learning or relearning.
    let added = (after.reps - before.reps).min(MAX_ANSWERS_PER_POLL);
    let count = if clock.day(read_at_ms) == clock.day(last_at) {
        added
    } else {
        1
    };
    let step_kind = match before.kind {
        CardType::Review | CardType::Relearning => 2,
        CardType::New | CardType::Learning => 0,
    };
    let answers = (0..count)
        .map(|index| {
            let first = index == 0 && count == added;
            let last = index + 1 == count;
            ObservedAnswer {
                cid,
                at: last_at - i64::from(count - 1 - index),
                kind: if first {
                    before.kind.answer_kind()
                } else {
                    step_kind
                },
                last_ivl: if first && before.kind == CardType::Review {
                    before.interval
                } else {
                    0
                },
                ivl: (last && after.kind == CardType::Review).then_some(after.interval),
                timing: if last { Timing::Known } else { Timing::Placed },
            }
        })
        .collect();
    Change::Answered { answers }
}

/// Everything the server keeps of a player's observed study.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Observed {
    /// Answers worked out from card states, ordered by time.
    pub answers: Vec<ObservedAnswer>,
    /// Distinct cards a day's `prop:rated` search found, by day number.
    pub days: BTreeMap<i64, u64>,
}

impl Observed {
    pub fn is_empty(&self) -> bool {
        self.answers.is_empty() && self.days.is_empty()
    }
}

/// Where an answer the game counts comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// A review log row: time, duration and kind are known.
    Exact,
    /// An answer worked out from two states of a card; its duration is not known.
    Observed(Timing),
    /// A card a day's count found that nothing else covers: neither card nor time is known.
    /// It sits at the start of the day and earns the learning rate.
    Counted,
}

impl Source {
    /// Whether the answer's time of day is known, for quests and achievements about it.
    pub(super) fn timed(self) -> bool {
        matches!(self, Source::Exact | Source::Observed(Timing::Known))
    }
}

/// An answer as the game counts it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Answer {
    pub review: Review,
    pub source: Source,
}

/// Exact reviews with the observed study they do not cover, ordered by time. An exact row for
/// a card replaces every observed answer to that card on the same day, and a day's count only
/// adds the cards nothing else covered, so no answer is counted twice.
pub fn merge(exact: &[Review], observed: &Observed, clock: &ClockHistory) -> Vec<Answer> {
    let mut answers: Vec<Answer> = exact
        .iter()
        .map(|review| Answer {
            review: *review,
            source: Source::Exact,
        })
        .collect();
    if observed.is_empty() {
        return answers;
    }

    // Days and cards the observed study touches, and which of them exact rows cover.
    let touched: HashSet<(i64, i64)> = observed
        .answers
        .iter()
        .map(|answer| (clock.day(answer.at), answer.cid))
        .collect();
    let mut covered = HashSet::new();
    let mut cards: HashMap<i64, HashSet<i64>> = HashMap::new();
    for review in exact {
        let day = clock.day(review.id);
        if touched.contains(&(day, review.cid)) {
            covered.insert((day, review.cid));
        }
        if observed.days.contains_key(&day) {
            cards.entry(day).or_default().insert(review.cid);
        }
    }

    // Observed answers exact rows do not replace.
    for answer in &observed.answers {
        let day = clock.day(answer.at);
        if covered.contains(&(day, answer.cid)) {
            continue;
        }
        if observed.days.contains_key(&day) {
            cards.entry(day).or_default().insert(answer.cid);
        }
        answers.push(Answer {
            review: answer.review(),
            source: Source::Observed(answer.timing),
        });
    }

    // Cards a day's count found beyond those covered.
    for (&day, &count) in &observed.days {
        let known = cards.get(&day).map_or(0, |cards| cards.len() as u64);
        let at = clock.day_start_ms(day);
        for _ in known..count {
            answers.push(Answer {
                review: Review {
                    id: at,
                    cid: 0,
                    last_ivl: 0,
                    ivl: None,
                    time_ms: 0,
                    kind: 0,
                },
                source: Source::Counted,
            });
        }
    }
    answers.sort_by_key(|answer| answer.review.id);
    answers
}

/// Whether a period's XP includes answers whose time is not known.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum XpSource {
    /// All from review log rows.
    #[default]
    Exact,
    /// Some or all from observed answers.
    Observed,
}

/// [`XpSource`] for each of a profile's [`Periods`].
#[derive(Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct XpSources {
    pub hour: XpSource,
    pub day: XpSource,
    pub week: XpSource,
    pub month: XpSource,
    pub year: XpSource,
    pub all: XpSource,
}

impl XpSources {
    /// The source of a period named as in [`Periods::get`].
    pub fn get(&self, period: &str) -> XpSource {
        match period {
            "hour" => self.hour,
            "day" => self.day,
            "month" => self.month,
            "year" => self.year,
            "all" => self.all,
            _ => self.week,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    const FIXTURE: &str = include_str!("../../tests/fixtures/observed_reviews.json");

    fn fixture() -> Value {
        serde_json::from_str(FIXTURE).unwrap()
    }

    fn clock_of(case: &Value) -> ClockHistory {
        ClockHistory::single(serde_json::from_value(case["clock"].clone()).unwrap())
    }

    fn source_name(source: Source) -> &'static str {
        match source {
            Source::Exact => "exact",
            Source::Observed(_) => "observed",
            Source::Counted => "counted",
        }
    }

    #[test]
    fn snapshots_turn_into_the_fixture_answers() {
        for case in fixture()["snapshots"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let before: Option<Baseline> = serde_json::from_value(case["before"].clone()).unwrap();
            let after: CardState = serde_json::from_value(case["after"].clone()).unwrap();
            let change = observe(
                case["cid"].as_i64().unwrap(),
                before.as_ref(),
                &after,
                &clock_of(case),
            );
            let mut got = serde_json::to_value(&change).unwrap();

            // Each answer's XP is its base rate: no combo, since spacing is not known.
            if let Change::Answered { answers } = &change {
                for (index, answer) in answers.iter().enumerate() {
                    got["answers"][index]["xp"] =
                        json!(review_base_xp_with(&answer.review(), ReviewWeighting::Flat));
                }
            }
            assert_eq!(got, case["expected"], "{name}");
        }
    }

    #[test]
    fn one_poll_counts_at_most_the_cap() {
        let state = |reps, secs| CardState {
            reps,
            kind: CardType::Learning,
            interval: 0,
            last_review_time_secs: Some(secs),
        };
        let noon = 1_788_955_200;
        let before = Baseline {
            state: state(10, noon - 600),
            read_at_ms: (noon - 300) * 1000,
        };
        let Change::Answered { answers } = observe(
            7,
            Some(&before),
            &state(10 + MAX_ANSWERS_PER_POLL + 20, noon),
            &ClockHistory::default(),
        ) else {
            panic!("answers expected");
        };
        assert_eq!(answers.len(), MAX_ANSWERS_PER_POLL as usize);
        assert_eq!(answers.last().unwrap().at, noon * 1000);
        assert!(answers.windows(2).all(|pair| pair[0].at < pair[1].at));
    }

    #[test]
    fn merges_and_verdicts_match_the_fixture() {
        for case in fixture()["merge"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let clock = clock_of(case);
            let exact: Vec<Review> = serde_json::from_value(case["exact"].clone()).unwrap();
            let observed = Observed {
                answers: serde_json::from_value(case["observed"].clone()).unwrap(),
                days: case["days"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|day| (day["day"].as_i64().unwrap(), day["cards"].as_u64().unwrap()))
                    .collect(),
            };

            // Which answers count, and where they come from.
            let merged = merge(&exact, &observed, &clock);
            let kept: Vec<Value> = merged
                .iter()
                .map(|answer| {
                    json!({
                        "at": answer.review.id,
                        "cid": (answer.source != Source::Counted).then_some(answer.review.cid),
                        "source": source_name(answer.source),
                    })
                })
                .collect();
            assert_eq!(json!(kept), case["expected"]["answers"], "{name}: answers");

            // Each day's counts, review XP and verdict.
            let (stats, _, _) = collect_days_weighted(
                merged.iter().map(|a| (&a.review, a.source)),
                &clock,
                0,
                ReviewWeighting::Flat,
            );
            let now = case["now_ms"].as_i64().unwrap();
            let rule = StreakRule {
                changes: vec![StreakChange {
                    at: 0,
                    minutes: case["streak_minutes"].as_u64().unwrap() as u32,
                }],
                ..StreakRule::default()
            };
            let profile = compute_with_rules(
                "player",
                "Player",
                &exact,
                &observed,
                &clock,
                &Week::default(),
                now,
                &FreezePolicy::default(),
                &rule,
            );
            for expected in case["expected"]["days"].as_array().unwrap() {
                let day = expected["day"].as_i64().unwrap();
                let date = expected["date"].as_str().unwrap();
                assert_eq!(date_string(day), date, "{name}: date of {day}");
                let s = &stats[&day];
                let history = profile.history.iter().find(|h| h.date == date).unwrap();
                let got = json!({
                    "day": day,
                    "date": date,
                    "reviews": s.reviews,
                    "observed": s.observed,
                    "time_ms": s.time_ms,
                    "state": history.state,
                });
                let mut want = expected.clone();
                let xp = want.as_object_mut().unwrap().remove("review_xp").unwrap();
                assert_eq!(got, want, "{name}: {date}");
                assert!(
                    (s.review_xp - xp.as_f64().unwrap()).abs() < 1e-9,
                    "{name}: {date} review XP {} is not {xp}",
                    s.review_xp
                );
                assert_eq!(history.reviews, s.reviews, "{name}: {date}");
                assert_eq!(history.observed, s.observed, "{name}: {date}");
            }
        }
    }
}
