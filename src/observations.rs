//! Uploads of observed study: what AnkiQuest Lite reads of stock AnkiDroid, which has no review
//! log. The client sends each answered card's state and each day's count of answered cards; the
//! server keeps the last state of every card and works out the answers since (see
//! [`game::observe`]). Observed answers are stored apart from review rows, and exact rows replace
//! them when they arrive (see `game::observed`).

use crate::access::require_owner;
use crate::app::{App, now_ms};
use crate::game::{
    self, Baseline, CardState, Change, Clock, ClockHistory, Observed, ObservedAnswer, Timing,
};
use crate::store::Error;
use crate::study::{MAX_PENDING, Recorded, UploadResponse, answer_upload};
use crate::web_util::failed_for;
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The capability that says this server takes observed study.
pub(crate) const FEATURE: &str = "observed_reviews";

pub(crate) const PATH: &str = "/api/observations/{user}";

/// The most days one upload can count, about a year of backfill.
const MAX_DAYS: usize = 400;

/// The most cards a day's count can name.
const MAX_DAY_CARDS: u64 = 100_000;

/// How far ahead of the server's clock a client's clock may run.
const CLOCK_SKEW_MS: i64 = 5 * 60_000;

/// One card as the client read it from AnkiDroid's `cards` table.
#[derive(Deserialize)]
struct CardUpload {
    cid: i64,
    #[serde(flatten)]
    state: CardState,
}

/// How many distinct cards a day's `prop:rated` search found.
#[derive(Deserialize)]
struct DayCount {
    day: i64,
    cards: u64,
}

#[derive(Deserialize)]
struct Upload {
    clock: Clock,
    /// When the client read the cards, on its own clock.
    read_at_ms: i64,
    #[serde(default)]
    cards: Vec<CardUpload>,
    #[serde(default)]
    days: Vec<DayCount>,
    #[serde(default)]
    silent: bool,
}

/// What the upload's card states meant, card by card.
#[derive(Default, Serialize)]
struct Summary {
    /// Answers counted.
    answered: u64,
    /// Answers taken back because a card's `reps` went down.
    undone: u64,
    /// Cards reset to new.
    reset: u64,
    /// Cards with more answers that their last review time does not date.
    undated: u64,
}

#[derive(Serialize)]
struct ObservationResponse {
    #[serde(flatten)]
    upload: UploadResponse,
    observed: Summary,
}

pub(crate) fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists observed_cards (
             user text not null,
             cid integer not null,
             reps integer not null,
             type integer not null,
             interval integer not null,
             last_review_time_secs integer,
             read_at_ms integer not null,
             primary key (user, cid)
         ) without rowid;
         create table if not exists observed_answers (
             user text not null,
             cid integer not null,
             at integer not null,
             kind integer not null,
             last_ivl integer not null,
             ivl integer,
             timing text not null,
             primary key (user, cid, at)
         ) without rowid;
         create table if not exists observed_days (
             user text not null,
             day integer not null,
             cards integer not null,
             primary key (user, day)
         ) without rowid;",
    )?;
    Ok(())
}

fn timing_name(timing: Timing) -> &'static str {
    match timing {
        Timing::Known => "known",
        Timing::Placed => "placed",
    }
}

fn timing_of(name: &str) -> Timing {
    match name {
        "placed" => Timing::Placed,
        _ => Timing::Known,
    }
}

/// Everything stored of `user`'s observed study.
pub(crate) fn load(conn: &Connection, user: &str) -> Result<Observed, Error> {
    let answers = conn
        .prepare(
            "select cid, at, kind, last_ivl, ivl, timing from observed_answers
             where user = ?1 order by at, cid",
        )?
        .query_map([user], |r| {
            Ok(ObservedAnswer {
                cid: r.get(0)?,
                at: r.get(1)?,
                kind: r.get(2)?,
                last_ivl: r.get(3)?,
                ivl: r.get(4)?,
                timing: timing_of(&r.get::<_, String>(5)?),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let days = conn
        .prepare("select day, cards from observed_days where user = ?1")?
        .query_map([user], |r| {
            Ok((r.get(0)?, r.get::<_, i64>(1)?.max(0) as u64))
        })?
        .collect::<Result<BTreeMap<i64, u64>, _>>()?;
    Ok(Observed { answers, days })
}

fn baseline(conn: &Connection, user: &str, cid: i64) -> Result<Option<Baseline>, Error> {
    let found = conn
        .query_row(
            "select reps, type, interval, last_review_time_secs, read_at_ms from observed_cards
             where user = ?1 and cid = ?2",
            params![user, cid],
            |r| {
                Ok((
                    r.get::<_, u32>(0)?,
                    r.get::<_, u8>(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                ))
            },
        )
        .optional()?;
    let Some((reps, kind, interval, last_review_time_secs, read_at_ms)) = found else {
        return Ok(None);
    };
    Ok(Some(Baseline {
        state: CardState {
            reps,
            kind: kind.try_into()?,
            interval,
            last_review_time_secs,
        },
        read_at_ms,
    }))
}

/// Records one card's new state and what it changed, and says what that was.
fn record_card(
    conn: &Connection,
    user: &str,
    card: &CardUpload,
    read_at_ms: i64,
    clock: &ClockHistory,
) -> Result<Change, Error> {
    let before = baseline(conn, user, card.cid)?;
    let change = game::observe(card.cid, before.as_ref(), &card.state, clock);

    // New answers are added; undone ones go, newest first.
    match &change {
        Change::Answered { answers } => {
            let mut insert = conn.prepare(
                "insert or ignore into observed_answers
                     (user, cid, at, kind, last_ivl, ivl, timing)
                 values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for answer in answers {
                insert.execute(params![
                    user,
                    answer.cid,
                    answer.at,
                    answer.kind,
                    answer.last_ivl,
                    answer.ivl,
                    timing_name(answer.timing)
                ])?;
            }
        }
        Change::Undone { count } => {
            conn.execute(
                "delete from observed_answers where user = ?1 and cid = ?2 and at in
                     (select at from observed_answers where user = ?1 and cid = ?2
                      order by at desc limit ?3)",
                params![user, card.cid, count],
            )?;
        }
        Change::Unchanged | Change::Reset | Change::Undated => {}
    }

    // The new state is what the next one is compared with.
    conn.execute(
        "insert into observed_cards
             (user, cid, reps, type, interval, last_review_time_secs, read_at_ms)
         values (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         on conflict (user, cid) do update set
             reps = excluded.reps, type = excluded.type, interval = excluded.interval,
             last_review_time_secs = excluded.last_review_time_secs,
             read_at_ms = excluded.read_at_ms",
        params![
            user,
            card.cid,
            card.state.reps,
            u8::from(card.state.kind),
            card.state.interval,
            card.state.last_review_time_secs,
            read_at_ms
        ],
    )?;
    Ok(change)
}

/// Records an upload's cards and day counts in one transaction.
fn record(
    conn: &Connection,
    user: &str,
    upload: &Upload,
    clock: &ClockHistory,
) -> Result<Summary, Error> {
    let tx = conn.unchecked_transaction()?;
    let mut summary = Summary::default();
    for card in &upload.cards {
        match record_card(&tx, user, card, upload.read_at_ms, clock)? {
            Change::Answered { answers } => summary.answered += answers.len() as u64,
            Change::Undone { count } => summary.undone += u64::from(count),
            Change::Reset => summary.reset += 1,
            Change::Undated => summary.undated += 1,
            Change::Unchanged => {}
        }
    }
    let mut days = tx.prepare(
        "insert into observed_days (user, day, cards) values (?1, ?2, ?3)
         on conflict (user, day) do update set cards = excluded.cards",
    )?;
    for day in &upload.days {
        days.execute(params![user, day.day, day.cards as i64])?;
    }
    drop(days);
    tx.commit()?;
    Ok(summary)
}

/// Whether an upload is one the server can take: bounded, with a valid clock, read no later
/// than now, cards answered no later than they were read, and no day after tomorrow.
fn valid(upload: &Upload, now: i64) -> Result<(), StatusCode> {
    if upload.cards.len() > MAX_PENDING || upload.days.len() > MAX_DAYS {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let latest = now + CLOCK_SKEW_MS;
    let today = upload.clock.day(now);
    let cards_ok = upload.cards.iter().all(|card| {
        card.cid > 0
            && card
                .state
                .last_review_time_secs
                .is_none_or(|secs| secs.saturating_mul(1000) <= upload.read_at_ms + CLOCK_SKEW_MS)
    });
    let days_ok = upload
        .days
        .iter()
        .all(|day| day.day <= today + 1 && day.cards <= MAX_DAY_CARDS);
    if !crate::decks::valid_clock(upload.clock)
        || upload.read_at_ms <= 0
        || upload.read_at_ms > latest
        || !cards_ok
        || !days_ok
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(())
}

async fn upload(
    State(app): State<Arc<App>>,
    UrlPath(user): UrlPath<String>,
    headers: HeaderMap,
    Json(upload): Json<Upload>,
) -> Result<Json<ObservationResponse>, StatusCode> {
    require_owner(&app, &user, &headers)?;
    valid(&upload, now_ms())?;

    // Recording locks the store and works out the profile, so it runs on a blocking thread.
    crate::app::spawn_blocking(move || observe_upload(&app, &user, &headers, upload))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map(Json)
}

fn observe_upload(
    app: &App,
    user: &str,
    headers: &HeaderMap,
    upload: Upload,
) -> Result<ObservationResponse, StatusCode> {
    let store_error = failed_for("observation upload for", user);
    let known = app
        .store
        .lock()
        .unwrap()
        .is_known(user)
        .map_err(store_error)?;
    let silent = upload.silent || !known;
    let before = if silent { None } else { app.profile(user) };

    // The clock first, so the answers are dated on the day the player keeps.
    let (summary, changed) = {
        let mut store = app.store.lock().unwrap();
        store
            .upsert_stamped(user, &[], &[], &[], upload.clock)
            .map_err(store_error)?;
        let clock = store.clock_history(user).map_err(store_error)?;
        let summary = record(&store.conn, user, &upload, &clock).map_err(store_error)?;
        let changed = app.apply_observed(&store, user).map_err(store_error)?;
        crate::challenges::refresh_member(&mut store, &app.participants(), now_ms(), user)
            .map_err(store_error)?;
        (summary, changed)
    };
    let recorded = Recorded {
        before,
        changed,
        silent,
        announced: Vec::new(),
    };
    Ok(ObservationResponse {
        upload: answer_upload(app, user, headers, recorded)?,
        observed: summary,
    })
}

pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new().route(PATH, post(upload))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::cleanup;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    /// Noon UTC on 9 September 2026, day 20705.
    const NOON_SECS: i64 = 1_788_955_200;

    async fn post_json(app: &Arc<App>, uri: &str, body: Value) -> (StatusCode, Value) {
        let response = crate::router(app.clone())
            .oneshot(
                Request::post(uri)
                    .header("authorization", "Bearer alice-token")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1 << 22).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn observation(cards: Value, days: Value, read_at_ms: i64) -> Value {
        json!({
            "clock": {"offset_west_min": 0, "rollover_hour": 4},
            "read_at_ms": read_at_ms,
            "cards": cards,
            "days": days,
        })
    }

    fn card(cid: i64, reps: u32, kind: u8, interval: i64, secs: i64) -> Value {
        json!({"cid": cid, "reps": reps, "type": kind, "interval": interval, "last_review_time_secs": secs})
    }

    #[tokio::test]
    async fn observed_answers_count_once_and_exact_rows_replace_them() {
        let now = NOON_SECS * 1000 + 3_600_000;
        crate::app::fix_now_ms(Some(now));
        let (app, path) = crate::test_support::app_with(json!({
            "configured_tokens": "enabled",
            "users": {"alice": {"token": "alice-token"}}
        }));
        let uri = "/api/observations/alice";

        // First sight: one answer per card at its last review time.
        let first = observation(
            json!([
                card(7, 5, 2, 12, NOON_SECS),
                card(8, 1, 1, 0, NOON_SECS + 60)
            ]),
            json!([{"day": 20705, "cards": 3}]),
            now,
        );
        let (status, body) = post_json(&app, uri, first.clone()).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["observed"]["answered"], 2);
        assert_eq!(body["today"]["reviews"], 3, "the day's count adds one card");
        assert_eq!(body["today"]["observed"], 3);
        assert_eq!(body["xp_sources"]["day"], "observed");

        // The same upload again changes nothing.
        let (_, again) = post_json(&app, uri, first).await;
        assert_eq!(again["observed"]["answered"], 0);
        assert_eq!(again["today"]["reviews"], 3);

        // An undo takes the answer back.
        let undo = observation(json!([card(8, 0, 1, 0, NOON_SECS + 60)]), json!([]), now);
        let (_, undone) = post_json(&app, uri, undo).await;
        assert_eq!(undone["observed"]["undone"], 1);
        assert_eq!(
            undone["today"]["reviews"], 3,
            "the day's count still covers the card"
        );

        // An exact row for card 7 today replaces its observed answer.
        let exact = json!({
            "reviews": [{"id": NOON_SECS * 1000 - 1500, "cid": 7, "last_ivl": 12, "ivl": 30, "time_ms": 8000, "kind": 1}],
            "clock": {"offset_west_min": 0, "rollover_hour": 4},
        });
        let (status, body) = post_json(&app, "/api/reviews/alice", exact).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["today"]["reviews"], 3);
        assert_eq!(body["today"]["observed"], 2);
        assert_eq!(body["today"]["time_ms"], 8000);
        cleanup(app, path);
    }

    /// A test app with alice signed in and the clock at `now`.
    fn alice_at(now: i64) -> (Arc<App>, std::path::PathBuf) {
        crate::app::fix_now_ms(Some(now));
        crate::test_support::app_with(json!({
            "configured_tokens": "enabled",
            "users": {"alice": {"token": "alice-token"}}
        }))
    }

    #[tokio::test]
    async fn an_older_card_snapshot_does_not_undo_a_newer_answer() {
        let now = NOON_SECS * 1000 + 3_600_000;
        let (app, path) = alice_at(now);
        let uri = "/api/observations/alice";
        let older = observation(
            json!([card(7, 5, 2, 12, NOON_SECS)]),
            json!([]),
            now - 60_000,
        );
        let newer = observation(json!([card(7, 6, 2, 20, NOON_SECS + 30)]), json!([]), now);
        post_json(&app, uri, older.clone()).await;
        post_json(&app, uri, newer).await;

        // The retried older read must not look like an undo of the sixth answer.
        let (status, body) = post_json(&app, uri, older).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["observed"]["undone"], 0);
        let observed = load(&app.store.lock().unwrap().conn, "alice").unwrap();
        assert_eq!(observed.answers.len(), 2);
        cleanup(app, path);
    }

    #[tokio::test]
    async fn an_older_snapshot_does_not_restore_an_undone_answer() {
        let now = NOON_SECS * 1000 + 3_600_000;
        let (app, path) = alice_at(now);
        let uri = "/api/observations/alice";
        // Undo in AnkiDroid also puts back the previous review time.
        let answered = observation(
            json!([card(8, 2, 1, 0, NOON_SECS + 60)]),
            json!([]),
            now - 60_000,
        );
        let undone = observation(json!([card(8, 1, 1, 0, NOON_SECS - 3600)]), json!([]), now);
        post_json(&app, uri, answered.clone()).await;
        post_json(&app, uri, undone).await;

        // Retrying the read from before the undo leaves the answer undone.
        let (status, body) = post_json(&app, uri, answered).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["observed"]["answered"], 0);
        assert!(
            load(&app.store.lock().unwrap().conn, "alice")
                .unwrap()
                .answers
                .is_empty()
        );
        cleanup(app, path);
    }

    #[tokio::test]
    async fn an_older_day_count_does_not_replace_a_newer_one() {
        let now = NOON_SECS * 1000 + 3_600_000;
        let (app, path) = alice_at(now);
        let uri = "/api/observations/alice";
        let older = observation(json!([]), json!([{"day": 20705, "cards": 1}]), now - 60_000);
        let newer = observation(json!([]), json!([{"day": 20705, "cards": 2}]), now);
        post_json(&app, uri, older.clone()).await;
        post_json(&app, uri, newer).await;
        post_json(&app, uri, older).await;

        let observed = load(&app.store.lock().unwrap().conn, "alice").unwrap();
        assert_eq!(observed.days.get(&20705), Some(&2));
        cleanup(app, path);
    }

    #[tokio::test]
    async fn uploads_from_the_future_or_unbounded_are_refused() {
        let now = NOON_SECS * 1000;
        crate::app::fix_now_ms(Some(now));
        let (app, path) = crate::test_support::app_with(json!({
            "configured_tokens": "enabled",
            "users": {"alice": {"token": "alice-token"}}
        }));
        let uri = "/api/observations/alice";
        let later = now + CLOCK_SKEW_MS + 1;
        for (body, status) in [
            (
                observation(json!([]), json!([]), later),
                StatusCode::BAD_REQUEST,
            ),
            (
                observation(json!([card(7, 1, 1, 0, NOON_SECS + 3600)]), json!([]), now),
                StatusCode::BAD_REQUEST,
            ),
            (
                observation(json!([]), json!([{"day": 20707, "cards": 1}]), now),
                StatusCode::BAD_REQUEST,
            ),
            (
                observation(json!([card(7, 1, 9, 0, NOON_SECS)]), json!([]), now),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                observation(
                    json!([]),
                    Value::Array(
                        (0..=MAX_DAYS as i64)
                            .map(|d| json!({"day": d, "cards": 1}))
                            .collect(),
                    ),
                    now,
                ),
                StatusCode::PAYLOAD_TOO_LARGE,
            ),
        ] {
            assert_eq!(post_json(&app, uri, body.clone()).await.0, status, "{body}");
        }
        assert!(
            load(&app.store.lock().unwrap().conn, "alice")
                .unwrap()
                .is_empty()
        );
        cleanup(app, path);
    }
}
