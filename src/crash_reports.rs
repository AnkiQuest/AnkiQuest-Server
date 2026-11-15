//! Crash reports from the Android app, sent by ACRA's HTTP sender as JSON.
//!
//! Anyone can send one, so each client address gets a few per hour and a report is at most
//! [`MAX_BYTES`]. Fields that identify a player, a device or an install are removed before
//! a report is stored, and reports are deleted after [`RETENTION_DAYS`]. Devs read them in
//! the dev console (`/dev`).

use crate::app::{App, now_ms};
use crate::game::{DAY_MS, HOUR_MS};
use crate::store::Error;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};

pub(crate) const PATH: &str = "/api/crash-reports";
/// The largest report accepted; a longer body is `413`.
const MAX_BYTES: usize = 256 * 1024;
/// Reports one client address (an IPv6 /64) may send per hour; more are `429`.
const REPORTS_PER_HOUR: usize = 10;
/// Client addresses remembered at once, so the limiter's memory stays bounded.
const MAX_CLIENTS: usize = 10_000;
/// Days a report is kept.
const RETENTION_DAYS: i64 = 30;
/// Reports kept at most; the oldest go first, so the table stays bounded.
const MAX_STORED: i64 = 5_000;
/// Reports the dev console lists.
const LISTED: i64 = 100;
/// What replaces a value that looks like a credential.
const REMOVED: &str = "[removed]";
/// The shortest run of token characters treated as a credential.
const MIN_TOKEN_LEN: usize = 32;
/// The host part of an IPv6 address; the limit covers the /64 in front of it.
const IPV6_HOST_BITS: u32 = 64;
/// The word in front of a credential in an `Authorization` header.
const BEARER: &str = "bearer";

/// Fields removed wherever they appear, compared without case: ACRA's account and install
/// identifiers, Android's device id, and the app's AnkiQuest sign-in.
const DROPPED_KEYS: &[&str] = &[
    "USER_EMAIL",
    "DEVICE_ID",
    "INSTALLATION_ID",
    "android_id",
    "ankiquestToken",
    "ankiquestUser",
    "ankiquestUrl",
];
/// Preference keys removed from `SHARED_PREFERENCES`: AnkiWeb's sign-in and sync address and
/// the browser search history, which AnkiDroid already excludes, checked again here.
const DROPPED_PREFERENCES: &[&str] = &[
    "username",
    "hkey",
    "currentSyncUri",
    "browser_search_history",
];
/// Preference keys starting with this are AnkiQuest's own and removed.
const QUEST_PREFIX: &str = "ankiquest";
/// ACRA's random id for the report, kept so duplicates can be told apart.
const REPORT_ID: &str = "REPORT_ID";
const SHARED_PREFERENCES: &str = "SHARED_PREFERENCES";

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists crash_reports (
             id integer primary key,
             received_at integer not null,
             app_version_code integer,
             android_version text,
             report text not null
         );
         create index if not exists crash_reports_received_at on crash_reports (received_at);",
    )?;
    Ok(())
}

/// How many reports each client address sent in the last hour, kept only in memory.
#[derive(Default)]
pub(crate) struct Limiter(Mutex<HashMap<IpAddr, Vec<i64>>>);

impl Limiter {
    /// Counts a report from `client` at `now`; false when it is over the hourly limit.
    fn admit(&self, client: IpAddr, now: i64) -> bool {
        let client = client_key(client);
        let recent = |at: &i64| now - at < HOUR_MS;
        let mut clients = self.0.lock().unwrap();

        // A new client in a full map: forget clients quiet for an hour, else the quietest one.
        if !clients.contains_key(&client) && clients.len() >= MAX_CLIENTS {
            clients.retain(|_, times| times.iter().any(recent));
            let quietest = clients
                .iter()
                .min_by_key(|(_, times)| times.iter().max().copied())
                .map(|(client, _)| *client);
            if clients.len() >= MAX_CLIENTS
                && let Some(quietest) = quietest
            {
                clients.remove(&quietest);
            }
        }

        // Count this report unless the client already sent its hourly share.
        let times = clients.entry(client).or_default();
        times.retain(recent);
        if times.len() >= REPORTS_PER_HOUR {
            return false;
        }
        times.push(now);
        true
    }
}

/// The address a limit applies to: an IPv6 client's /64, since one device rotates within it.
fn client_key(client: IpAddr) -> IpAddr {
    match client {
        IpAddr::V4(_) => client,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(Ipv6Addr::from(v6.to_bits() & (u128::MAX << IPV6_HOST_BITS))),
        },
    }
}

/// Removes everything in `report` that identifies a player, a device or an install.
fn strip(report: &mut Value) {
    // ACRA's report id is a random UUID, not a credential; keep it as sent.
    let id = report.get_mut(REPORT_ID).map(Value::take);
    if let Some(preferences) = report.get_mut(SHARED_PREFERENCES) {
        drop_preferences(preferences);
    }
    scrub(report);
    if let (Some(id), Value::Object(fields)) = (id, report) {
        fields.insert(REPORT_ID.into(), id);
    }
}

/// Removes [`DROPPED_KEYS`] at any depth and replaces credentials in every string.
fn scrub(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            fields.retain(|key, _| !DROPPED_KEYS.iter().any(|d| d.eq_ignore_ascii_case(key)));
            fields.values_mut().for_each(scrub);
        }
        Value::Array(items) => items.iter_mut().for_each(scrub),
        Value::String(text) => {
            if let Some(clean) = without_tokens(text) {
                *text = clean;
            }
        }
        _ => {}
    }
}

/// Removes AnkiQuest's and AnkiWeb's preferences from every preferences file in `value`.
fn drop_preferences(value: &mut Value) {
    let Value::Object(fields) = value else {
        return;
    };
    fields.retain(|key, _| !private_preference(key));
    fields.values_mut().for_each(drop_preferences);
}

fn private_preference(key: &str) -> bool {
    let quest = key
        .get(..QUEST_PREFIX.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(QUEST_PREFIX));
    quest || DROPPED_PREFERENCES.contains(&key)
}

/// Characters of hex, base64url and similar credentials.
fn token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

/// A run of token characters that is a credential: long and mixing letters and digits.
fn looks_like_token(word: &str) -> bool {
    word.len() >= MIN_TOKEN_LEN
        && word.chars().any(|c| c.is_ascii_digit())
        && word.chars().any(|c| c.is_ascii_alphabetic())
}

/// `text` with each credential replaced by [`REMOVED`]: long token-like runs, and whatever
/// follows "Bearer". `None` when there is none.
fn without_tokens(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    let mut after_bearer = false;
    let mut rest = text;
    while !rest.is_empty() {
        // Copy what separates words; a credential follows "Bearer" after spaces only.
        let gap = rest.find(token_char).unwrap_or(rest.len());
        let (separator, tail) = rest.split_at(gap);
        out.push_str(separator);
        after_bearer &= !separator.is_empty() && separator.chars().all(|c| c == ' ');
        let end = tail.find(|c| !token_char(c)).unwrap_or(tail.len());
        let (word, tail) = tail.split_at(end);
        rest = tail;
        if word.is_empty() {
            continue;
        }

        // Replace the word if it is a credential.
        if after_bearer || looks_like_token(word) {
            out.push_str(REMOVED);
            changed = true;
        } else {
            out.push_str(word);
        }
        after_bearer = word.eq_ignore_ascii_case(BEARER);
    }
    changed.then_some(out)
}

/// Saves a stripped report received at `now`, then deletes what is past the retention
/// or over the stored maximum.
fn insert(conn: &Connection, report: &Value, now: i64) -> Result<(), Error> {
    // APP_VERSION_CODE is a number, but older senders wrote it as a string.
    let code = match &report["APP_VERSION_CODE"] {
        Value::Number(code) => code.as_i64(),
        Value::String(code) => code.trim().parse().ok(),
        _ => None,
    };
    let android = report["ANDROID_VERSION"].as_str();
    conn.execute(
        "insert into crash_reports (received_at, app_version_code, android_version, report)
         values (?1, ?2, ?3, ?4)",
        params![now, code, android, report.to_string()],
    )?;

    // Keep the table within the retention and the stored maximum.
    prune(conn, now)?;
    conn.execute(
        "delete from crash_reports where id <= (
             select id from crash_reports order by id desc limit 1 offset ?1
         )",
        [MAX_STORED],
    )?;
    Ok(())
}

/// Deletes reports older than [`RETENTION_DAYS`]; the worker calls it on every tick.
pub(crate) fn prune(conn: &Connection, now: i64) -> Result<(), Error> {
    conn.execute(
        "delete from crash_reports where received_at <= ?1",
        [now - RETENTION_DAYS * DAY_MS],
    )?;
    Ok(())
}

/// A report as the dev console lists it.
#[derive(Debug, Serialize)]
pub(crate) struct Summary {
    id: i64,
    received_at: i64,
    app_version_code: Option<i64>,
    app_version_name: Option<String>,
    android_version: Option<String>,
    /// The first line of the stack trace: the exception and its message.
    exception: Option<String>,
}

/// The latest reports, newest first.
pub(crate) fn recent(conn: &Connection) -> Result<Vec<Summary>, Error> {
    let mut stmt = conn.prepare(
        "select id, received_at, app_version_code, android_version,
                json_extract(report, '$.APP_VERSION_NAME'),
                json_extract(report, '$.STACK_TRACE')
         from crash_reports order by id desc limit ?1",
    )?;
    let rows = stmt.query_map([LISTED], |r| {
        let trace: Option<String> = r.get(5).ok().flatten();
        Ok(Summary {
            id: r.get(0)?,
            received_at: r.get(1)?,
            app_version_code: r.get(2)?,
            android_version: r.get(3)?,
            app_version_name: r.get(4).ok().flatten(),
            exception: trace.and_then(|trace| trace.lines().next().map(str::to_string)),
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// One stored report, as it was saved after stripping.
pub(crate) fn report(conn: &Connection, id: i64) -> Result<Option<Value>, Error> {
    let text: Option<String> = conn
        .query_row(
            "select report from crash_reports where id = ?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(text.map(|text| serde_json::from_str(&text)).transpose()?)
}

async fn receive(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    // The body limit has already answered 413 for a longer report.
    let client = crate::access::client_ip(&app.config, &headers, peer);
    let now = now_ms();
    if !app.crash_reports.admit(client, now) {
        return StatusCode::TOO_MANY_REQUESTS;
    }
    let Ok(mut report @ Value::Object(_)) = serde_json::from_slice::<Value>(&body) else {
        return StatusCode::BAD_REQUEST;
    };

    strip(&mut report);
    match insert(&app.store.lock().unwrap().conn, &report, now) {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(error) => {
            log_error!("crash report not saved: {error}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new().route(PATH, post(receive).layer(DefaultBodyLimit::max(MAX_BYTES)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::json;

    /// Wednesday 28 October 2026, 10:30 UTC.
    const NOW_MS: i64 = 1_793_183_400_000;
    const TOKEN: &str = "0f3c9a7b1e2d4c6f8a0b2c4d6e8f0a1b3c5d7e9f1a2b3c4d5e6f7a8b9c0d1e2f";

    fn post_from(peer: &str, body: impl Into<Body>) -> (Request<Body>, String) {
        let request = Request::builder()
            .method("POST")
            .uri(PATH)
            .header("content-type", "application/json")
            .body(body.into())
            .unwrap();
        (request, peer.to_string())
    }

    async fn send(app: &Arc<App>, peer: &str, body: impl Into<Body>) -> StatusCode {
        let (request, peer) = post_from(peer, body);
        route_from(app, request, &peer).await.status()
    }

    fn report() -> Value {
        json!({
            "REPORT_ID": "5c1d4b1e-8a5f-4b8e-9b0e-2f8b5d0a6c11",
            "APP_VERSION_CODE": 22_100_300,
            "APP_VERSION_NAME": "2.21.0-quest",
            "ANDROID_VERSION": "14",
            "STACK_TRACE": "java.lang.IllegalStateException: boom\n\tat com.ichi2.anki.DeckPicker.onCreate(DeckPicker.kt:120)",
            "USER_EMAIL": "player@example.com",
            "INSTALLATION_ID": "a2b4c6d8-0000-4000-8000-123456789abc",
            "DEVICE_ID": "35-209900-176148-1",
            "CUSTOM_DATA": {"ankiquestUser": "cerro", "WEBVIEW_VER_NAME": "120.0"},
            "LOGCAT": format!("I/Sync: Authorization: Bearer hill-secret\nD/Quest: sent {TOKEN}"),
            "SHARED_PREFERENCES": {
                "default": {
                    "ankiquestToken": TOKEN,
                    "ankiquest_url": "https://anki.example.test",
                    "AnkiquestStreakCache": "7",
                    "username": "player@example.com",
                    "hkey": "abc",
                    "dayTheme": "1",
                    "lastSyncToken": TOKEN
                }
            },
            "BUILD": {"MODEL": "Pixel 8", "nested": {"ankiquestUrl": "https://anki.example.test"}}
        })
    }

    fn stored(app: &App) -> Vec<(i64, Option<i64>, Option<String>, Value)> {
        let store = app.store.lock().unwrap();
        let mut stmt = store
            .conn
            .prepare(
                "select received_at, app_version_code, android_version, report
                 from crash_reports order by id",
            )
            .unwrap();
        stmt.query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                serde_json::from_str(&r.get::<_, String>(3)?).unwrap(),
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
    }

    #[tokio::test]
    async fn a_report_is_stored_without_account_or_device_identifiers() {
        fix_now(NOW_MS);
        let (app, path) = fixture();
        assert_eq!(
            send(&app, "192.0.2.7:4000", report().to_string()).await,
            StatusCode::NO_CONTENT
        );

        // The row keeps when it came, the app and Android versions, and the stripped report.
        let rows = stored(&app);
        assert_eq!(rows.len(), 1);
        let (received_at, code, android, saved) = &rows[0];
        assert_eq!(*received_at, NOW_MS);
        assert_eq!(*code, Some(22_100_300));
        assert_eq!(android.as_deref(), Some("14"));
        assert_eq!(saved["REPORT_ID"], "5c1d4b1e-8a5f-4b8e-9b0e-2f8b5d0a6c11");
        assert_eq!(saved["STACK_TRACE"], report()["STACK_TRACE"]);
        for key in ["USER_EMAIL", "INSTALLATION_ID", "DEVICE_ID"] {
            assert!(saved.get(key).is_none(), "{key} is removed");
        }
        assert_eq!(saved["CUSTOM_DATA"], json!({"WEBVIEW_VER_NAME": "120.0"}));
        assert_eq!(saved["BUILD"], json!({"MODEL": "Pixel 8", "nested": {}}));
        assert_eq!(
            saved["SHARED_PREFERENCES"],
            json!({"default": {"dayTheme": "1", "lastSyncToken": REMOVED}})
        );
        assert_eq!(
            saved["LOGCAT"],
            format!("I/Sync: Authorization: Bearer {REMOVED}\nD/Quest: sent {REMOVED}")
        );
        let text = saved.to_string();
        for secret in [TOKEN, "hill-secret", "player@example.com", "cerro"] {
            assert!(!text.contains(secret), "{secret} is not stored");
        }
        cleanup(app, path);
    }

    #[test]
    fn tokens_are_found_inside_text_but_names_and_ids_stay() {
        let mut value = json!({
            "REPORT_ID": "5c1d4b1e-8a5f-4b8e-9b0e-2f8b5d0a6c11",
            "trace": "at com.ichi2.anki.AnkiDroidCrashReportDialogFragment.onCreate(Dialog.kt:42)",
            "hash": "7f3a2b1c",
            "words": ["bearer", "BEARER  x", "token=0123456789abcdef0123456789abcdef"],
            "count": 12
        });
        strip(&mut value);
        assert_eq!(
            value,
            json!({
                "REPORT_ID": "5c1d4b1e-8a5f-4b8e-9b0e-2f8b5d0a6c11",
                "trace": "at com.ichi2.anki.AnkiDroidCrashReportDialogFragment.onCreate(Dialog.kt:42)",
                "hash": "7f3a2b1c",
                "words": ["bearer", format!("BEARER  {REMOVED}"), format!("token={REMOVED}")],
                "count": 12
            })
        );
    }

    /// `docs/api/examples/crash_report.json` is a report as ACRA sends it, and
    /// `crash_report_stored.json` what the server keeps of it. Regenerate the second with
    /// `UPDATE_API_EXAMPLES=1`.
    #[tokio::test]
    async fn the_example_report_is_accepted_and_stored_as_documented() {
        fix_now(NOW_MS);
        let (app, path) = fixture();
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/api/examples");
        let sent = std::fs::read_to_string(dir.join("crash_report.json")).unwrap();
        assert_eq!(
            send(&app, "192.0.2.12:4000", sent).await,
            StatusCode::NO_CONTENT
        );

        let saved = stored(&app).remove(0).3;
        let text = serde_json::to_string_pretty(&saved).unwrap() + "\n";
        let file = dir.join("crash_report_stored.json");
        if std::env::var_os("UPDATE_API_EXAMPLES").is_some_and(|v| v == "1") {
            std::fs::write(&file, &text).unwrap();
        }
        let expected = std::fs::read_to_string(&file)
            .unwrap()
            .replace("\r\n", "\n");
        assert_eq!(text, expected, "regenerate with UPDATE_API_EXAMPLES=1");
        cleanup(app, path);
    }

    #[tokio::test]
    async fn bodies_over_the_limit_and_bodies_that_are_not_json_are_refused() {
        fix_now(NOW_MS);
        let (app, path) = fixture();

        // A report of exactly the maximum is taken; one byte more is 413.
        let mut padded = report();
        padded["LOGCAT"] = Value::from("");
        let room = MAX_BYTES - padded.to_string().len();
        padded["LOGCAT"] = Value::from("x".repeat(room));
        assert_eq!(padded.to_string().len(), MAX_BYTES);
        assert_eq!(
            send(&app, "192.0.2.8:4000", padded.to_string()).await,
            StatusCode::NO_CONTENT
        );
        padded["LOGCAT"] = Value::from("x".repeat(room + 1));
        assert_eq!(
            send(&app, "192.0.2.8:4000", padded.to_string()).await,
            StatusCode::PAYLOAD_TOO_LARGE
        );

        // Anything but a JSON object is 400.
        for body in ["not json", "[1, 2]", "\"text\"", ""] {
            assert_eq!(
                send(&app, "192.0.2.9:4000", body).await,
                StatusCode::BAD_REQUEST,
                "{body:?}"
            );
        }
        assert_eq!(stored(&app).len(), 1);
        cleanup(app, path);
    }

    #[tokio::test]
    async fn each_client_sends_ten_reports_an_hour() {
        fix_now(NOW_MS);
        let (app, path) = fixture();
        let body = || report().to_string();
        for _ in 0..REPORTS_PER_HOUR {
            assert_eq!(
                send(&app, "192.0.2.10:4000", body()).await,
                StatusCode::NO_CONTENT
            );
        }
        assert_eq!(
            send(&app, "192.0.2.10:5000", body()).await,
            StatusCode::TOO_MANY_REQUESTS
        );

        // Another address has its own allowance; addresses in one IPv6 /64 share one.
        assert_eq!(
            send(&app, "192.0.2.11:4000", body()).await,
            StatusCode::NO_CONTENT
        );
        for _ in 0..REPORTS_PER_HOUR {
            assert_eq!(
                send(&app, "[2001:db8::1]:4000", body()).await,
                StatusCode::NO_CONTENT
            );
        }
        assert_eq!(
            send(&app, "[2001:db8::2]:4000", body()).await,
            StatusCode::TOO_MANY_REQUESTS
        );

        // An hour later the first address may send again.
        fix_now(NOW_MS + HOUR_MS);
        assert_eq!(
            send(&app, "192.0.2.10:4000", body()).await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(stored(&app).len(), 2 * REPORTS_PER_HOUR + 2);
        cleanup(app, path);
    }

    #[test]
    fn the_limiter_forgets_the_quietest_client_when_full() {
        let limiter = Limiter::default();
        for index in 0..MAX_CLIENTS as u32 {
            assert!(limiter.admit(IpAddr::from(index.to_be_bytes()), NOW_MS + i64::from(index)));
        }
        assert!(limiter.admit("198.51.100.1".parse().unwrap(), NOW_MS + 1_000_000));
        let clients = limiter.0.lock().unwrap();
        assert_eq!(clients.len(), MAX_CLIENTS);
        assert!(!clients.contains_key(&IpAddr::from(0u32.to_be_bytes())));
    }

    #[test]
    fn reports_are_kept_thirty_days_and_at_most_the_stored_maximum() {
        let conn = Connection::open_in_memory().unwrap();
        initialize(&conn).unwrap();
        let count = || -> i64 {
            conn.query_row("select count(*) from crash_reports", [], |r| r.get(0))
                .unwrap()
        };

        // The worker deletes reports once they are older than the retention.
        let old = NOW_MS - RETENTION_DAYS * DAY_MS;
        insert(&conn, &report(), old).unwrap();
        insert(&conn, &report(), old + 1).unwrap();
        prune(&conn, NOW_MS).unwrap();
        assert_eq!(
            count(),
            1,
            "exactly 30 days old is deleted, younger is kept"
        );
        prune(&conn, NOW_MS + 1).unwrap();
        assert_eq!(count(), 0);

        // An insert also deletes what is past the retention.
        insert(&conn, &report(), old).unwrap();
        insert(&conn, &report(), NOW_MS).unwrap();
        assert_eq!(count(), 1);

        // Past the maximum, the oldest reports go first.
        let small = json!({"STACK_TRACE": "x"});
        for index in 0..MAX_STORED {
            insert(&conn, &small, NOW_MS + index).unwrap();
        }
        assert_eq!(count(), MAX_STORED);
        let oldest: i64 = conn
            .query_row("select min(received_at) from crash_reports", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(oldest, NOW_MS);
    }

    #[tokio::test]
    async fn the_tick_deletes_old_reports() {
        fix_now(NOW_MS);
        let (app, path) = fixture();
        insert(
            &app.store.lock().unwrap().conn,
            &report(),
            NOW_MS - RETENTION_DAYS * DAY_MS,
        )
        .unwrap();
        crate::worker::tick(&app).unwrap();
        assert!(stored(&app).is_empty());
        cleanup(app, path);
    }

    fn fix_now(ms: i64) {
        crate::app::fix_now_ms(Some(ms));
    }
}
