//! Crash reports from the Android app, sent by ACRA's HTTP sender as JSON.
//!
//! Anyone can send one, so each client address gets a few per hour and a report is at most
//! [`MAX_BYTES`]. Fields that identify a player, a device or an install, email addresses and
//! credentials are removed before a report is stored, and reports are deleted after
//! [`RETENTION_DAYS`]. Reports of one failure share a signature ([`crate::crash_signature`]):
//! the first [`MAX_SAMPLES`] are stored, later ones only counted. At most [`MAX_SIGNATURES`]
//! signatures are kept and [`STORED_PER_HOUR`] reports stored an hour, so the tables stay
//! bounded however many addresses send. Devs read them in the dev console (`/dev`).

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
use std::ops::Range;
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
/// Signatures remembered at most. A new one takes the place of the least recently seen
/// signature without stored reports; when every one has reports, the new one is dropped.
const MAX_SIGNATURES: i64 = 2_000;
/// Reports stored per hour across all clients; over it, known signatures are only counted
/// and new ones dropped, so a flood from many addresses cannot fill the tables.
const STORED_PER_HOUR: i64 = 500;
/// The longest exception line kept per signature, in bytes.
const MAX_EXCEPTION_BYTES: usize = 1024;
/// Signatures the dev console lists.
const LISTED: i64 = 100;
/// App versions remembered per signature; the least recently seen is forgotten first.
const MAX_VERSIONS: usize = 10;
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
const STACK_TRACE: &str = "STACK_TRACE";
const SHARED_PREFERENCES: &str = "SHARED_PREFERENCES";
/// Where ACRA puts the app's custom fields.
const CUSTOM_DATA: &str = "CUSTOM_DATA";
/// The custom field the app puts its signature of the crash in.
const SIGNATURE_FIELD: &str = "ANKIQUEST_SIGNATURE";
/// Reports stored per signature; later ones are only counted.
const MAX_SAMPLES: i64 = 2;

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists crash_reports (
             id integer primary key,
             received_at integer not null,
             signature text not null,
             app_version_code integer,
             android_version text,
             report text not null
         );
         create index if not exists crash_reports_received_at on crash_reports (received_at);
         create index if not exists crash_reports_signature on crash_reports (signature);
         create table if not exists crash_signatures (
             signature text primary key,
             first_seen integer not null,
             last_seen integer not null,
             count integer not null,
             exception text,
             app_versions text not null
         ) without rowid;
         create index if not exists crash_signatures_last_seen on crash_signatures (last_seen);",
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

/// Where a report may carry the app's signature: in ACRA's custom data, or at the top.
#[derive(Clone, Copy)]
enum Slot {
    CustomData,
    TopLevel,
}

impl Slot {
    const ALL: [Slot; 2] = [Slot::CustomData, Slot::TopLevel];

    fn get(self, report: &mut Value) -> Option<&mut Value> {
        match self {
            Slot::CustomData => report.get_mut(CUSTOM_DATA)?.get_mut(SIGNATURE_FIELD),
            Slot::TopLevel => report.get_mut(SIGNATURE_FIELD),
        }
    }
}

/// The well-formed signatures in `report` and where they were, normalized.
fn sent_signatures(report: &mut Value) -> Vec<(Slot, String)> {
    Slot::ALL
        .into_iter()
        .filter_map(|slot| {
            let sent = slot.get(report)?.as_str()?;
            Some((slot, crate::crash_signature::parse_sent(sent)?))
        })
        .collect()
}

/// The report's signature, computed from `STACK_TRACE` (an empty one when there is none).
/// The app's own `ANKIQUEST_SIGNATURE` is the same when it matches the trace; it is kept in
/// the stored report but not used, so a sender cannot file reports under another crash.
fn signature(report: &Value) -> String {
    let trace = report
        .get(STACK_TRACE)
        .and_then(Value::as_str)
        .unwrap_or("");
    crate::crash_signature::of_trace(trace)
}

/// Removes everything in `report` that identifies a player, a device or an install.
fn strip(report: &mut Value) {
    // ACRA's report id is a random UUID and the signature a hash, not credentials; keep them.
    let id = report.get_mut(REPORT_ID).map(Value::take);
    let signatures = sent_signatures(report);
    if let Some(preferences) = report.get_mut(SHARED_PREFERENCES) {
        drop_preferences(preferences);
    }
    scrub(report);
    for (slot, signature) in signatures {
        if let Some(value) = slot.get(report) {
            *value = signature.into();
        }
    }
    if let (Some(id), Value::Object(fields)) = (id, report) {
        fields.insert(REPORT_ID.into(), id);
    }
}

/// Removes [`DROPPED_KEYS`] at any depth and replaces email addresses and credentials in
/// every string.
fn scrub(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            fields.retain(|key, _| !DROPPED_KEYS.iter().any(|d| d.eq_ignore_ascii_case(key)));
            fields.values_mut().for_each(scrub);
        }
        Value::Array(items) => items.iter_mut().for_each(scrub),
        Value::String(text) => {
            if let Some(clean) = without_emails(text) {
                *text = clean;
            }
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

/// Characters of an email address's local part, before the `@`.
fn local_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b"._%+-".contains(&c)
}

/// Characters of an email address's domain, after the `@`.
fn domain_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'.' || c == b'-'
}

/// `text` with each email address replaced by [`REMOVED`]. AnkiWeb usernames are email
/// addresses, so this also removes them from log lines and comments. `None` when there is none.
fn without_emails(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    for (at, _) in text.match_indices('@') {
        if at < copied {
            continue;
        }
        let Some(address) = email_at(text.as_bytes(), at) else {
            continue;
        };
        out.push_str(&text[copied..address.start]);
        out.push_str(REMOVED);
        copied = address.end;
    }
    if copied == 0 {
        return None;
    }
    out.push_str(&text[copied..]);
    Some(out)
}

/// The bytes of the email address around the `@` at `at`, if it is one: a local part, and a
/// domain of at least two labels ending in letters. Only ASCII is part of it, so the range
/// falls on character boundaries.
fn email_at(text: &[u8], at: usize) -> Option<Range<usize>> {
    // The local part runs back to a character that cannot be in it; dots in front are not part.
    let start = text[..at]
        .iter()
        .rposition(|&c| !local_char(c))
        .map_or(0, |before| before + 1);
    let start = start + text[start..at].iter().take_while(|&&c| c == b'.').count();
    if start == at {
        return None;
    }

    // The domain runs to a character that cannot be in it; dots and dashes after are not part.
    let after = &text[at + 1..];
    let len = after
        .iter()
        .position(|&c| !domain_char(c))
        .unwrap_or(after.len());
    let domain = std::str::from_utf8(&after[..len])
        .ok()?
        .trim_end_matches(['.', '-']);
    let labels: Vec<&str> = domain.split('.').collect();
    let top = labels.last()?;
    let named = top.len() >= 2 && top.bytes().all(|c| c.is_ascii_alphabetic());
    if labels.len() < 2 || labels.iter().any(|label| label.is_empty()) || !named {
        return None;
    }
    Some(start..at + 1 + domain.len())
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

/// What [`insert`] did with a report.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// Counted and stored as a sample of its signature.
    Stored,
    /// Counted under a known signature, not stored.
    Counted,
    /// Neither counted nor stored: a new signature with no room for it.
    Dropped,
}

/// Deletes what is past the retention, then counts a stripped report with `signature`
/// received at `now` and stores it while the signature has fewer than [`MAX_SAMPLES`] and the
/// last hour fewer than [`STORED_PER_HOUR`]. A new signature is dropped when the hour or the
/// signatures table has no room. One transaction, so the count and the samples agree.
fn insert(conn: &Connection, report: &Value, signature: &str, now: i64) -> Result<Outcome, Error> {
    let tx = conn.unchecked_transaction()?;
    let outcome = file(&tx, report, signature, now)?;
    tx.commit()?;
    Ok(outcome)
}

/// [`insert`] inside its transaction.
fn file(conn: &Connection, report: &Value, signature: &str, now: i64) -> Result<Outcome, Error> {
    // APP_VERSION_CODE is a number, but older senders wrote it as a string.
    let code = match &report["APP_VERSION_CODE"] {
        Value::Number(code) => code.as_i64(),
        Value::String(code) => code.trim().parse().ok(),
        _ => None,
    };
    let version = report["APP_VERSION_NAME"]
        .as_str()
        .map(str::to_string)
        .or(code.map(|code| code.to_string()));

    // Delete what is past the retention first, so it does not take up room below.
    prune(conn, now)?;

    // A new signature needs room in the hour and in the table; a known one is always counted.
    let known: bool = conn.query_row(
        "select exists (select 1 from crash_signatures where signature = ?1)",
        [signature],
        |r| r.get(0),
    )?;
    let stored_lately: i64 = conn.query_row(
        "select count(*) from crash_reports where received_at > ?1",
        [now - HOUR_MS],
        |r| r.get(0),
    )?;
    let hour_has_room = stored_lately < STORED_PER_HOUR;
    if !known && !(hour_has_room && make_room(conn)?) {
        return Ok(Outcome::Dropped);
    }
    count(conn, signature, exception(report).as_deref(), version, now)?;

    // Store it only while the hour and the signature have room for another sample.
    let samples: i64 = conn.query_row(
        "select count(*) from crash_reports where signature = ?1",
        [signature],
        |r| r.get(0),
    )?;
    if !hour_has_room || samples >= MAX_SAMPLES {
        return Ok(Outcome::Counted);
    }
    let android = report["ANDROID_VERSION"].as_str();
    conn.execute(
        "insert into crash_reports
             (received_at, signature, app_version_code, android_version, report)
         values (?1, ?2, ?3, ?4, ?5)",
        params![now, signature, code, android, report.to_string()],
    )?;
    Ok(Outcome::Stored)
}

/// Whether the signatures table has room for a new one. When it is full, the least recently
/// seen signature without stored reports makes room; one with reports is never deleted for a
/// new one, so a flood of new signatures cannot push out the samples already kept.
fn make_room(conn: &Connection) -> Result<bool, Error> {
    let rows: i64 = conn.query_row("select count(*) from crash_signatures", [], |r| r.get(0))?;
    if rows < MAX_SIGNATURES {
        return Ok(true);
    }
    let deleted = conn.execute(
        "delete from crash_signatures where signature = (
             select signature from crash_signatures
             where not exists (
                 select 1 from crash_reports
                 where crash_reports.signature = crash_signatures.signature
             )
             order by last_seen, signature limit 1
         )",
        [],
    )?;
    Ok(deleted > 0)
}

/// The first line of the report's stack trace, the exception and its message, cut to at most
/// [`MAX_EXCEPTION_BYTES`].
fn exception(report: &Value) -> Option<String> {
    let line = report.get(STACK_TRACE)?.as_str()?.lines().next()?;
    let mut end = line.len().min(MAX_EXCEPTION_BYTES);
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    Some(line[..end].to_string())
}

/// Records that a report with `signature` from app `version` came at `now`.
fn count(
    conn: &Connection,
    signature: &str,
    exception: Option<&str>,
    version: Option<String>,
    now: i64,
) -> Result<(), Error> {
    let known: Option<String> = conn
        .query_row(
            "select app_versions from crash_signatures where signature = ?1",
            [signature],
            |r| r.get(0),
        )
        .optional()?;

    // The versions seen, the latest last; a version seen again moves to the end.
    let mut versions: Vec<String> = known
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?
        .unwrap_or_default();
    if let Some(version) = version {
        versions.retain(|seen| *seen != version);
        versions.push(version);
    }
    let excess = versions.len().saturating_sub(MAX_VERSIONS);
    versions.drain(..excess);
    let versions = serde_json::to_string(&versions)?;

    // The latest report names the exception, so the line is no older than `last_seen` and
    // is deleted with the row 30 days after it.
    conn.execute(
        "insert into crash_signatures
             (signature, first_seen, last_seen, count, exception, app_versions)
         values (?1, ?2, ?2, 1, ?3, ?4)
         on conflict (signature) do update
             set last_seen = excluded.last_seen, count = count + 1,
                 exception = excluded.exception, app_versions = excluded.app_versions",
        params![signature, now, exception, versions],
    )?;
    Ok(())
}

/// Deletes reports and signatures last seen [`RETENTION_DAYS`] ago or earlier; the worker
/// calls it on every tick. A signature's reports are never younger than its `last_seen`, so
/// no report is left without its signature.
pub(crate) fn prune(conn: &Connection, now: i64) -> Result<(), Error> {
    let cutoff = now - RETENTION_DAYS * DAY_MS;
    conn.execute(
        "delete from crash_reports where received_at <= ?1",
        [cutoff],
    )?;
    conn.execute(
        "delete from crash_signatures where last_seen <= ?1",
        [cutoff],
    )?;
    Ok(())
}

/// A signature as the dev console lists it.
#[derive(Debug, Serialize)]
pub(crate) struct Group {
    signature: String,
    count: i64,
    first_seen: i64,
    last_seen: i64,
    /// The app version of the latest report that named one.
    app_version: Option<String>,
    /// The versions seen, the latest last.
    app_versions: Vec<String>,
    /// The first line of the first report's stack trace: the exception and its message.
    exception: Option<String>,
    /// The ids of the stored reports, oldest first.
    reports: Vec<i64>,
}

/// The signatures seen most recently, newest first.
pub(crate) fn groups(conn: &Connection) -> Result<Vec<Group>, Error> {
    let mut stmt = conn.prepare(
        "select signature, count, first_seen, last_seen, exception, app_versions,
                (select json_group_array(id) from (
                     select id from crash_reports
                     where crash_reports.signature = crash_signatures.signature order by id
                 ))
         from crash_signatures order by last_seen desc, signature limit ?1",
    )?;
    let rows = stmt.query_map([LISTED], |r| {
        Ok((
            Group {
                signature: r.get(0)?,
                count: r.get(1)?,
                first_seen: r.get(2)?,
                last_seen: r.get(3)?,
                exception: r.get(4)?,
                app_version: None,
                app_versions: Vec::new(),
                reports: Vec::new(),
            },
            r.get::<_, String>(5)?,
            r.get::<_, String>(6)?,
        ))
    })?;

    // The versions and report ids are JSON arrays in the database.
    let mut groups = Vec::new();
    for row in rows {
        let (mut group, versions, reports) = row?;
        group.app_versions = serde_json::from_str(&versions)?;
        group.app_version = group.app_versions.last().cloned();
        group.reports = serde_json::from_str(&reports)?;
        groups.push(group);
    }
    Ok(groups)
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

    // The signature comes from the stack trace as sent, as the app computes it.
    let signature = signature(&report);
    strip(&mut report);
    match insert(&app.store.lock().unwrap().conn, &report, &signature, now) {
        Ok(_) => StatusCode::NO_CONTENT,
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
    const SIGNATURE: &str = "5e1f0c3a9b7d2e4f6a8c0b1d3e5f7a9c2b4d6e8f0a1c3e5b7d9f2a4c6e8b0d1f";
    const OTHER: &str = "9d8c7b6a5f4e3d2c1b0a9f8e7d6c5b4a3f2e1d0c9b8a7f6e5d4c3b2a1f0e9d8c";
    const MINUTE_MS: i64 = 60_000;

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

    /// `tests/fixtures/crash_redaction.json`. The app redacts with the same token rule before
    /// it sends; the `device` cases need the device's hosts and token, so only the app reads them.
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Redaction {
        #[serde(rename = "_about")]
        _about: String,
        version: u32,
        tokens: Vec<TokenCase>,
        #[serde(rename = "device")]
        _device: Vec<Value>,
    }

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct TokenCase {
        name: String,
        #[serde(default, rename = "in")]
        place: Place,
        text: String,
        expected: String,
    }

    /// Where a case's text stands in a report.
    #[derive(serde::Deserialize, Default)]
    #[serde(rename_all = "snake_case")]
    enum Place {
        /// A string value.
        #[default]
        Text,
        /// The name of a preference in `SHARED_PREFERENCES`.
        PreferenceKey,
    }

    #[test]
    fn tokens_match_the_shared_cases() {
        let text = include_str!("../tests/fixtures/crash_redaction.json");
        let fixture: Redaction = serde_json::from_str(text).unwrap();
        assert_eq!(fixture.version, 1);
        assert!(!fixture.tokens.is_empty());

        // Every string and key name in a report goes through `scrub`; these cases hold no
        // email addresses.
        for case in fixture.tokens {
            let (mut value, expected) = match case.place {
                Place::Text => (Value::String(case.text), Value::String(case.expected)),
                Place::PreferenceKey => (
                    json!({SHARED_PREFERENCES: {"default": {case.text: "1"}}}),
                    json!({SHARED_PREFERENCES: {"default": {case.expected: "1"}}}),
                ),
            };
            scrub(&mut value);
            assert_eq!(value, expected, "{}", case.name);
        }
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
        // Every accepted report is counted; only the first of one crash are stored.
        let total: i64 = app
            .store
            .lock()
            .unwrap()
            .conn
            .query_row("select sum(count) from crash_signatures", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 2 * REPORTS_PER_HOUR as i64 + 2);
        assert_eq!(stored(&app).len(), MAX_SAMPLES as usize);
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
    fn reports_are_kept_thirty_days() {
        let conn = Connection::open_in_memory().unwrap();
        initialize(&conn).unwrap();
        let count = || -> i64 {
            conn.query_row("select count(*) from crash_reports", [], |r| r.get(0))
                .unwrap()
        };

        // The worker deletes reports once they are older than the retention.
        let old = NOW_MS - RETENTION_DAYS * DAY_MS;
        insert(&conn, &report(), SIGNATURE, old).unwrap();
        insert(&conn, &report(), SIGNATURE, old + 1).unwrap();
        prune(&conn, NOW_MS).unwrap();
        assert_eq!(
            count(),
            1,
            "exactly 30 days old is deleted, younger is kept"
        );
        prune(&conn, NOW_MS + 1).unwrap();
        assert_eq!(count(), 0);

        // An insert also deletes what is past the retention.
        insert(&conn, &report(), SIGNATURE, old).unwrap();
        insert(&conn, &report(), SIGNATURE, NOW_MS).unwrap();
        assert_eq!(count(), 1);
    }

    #[tokio::test]
    async fn the_tick_deletes_old_reports() {
        fix_now(NOW_MS);
        let (app, path) = fixture();
        insert(
            &app.store.lock().unwrap().conn,
            &report(),
            SIGNATURE,
            NOW_MS - RETENTION_DAYS * DAY_MS,
        )
        .unwrap();
        crate::worker::tick(&app).unwrap();
        assert!(stored(&app).is_empty());
        cleanup(app, path);
    }

    /// A report the app signed with `signature`, from app version `version`.
    fn signed(signature: &str, version: &str) -> Value {
        let mut report = report();
        report["CUSTOM_DATA"][SIGNATURE_FIELD] = signature.into();
        report["APP_VERSION_NAME"] = version.into();
        report
    }

    /// The stored reports' arrival times and signatures, oldest first.
    fn samples(app: &App) -> Vec<(i64, String)> {
        let store = app.store.lock().unwrap();
        let mut stmt = store
            .conn
            .prepare("select received_at, signature from crash_reports order by id")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    /// One signature's row: first and last seen, count and app versions.
    fn seen(conn: &Connection, signature: &str) -> Option<(i64, i64, i64, Value)> {
        conn.query_row(
            "select first_seen, last_seen, count, app_versions
             from crash_signatures where signature = ?1",
            [signature],
            |r| {
                let versions: String = r.get(3)?;
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    serde_json::from_str(&versions).unwrap(),
                ))
            },
        )
        .optional()
        .unwrap()
    }

    /// The signature whose row was last seen at `at`.
    fn seen_at(app: &App, at: i64) -> String {
        let store = app.store.lock().unwrap();
        store
            .conn
            .query_row(
                "select signature from crash_signatures where last_seen = ?1",
                [at],
                |r| r.get(0),
            )
            .unwrap()
    }

    /// The cases in `tests/fixtures/crash_signatures.json`: name, stack trace, signature.
    fn shared_cases() -> Vec<(String, String, String)> {
        let text = include_str!("../tests/fixtures/crash_signatures.json");
        let fixture: Value = serde_json::from_str(text).unwrap();
        fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|case| {
                let field = |name: &str| case[name].as_str().unwrap().to_string();
                (field("name"), field("stack_trace"), field("signature"))
            })
            .collect()
    }

    #[tokio::test]
    async fn the_same_signature_is_stored_twice_and_then_counted() {
        fix_now(NOW_MS);
        let (app, path) = fixture();
        let trace = report()["STACK_TRACE"].as_str().unwrap().to_string();
        let mine = crate::crash_signature::of_trace(&trace);

        // Three reports with one signature: the first two are kept, the third only counted.
        for (minute, version) in [(0, "2.21.0"), (1, "2.21.0"), (2, "2.22.0")] {
            fix_now(NOW_MS + minute * MINUTE_MS);
            let sent = signed(&mine, version).to_string();
            assert_eq!(
                send(&app, "192.0.2.20:4000", sent).await,
                StatusCode::NO_CONTENT
            );
        }
        assert_eq!(
            samples(&app),
            vec![(NOW_MS, mine.clone()), (NOW_MS + MINUTE_MS, mine.clone())]
        );
        let row = seen(&app.store.lock().unwrap().conn, &mine);
        assert_eq!(
            row,
            Some((
                NOW_MS,
                NOW_MS + 2 * MINUTE_MS,
                3,
                json!(["2.21.0", "2.22.0"])
            ))
        );

        // The stored report keeps the signature, though it looks like a token.
        let saved = &stored(&app)[0].3;
        assert_eq!(saved["CUSTOM_DATA"][SIGNATURE_FIELD], mine);

        // Another signature is stored on its own; one in capitals is the same signature.
        fix_now(NOW_MS + 3 * MINUTE_MS);
        let mut other = signed(OTHER, "2.22.0");
        other["STACK_TRACE"] = "java.lang.IllegalStateException: other".into();
        other["CUSTOM_DATA"][SIGNATURE_FIELD] =
            crate::crash_signature::of_trace("java.lang.IllegalStateException: other").into();
        let other = other.to_string();
        assert_eq!(
            send(&app, "192.0.2.21:4000", other).await,
            StatusCode::NO_CONTENT
        );
        let upper = signed(&mine.to_ascii_uppercase(), "2.21.0").to_string();
        assert_eq!(
            send(&app, "192.0.2.21:4000", upper).await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(samples(&app).len(), 3);
        let row = seen(&app.store.lock().unwrap().conn, &mine).unwrap();
        assert_eq!((row.2, row.3), (4, json!(["2.22.0", "2.21.0"])));
        cleanup(app, path);
    }

    #[tokio::test]
    async fn without_a_signature_the_server_computes_it() {
        fix_now(NOW_MS);
        let (app, path) = fixture();

        // Every shared case groups under the signature the fixture gives for its stack trace.
        for (index, (name, trace, expected)) in shared_cases().into_iter().enumerate() {
            let at = NOW_MS + index as i64;
            fix_now(at);
            let mut sent = report();
            sent["STACK_TRACE"] = trace.into();
            let peer = format!("192.0.2.{}:4000", 30 + index);
            assert_eq!(
                send(&app, &peer, sent.to_string()).await,
                StatusCode::NO_CONTENT
            );
            assert_eq!(seen_at(&app, at), expected, "{name}");
        }

        // A report without a stack trace has the signature of an empty one.
        let at = NOW_MS + MINUTE_MS;
        fix_now(at);
        let mut sent = report();
        sent.as_object_mut().unwrap().remove("STACK_TRACE");
        assert_eq!(
            send(&app, "192.0.2.60:4000", sent.to_string()).await,
            StatusCode::NO_CONTENT
        );
        let empty = crate::crash_signature::of_trace("");
        assert_eq!(seen_at(&app, at), empty);
        cleanup(app, path);
    }

    #[tokio::test]
    async fn a_malformed_signature_is_ignored_and_computed_instead() {
        fix_now(NOW_MS);
        let (app, path) = fixture();
        let (_, trace, expected) = shared_cases().remove(0);
        let long = format!("{SIGNATURE}0");
        let malformed = [
            json!("abc"),
            json!("g".repeat(64)),
            json!(&SIGNATURE[1..]),
            json!(long),
            json!(12),
            json!(null),
        ];

        // Each malformed value is ignored and scrubbed like any other field.
        for (index, value) in malformed.into_iter().enumerate() {
            let at = NOW_MS + index as i64;
            fix_now(at);
            let mut sent = report();
            sent["STACK_TRACE"] = trace.clone().into();
            sent["CUSTOM_DATA"][SIGNATURE_FIELD] = value;
            let peer = format!("192.0.2.{}:4000", 70 + index);
            assert_eq!(
                send(&app, &peer, sent.to_string()).await,
                StatusCode::NO_CONTENT
            );
            assert_eq!(seen_at(&app, at), expected, "{index}");
        }
        let saved = &stored(&app)[1].3;
        assert_eq!(saved["CUSTOM_DATA"][SIGNATURE_FIELD], json!("g".repeat(64)));
        cleanup(app, path);

        // A well-formed signature at the top level of the report is used too.
        let (app, path) = fixture();
        fix_now(NOW_MS);
        let mut sent = report();
        let mine = crate::crash_signature::of_trace(sent["STACK_TRACE"].as_str().unwrap());
        sent[SIGNATURE_FIELD] = mine.to_ascii_uppercase().into();
        assert_eq!(
            send(&app, "192.0.2.80:4000", sent.to_string()).await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(seen_at(&app, NOW_MS), mine);
        assert_eq!(stored(&app)[0].3[SIGNATURE_FIELD], mine);
        cleanup(app, path);
    }

    #[test]
    fn a_signature_is_kept_while_it_has_reports_or_was_seen_lately() {
        let conn = Connection::open_in_memory().unwrap();
        initialize(&conn).unwrap();
        let reports = |signature: &str| -> i64 {
            conn.query_row(
                "select count(*) from crash_reports where signature = ?1",
                [signature],
                |r| r.get(0),
            )
            .unwrap()
        };
        let old = NOW_MS - RETENTION_DAYS * DAY_MS;

        // Two reports of one signature just under 30 days ago and a third a day later, one of
        // another in between.
        for _ in 0..2 {
            insert(&conn, &report(), SIGNATURE, old + 1).unwrap();
        }
        insert(&conn, &report(), OTHER, old + 2).unwrap();
        insert(&conn, &report(), SIGNATURE, old + DAY_MS).unwrap();
        assert_eq!(reports(SIGNATURE), MAX_SAMPLES);

        // Seen again: its old samples are gone, the new report is kept, the count goes on.
        insert(&conn, &report(), SIGNATURE, NOW_MS + 1).unwrap();
        assert_eq!(reports(SIGNATURE), 1);
        assert_eq!(reports(OTHER), 1);
        assert_eq!(seen(&conn, SIGNATURE).unwrap().2, 4);
        assert_eq!(seen(&conn, SIGNATURE).unwrap().0, old + 1);

        // The other's report and row go once they are past the retention.
        prune(&conn, NOW_MS + 2).unwrap();
        assert_eq!(reports(OTHER), 0);
        assert_eq!(seen(&conn, OTHER), None);

        // Without reports a signature stays until it was last seen 30 days ago.
        conn.execute("delete from crash_reports", []).unwrap();
        prune(&conn, NOW_MS + 2).unwrap();
        assert!(seen(&conn, SIGNATURE).is_some());
        prune(&conn, NOW_MS + 1 + RETENTION_DAYS * DAY_MS).unwrap();
        assert_eq!(seen(&conn, SIGNATURE), None);
    }

    /// An empty database with the crash report tables.
    fn memory() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        initialize(&conn).unwrap();
        conn
    }

    /// The `index`th made-up signature, for tests that need many.
    fn nth(index: i64) -> String {
        format!("{index:064x}")
    }

    fn rows(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("select count(*) from {table}"), [], |r| r.get(0))
            .unwrap()
    }

    fn exception_of(conn: &Connection, signature: &str) -> Option<String> {
        conn.query_row(
            "select exception from crash_signatures where signature = ?1",
            [signature],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn the_exception_is_the_latest_and_at_most_a_kibibyte() {
        let conn = memory();

        // A long first line is cut at a character boundary.
        let long = format!(
            "java.lang.IllegalStateException: {}",
            "\u{e9}".repeat(MAX_EXCEPTION_BYTES)
        );
        let trace = format!("{long}\n\tat a.B.c(B.kt:1)");
        insert(&conn, &json!({"STACK_TRACE": trace}), SIGNATURE, NOW_MS).unwrap();
        let kept = exception_of(&conn, SIGNATURE).unwrap();
        assert_eq!(kept.len(), MAX_EXCEPTION_BYTES - 1);
        assert!(long.starts_with(&kept));

        // A later report replaces it, so the line is never older than the signature's last report.
        let later = "java.lang.IllegalStateException: 7 left";
        insert(&conn, &json!({"STACK_TRACE": later}), SIGNATURE, NOW_MS + 1).unwrap();
        assert_eq!(exception_of(&conn, SIGNATURE).as_deref(), Some(later));
    }

    #[test]
    fn at_most_five_hundred_reports_are_stored_an_hour() {
        let conn = memory();
        let small = json!({"STACK_TRACE": "x"});

        // A known signature with one sample, then new signatures fill the hour.
        assert_eq!(
            insert(&conn, &small, SIGNATURE, NOW_MS).unwrap(),
            Outcome::Stored
        );
        for index in 1..STORED_PER_HOUR {
            let outcome = insert(&conn, &small, &nth(index), NOW_MS + index).unwrap();
            assert_eq!(outcome, Outcome::Stored, "{index}");
        }

        // Over the cap a new signature is dropped and a known one only counted.
        let late = NOW_MS + HOUR_MS - 1;
        assert_eq!(
            insert(&conn, &small, OTHER, late).unwrap(),
            Outcome::Dropped
        );
        assert_eq!(seen(&conn, OTHER), None);
        assert_eq!(
            insert(&conn, &small, SIGNATURE, late).unwrap(),
            Outcome::Counted
        );
        assert_eq!(seen(&conn, SIGNATURE).unwrap().2, 2);
        assert_eq!(rows(&conn, "crash_reports"), STORED_PER_HOUR);

        // An hour after the first report there is room again.
        assert_eq!(
            insert(&conn, &small, OTHER, NOW_MS + HOUR_MS).unwrap(),
            Outcome::Stored
        );
    }

    #[test]
    fn signatures_are_capped_without_evicting_samples() {
        let conn = memory();
        let small = json!({"STACK_TRACE": "x"});

        // Fill the table, one new signature a minute so the hourly cap is not reached.
        for index in 0..MAX_SIGNATURES {
            let outcome = insert(&conn, &small, &nth(index), NOW_MS + index * MINUTE_MS).unwrap();
            assert_eq!(outcome, Outcome::Stored, "{index}");
        }
        let later = NOW_MS + MAX_SIGNATURES * MINUTE_MS;

        // Every signature has samples: a new one is dropped, a known one still stored.
        assert_eq!(
            insert(&conn, &small, OTHER, later).unwrap(),
            Outcome::Dropped
        );
        assert_eq!(seen(&conn, OTHER), None);
        assert_eq!(
            insert(&conn, &small, &nth(0), later).unwrap(),
            Outcome::Stored
        );
        assert_eq!(rows(&conn, "crash_signatures"), MAX_SIGNATURES);
        assert_eq!(rows(&conn, "crash_reports"), MAX_SIGNATURES + 1);

        // Signatures without samples make room, the least recently seen first.
        conn.execute(
            "delete from crash_reports where signature in (?1, ?2)",
            [nth(5), nth(3)],
        )
        .unwrap();
        assert_eq!(
            insert(&conn, &small, OTHER, later + 1).unwrap(),
            Outcome::Stored
        );
        assert_eq!(seen(&conn, &nth(3)), None);
        assert!(seen(&conn, &nth(5)).is_some());
        assert_eq!(rows(&conn, "crash_signatures"), MAX_SIGNATURES);
    }

    #[test]
    fn email_addresses_are_removed_from_every_string() {
        let mut value = json!({
            "LOGCAT": "D/Sync: signed in as player.one+anki@example.co.uk, ok",
            "USER_COMMENT": "Write to me: Hill@Example.org.",
            "nested": {"list": ["a@b.cd"]},
            "STACK_TRACE": "java.lang.Object@1a2b3c\n\tat a.B.c(B.kt:1)",
            "other": "@Override, user@localhost, @x.org and x@.org"
        });
        strip(&mut value);
        assert_eq!(
            value,
            json!({
                "LOGCAT": format!("D/Sync: signed in as {REMOVED}, ok"),
                "USER_COMMENT": format!("Write to me: {REMOVED}."),
                "nested": {"list": [REMOVED]},
                "STACK_TRACE": "java.lang.Object@1a2b3c\n\tat a.B.c(B.kt:1)",
                "other": "@Override, user@localhost, @x.org and x@.org"
            })
        );
    }

    /// A preference key can name an account; its name is scrubbed like a string.
    #[test]
    fn email_addresses_are_removed_from_key_names() {
        let mut value = json!({SHARED_PREFERENCES: {"default": {"deckOrder:player@example.com": "2"}}});
        strip(&mut value);
        assert_eq!(
            value,
            json!({SHARED_PREFERENCES: {"default": {format!("deckOrder:{REMOVED}"): "2"}}})
        );
    }

    #[tokio::test]
    async fn a_sent_signature_counts_only_when_it_matches_the_trace() {
        fix_now(NOW_MS);
        let (app, path) = fixture();
        let trace = report()["STACK_TRACE"].as_str().unwrap().to_string();
        let computed = crate::crash_signature::of_trace(&trace);

        // Another crash's signature, in custom data or at the top, is replaced by the trace's.
        assert_eq!(
            send(&app, "192.0.2.90:4000", signed(OTHER, "2.21.0").to_string()).await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(seen_at(&app, NOW_MS), computed);
        fix_now(NOW_MS + 1);
        let mut top = report();
        top[SIGNATURE_FIELD] = OTHER.into();
        assert_eq!(
            send(&app, "192.0.2.91:4000", top.to_string()).await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(seen_at(&app, NOW_MS + 1), computed);
        assert_eq!(seen(&app.store.lock().unwrap().conn, OTHER), None);

        // The stored report keeps what was sent, so a mismatch can be looked into.
        assert_eq!(stored(&app)[0].3["CUSTOM_DATA"][SIGNATURE_FIELD], OTHER);

        // The trace's own signature, in capitals too, is the same crash.
        fix_now(NOW_MS + 2);
        let upper = signed(&computed.to_ascii_uppercase(), "2.21.0").to_string();
        assert_eq!(
            send(&app, "192.0.2.92:4000", upper).await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            seen(&app.store.lock().unwrap().conn, &computed).unwrap().2,
            3
        );
        cleanup(app, path);
    }

    fn fix_now(ms: i64) {
        crate::app::fix_now_ms(Some(ms));
    }
}
