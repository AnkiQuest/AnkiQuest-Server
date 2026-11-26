//! Player preferences that clients keep the same on every device. Each key holds one value and
//! the time it was chosen; the newest choice wins, key by key, so two devices changing different
//! settings keep both changes.
//!
//! The keys and their types are defined once, in `tests/fixtures/synced_settings.json`, which
//! both clients test against too.

use crate::access::require_owner;
use crate::app::{App, now_ms};
use crate::store::Error;
use crate::web_util::store_error;
use ankiquest_rules::OptimizeState;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

const PATH: &str = "/api/settings/{user}";

/// Room for the known keys, a full countdown list and a few dozen newer keys with their times.
const BODY_LIMIT: usize = 65536;

/// The longest key name, as `key_rules.pattern` in the shared fixture allows.
const MAX_NAME_LEN: usize = 64;

const SCHEMA: &str = include_str!("../tests/fixtures/synced_settings.json");

/// What values a key accepts.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Kind {
    Bool,
    Int {
        min: i64,
        max: i64,
    },
    Countdowns(crate::countdowns::Limits),
    /// The optimize reminder's state, exactly as crates/rules writes it.
    OptimizeReminder,
}

impl Kind {
    fn accepts(&self, value: &Value) -> bool {
        match self {
            Kind::Bool => value.is_boolean(),
            Kind::Int { min, max } => value.as_i64().is_some_and(|n| (*min..=*max).contains(&n)),
            Kind::Countdowns(limits) => limits.accepts(value),
            Kind::OptimizeReminder => optimize_reminder(value),
        }
    }
}

/// A state of the optimize reminder in the form the clients write: a known kind, its fields and
/// no others, and times a client writes (epoch milliseconds).
fn optimize_reminder(value: &Value) -> bool {
    let Ok(state) = serde_json::from_value::<OptimizeState>(value.clone()) else {
        return false;
    };
    state.valid() && serde_json::to_value(state).is_ok_and(|written| written == *value)
}

#[derive(Debug, Deserialize)]
struct Key {
    key: String,
    #[serde(flatten)]
    kind: Kind,
}

/// Limits for every key, known or not.
#[derive(Debug, Deserialize)]
struct Rules {
    max_keys: usize,
    max_value_bytes: usize,
}

#[derive(Deserialize)]
struct Schema {
    keys: Vec<Key>,
    key_rules: Rules,
}

/// The known keys by name and the limits, read once from the shared fixture.
fn schema() -> &'static (BTreeMap<String, Kind>, Rules) {
    static SCHEMA_ONCE: OnceLock<(BTreeMap<String, Kind>, Rules)> = OnceLock::new();
    SCHEMA_ONCE.get_or_init(|| {
        let schema: Schema =
            serde_json::from_str(SCHEMA).expect("tests/fixtures/synced_settings.json is valid");
        let keys = schema
            .keys
            .into_iter()
            .map(|key| (key.key, key.kind))
            .collect();
        (keys, schema.key_rules)
    })
}

fn keys() -> &'static BTreeMap<String, Kind> {
    &schema().0
}

fn rules() -> &'static Rules {
    &schema().1
}

/// A lowercase snake_case name starting with a letter, at most [`MAX_NAME_LEN`] bytes.
fn valid_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let starts_with_letter = bytes.next().is_some_and(|b| b.is_ascii_lowercase());
    starts_with_letter
        && name.len() <= MAX_NAME_LEN
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Whether `value` may be stored under `key`: a known key's type and range, or any value up
/// to the size limit for a key this server does not know.
fn storable(key: &str, value: &Value) -> bool {
    match keys().get(key) {
        Some(kind) => kind.accepts(value),
        None => value.to_string().len() <= rules().max_value_bytes,
    }
}

/// One setting's value and when the player chose it, in milliseconds since the epoch.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Entry {
    value: Value,
    updated_at: i64,
}

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists synced_settings (
             user text not null,
             key text not null,
             value text not null,
             updated_at integer not null,
             primary key (user, key)
         ) without rowid;",
    )?;
    Ok(())
}

/// The player's stored settings. Rows a known key no longer accepts are left out.
fn load(conn: &Connection, user: &str) -> Result<BTreeMap<String, Entry>, Error> {
    let mut stmt =
        conn.prepare("select key, value, updated_at from synced_settings where user = ?1")?;
    let rows = stmt.query_map([user], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    let mut found = BTreeMap::new();
    for row in rows {
        let (key, text, updated_at) = row?;
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if storable(&key, &value) {
            found.insert(key, Entry { value, updated_at });
        }
    }
    Ok(found)
}

/// The change with future times moved to `now`. None when a key breaks the name rule, a value
/// cannot be stored under its key, or a time is negative.
fn checked(incoming: BTreeMap<String, Entry>, now: i64) -> Option<BTreeMap<String, Entry>> {
    let mut entries = BTreeMap::new();
    for (key, entry) in incoming {
        if !valid_name(&key) || !storable(&key, &entry.value) || entry.updated_at < 0 {
            return None;
        }
        let updated_at = entry.updated_at.min(now);
        entries.insert(
            key,
            Entry {
                value: entry.value,
                updated_at,
            },
        );
    }
    Some(entries)
}

/// How a merge ended.
#[derive(Debug, PartialEq)]
enum Merge {
    Stored,
    /// The player would keep more keys than allowed; nothing was stored.
    TooManyKeys,
}

/// Stores each entry newer than the one stored; an equally old one keeps the stored value.
fn merge(conn: &Connection, user: &str, entries: &BTreeMap<String, Entry>) -> Result<Merge, Error> {
    let transaction = conn.unchecked_transaction()?;

    // Count the keys the player would keep, stored ones included.
    let mut stmt = transaction.prepare("select key from synced_settings where user = ?1")?;
    let mut names = stmt
        .query_map([user], |row| row.get::<_, String>(0))?
        .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
    drop(stmt);
    names.extend(entries.keys().cloned());
    if names.len() > rules().max_keys {
        return Ok(Merge::TooManyKeys);
    }

    for (key, entry) in entries {
        transaction.execute(
            "insert into synced_settings (user, key, value, updated_at) values (?1, ?2, ?3, ?4)
             on conflict (user, key) do update set value = excluded.value, updated_at = excluded.updated_at
             where excluded.updated_at > synced_settings.updated_at",
            params![user, key, entry.value.to_string(), entry.updated_at],
        )?;
    }
    transaction.commit()?;
    Ok(Merge::Stored)
}

#[derive(Debug, Serialize)]
struct Settings {
    settings: BTreeMap<String, Entry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    settings: BTreeMap<String, Entry>,
}

async fn get_settings(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Settings>, StatusCode> {
    require_owner(&app, &user, &headers)?;
    let store = app.store.lock().unwrap();
    let settings = load(&store.conn, &user).map_err(store_error)?;
    Ok(Json(Settings { settings }))
}

async fn put_settings(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
    Json(change): Json<Change>,
) -> Result<Json<Settings>, StatusCode> {
    require_owner(&app, &user, &headers)?;
    let entries = checked(change.settings, now_ms()).ok_or(StatusCode::BAD_REQUEST)?;
    let store = app.store.lock().unwrap();
    if merge(&store.conn, &user, &entries).map_err(store_error)? == Merge::TooManyKeys {
        return Err(StatusCode::BAD_REQUEST);
    }
    let settings = load(&store.conn, &user).map_err(store_error)?;
    Ok(Json(Settings { settings }))
}

pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new().route(
        PATH,
        get(get_settings)
            .put(put_settings)
            .layer(DefaultBodyLimit::max(BODY_LIMIT)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use axum::extract::Path as UrlPath;

    #[derive(Deserialize)]
    struct Fixture {
        server_merge: Vec<Case>,
    }

    #[derive(Deserialize)]
    struct Case {
        name: String,
        now_ms: i64,
        stored: BTreeMap<String, Entry>,
        incoming: BTreeMap<String, Entry>,
        #[serde(default)]
        expected: BTreeMap<String, Entry>,
        #[serde(default)]
        refused: bool,
    }

    fn entry(value: Value, updated_at: i64) -> Entry {
        Entry { value, updated_at }
    }

    #[test]
    fn the_shared_fixture_names_every_key_once_with_a_valid_default() {
        let fixture: Value = serde_json::from_str(SCHEMA).unwrap();
        let listed = fixture["keys"].as_array().unwrap();
        assert_eq!(listed.len(), keys().len(), "keys are unique");
        for key in listed {
            let name = key["key"].as_str().unwrap();
            assert!(keys()[name].accepts(&key["default"]), "{name} default");
            assert!(valid_name(name), "{name} follows the name rule");
        }
        assert_eq!(
            fixture["key_rules"]["pattern"],
            format!("^[a-z][a-z0-9_]{{0,{}}}$", MAX_NAME_LEN - 1)
        );
    }

    #[test]
    fn key_names_follow_the_rule() {
        let longest = "a".repeat(MAX_NAME_LEN);
        for good in ["a", "notify_rank", "key_2", longest.as_str()] {
            assert!(valid_name(good), "{good}");
        }
        let too_long = "a".repeat(MAX_NAME_LEN + 1);
        for bad in ["", "_a", "2a", "Notify", "a-b", "a b", too_long.as_str()] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn merges_follow_the_shared_cases() {
        let fixture: Fixture = serde_json::from_str(SCHEMA).unwrap();
        for case in fixture.server_merge {
            let (store, path) = temporary_store();
            merge(&store.conn, "hill", &case.stored).unwrap();
            let before = load(&store.conn, "hill").unwrap();
            let merged = checked(case.incoming, case.now_ms)
                .map(|entries| merge(&store.conn, "hill", &entries).unwrap());
            let after = load(&store.conn, "hill").unwrap();
            if case.refused {
                assert_ne!(merged, Some(Merge::Stored), "{}: refused", case.name);
                assert_eq!(after, before, "{}: nothing stored", case.name);
            } else {
                assert_eq!(merged, Some(Merge::Stored), "{}: accepted", case.name);
                assert_eq!(after, case.expected, "{}", case.name);
            }
            cleanup(store, path);
        }
    }

    #[test]
    fn rows_a_known_key_no_longer_accepts_are_left_out() {
        let (store, path) = temporary_store();
        store
            .conn
            .execute_batch(
                "insert into synced_settings values ('hill', 'later', 'true', 1);
                 insert into synced_settings values ('hill', 'notify_rank', '3', 1);
                 insert into synced_settings values ('hill', 'deck_status', 'false', 1);",
            )
            .unwrap();
        assert_eq!(
            load(&store.conn, "hill").unwrap(),
            BTreeMap::from([
                ("deck_status".into(), entry(false.into(), 1)),
                ("later".into(), entry(true.into(), 1)),
            ])
        );
        cleanup(store, path);
    }

    #[tokio::test]
    async fn settings_are_private_and_kept_for_each_player() {
        let (app, path) = fixture();
        let put = |user: &str, token: &str, settings: Value| {
            put_settings(
                State(app.clone()),
                UrlPath(user.into()),
                headers(token),
                Json(serde_json::from_value(serde_json::json!({ "settings": settings })).unwrap()),
            )
        };
        assert_eq!(
            put("hill", "cerro", serde_json::json!({})).await.err(),
            Some(StatusCode::UNAUTHORIZED)
        );
        assert_eq!(
            get_settings(State(app.clone()), UrlPath("hill".into()), headers("cerro"))
                .await
                .err(),
            Some(StatusCode::UNAUTHORIZED)
        );
        assert_eq!(
            put(
                "hill",
                "hill",
                serde_json::json!({"notify_rank": {"value": 1, "updated_at": 5}})
            )
            .await
            .err(),
            Some(StatusCode::BAD_REQUEST)
        );
        let merged = put(
            "hill",
            "hill",
            serde_json::json!({
                "notify_rank": {"value": false, "updated_at": 5},
                "later": {"value": true, "updated_at": 5}
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(
            merged.settings,
            BTreeMap::from([
                ("later".into(), entry(true.into(), 5)),
                ("notify_rank".into(), entry(false.into(), 5)),
            ])
        );
        let read =
            |user: &str| get_settings(State(app.clone()), UrlPath(user.into()), headers(user));
        assert_eq!(read("hill").await.unwrap().0.settings, merged.settings);
        assert!(read("cerro").await.unwrap().0.settings.is_empty());
        cleanup(app, path);
    }
}
