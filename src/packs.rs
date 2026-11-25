//! Companion packs players upload so their friends see their companion
//! (`docs/companion-packs.md`). The checks and the rebuilt pack are `ankiquest-packs`'s; this
//! module stores packs, decides who may read them and answers the API.

use crate::access::authorized;
use crate::app::{App, now_ms};
use crate::game::DAY_MS;
use crate::store::Error;
use crate::web_util::{refuse, reply};
use ankiquest_art::Poses;
use ankiquest_packs::{Canonical, MAX_ARCHIVE, Refused};
use axum::body::to_bytes;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};
use tokio::sync::Semaphore;

/// The capability that says this server keeps packs.
pub(crate) const FEATURE: &str = "companion_packs";

/// Most packs one player keeps.
const MAX_PACKS: i64 = 5;

/// Most bytes of packs one player keeps.
const QUOTA: i64 = 48 * 1024 * 1024;

/// Most uploads one player starts in a day, refused ones included.
const UPLOADS_PER_DAY: i64 = 10;

/// Packs rebuilt at the same time; each decodes and encodes up to 128 pictures.
const REBUILDS: usize = 2;

static REBUILDING: Semaphore = Semaphore::const_new(REBUILDS);

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists player_packs (
            user text not null,
            sha256 text not null,
            id text not null,
            name text not null,
            version integer not null,
            licence text not null,
            credit text not null,
            notice text,
            size integer not null,
            zip blob not null,
            uploaded_at integer not null,
            primary key (user, sha256),
            unique (user, id)
        );
        create index if not exists player_packs_by_hash on player_packs (sha256);
        create table if not exists shown_packs (
            user text primary key,
            sha256 text not null
        ) without rowid;
        create table if not exists pack_reports (
            sha256 text not null,
            reporter text not null,
            reason text not null,
            created_at integer not null,
            primary key (sha256, reporter)
        ) without rowid;
        create table if not exists pack_removals (
            sha256 text primary key,
            removed_at integer not null
        ) without rowid;
        create table if not exists pack_uploads (
            user text not null,
            at integer not null
        );
        create index if not exists pack_uploads_by_user on pack_uploads (user, at);",
    )?;
    Ok(())
}

/// Who may read a pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Visibility {
    /// Its owner only.
    Owner,
    /// Its owner and their friends: the pack is their companion for friends. One per player.
    Friends,
}

/// Why a friend reported a pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Reason {
    Offensive,
    Copyright,
    Other,
}

impl Reason {
    fn name(self) -> &'static str {
        match self {
            Reason::Offensive => "offensive",
            Reason::Copyright => "copyright",
            Reason::Other => "other",
        }
    }
}

/// A stored pack as answers show it.
#[derive(Debug, Serialize)]
struct Pack {
    sha256: String,
    id: String,
    name: String,
    version: i64,
    licence: String,
    credit: String,
    notice: Option<String>,
    size: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    visibility: Option<Visibility>,
    #[serde(skip_serializing_if = "Option::is_none")]
    uploaded_at: Option<i64>,
}

/// The columns [`pack_row`] reads, prefixed `p.` for joins.
const COLUMNS: &str = "p.sha256, p.id, p.name, p.version, p.licence, p.credit, p.notice, p.size,
     p.uploaded_at, exists (select 1 from shown_packs s where s.user = p.user and s.sha256 = p.sha256)";

fn pack_row(row: &rusqlite::Row) -> rusqlite::Result<Pack> {
    let shown: bool = row.get(9)?;
    Ok(Pack {
        sha256: row.get(0)?,
        id: row.get(1)?,
        name: row.get(2)?,
        version: row.get(3)?,
        licence: row.get(4)?,
        credit: row.get(5)?,
        notice: row.get(6)?,
        size: row.get(7)?,
        uploaded_at: Some(row.get(8)?),
        visibility: Some(if shown {
            Visibility::Friends
        } else {
            Visibility::Owner
        }),
    })
}

/// The pose list uploads are checked against.
fn poses() -> &'static Poses {
    static POSES: OnceLock<Poses> = OnceLock::new();
    POSES.get_or_init(|| {
        Poses::parse(include_str!("../tests/fixtures/companion_poses.json"))
            .expect("the pose fixture parses")
    })
}

/// Whether `text` is a SHA-256 as packs are addressed: 64 lower-case hex digits.
fn is_hash(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn storage_failure(error: impl std::fmt::Display) -> Response {
    log_error!("companion pack storage failed: {error}");
    refuse(
        StatusCode::INTERNAL_SERVER_ERROR,
        "The companion could not be saved or loaded. Please try again.",
    )
}

/// `{"error", "reason"}` with `status`.
fn refuse_for(status: StatusCode, reason: &str, message: &str) -> Response {
    reply(status, json!({"error": message, "reason": reason}))
}

/// The player's packs, by name.
fn list_of(conn: &Connection, user: &str) -> rusqlite::Result<Vec<Pack>> {
    conn.prepare(&format!(
        "select {COLUMNS} from player_packs p where p.user = ?1 order by p.name, p.id"
    ))?
    .query_map([user], pack_row)?
    .collect()
}

fn one(conn: &Connection, user: &str, sha256: &str) -> rusqlite::Result<Option<Pack>> {
    conn.query_row(
        &format!("select {COLUMNS} from player_packs p where p.user = ?1 and p.sha256 = ?2"),
        params![user, sha256],
        pack_row,
    )
    .optional()
}

async fn list(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&app, &user, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let store = app.store.lock().unwrap();
    let packs = match list_of(&store.conn, &user) {
        Ok(packs) => packs,
        Err(error) => return storage_failure(error),
    };
    let used: i64 = packs.iter().map(|pack| pack.size).sum();
    reply(
        StatusCode::OK,
        json!({"packs": packs, "used": used, "quota": QUOTA, "max_packs": MAX_PACKS}),
    )
}

/// What an upload comes to before it is stored.
enum Admission {
    Allowed,
    /// [`UPLOADS_PER_DAY`] reached.
    Throttled,
}

/// Counts an upload toward the day's limit, forgetting uploads older than a day.
fn admit(conn: &Connection, user: &str, now: i64) -> rusqlite::Result<Admission> {
    conn.execute("delete from pack_uploads where at <= ?1", [now - DAY_MS])?;
    let recent: i64 = conn.query_row(
        "select count(*) from pack_uploads where user = ?1",
        [user],
        |row| row.get(0),
    )?;
    if recent >= UPLOADS_PER_DAY {
        return Ok(Admission::Throttled);
    }
    conn.execute(
        "insert into pack_uploads (user, at) values (?1, ?2)",
        params![user, now],
    )?;
    Ok(Admission::Allowed)
}

fn removed(conn: &Connection, sha256: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "select exists (select 1 from pack_removals where sha256 = ?1)",
        [sha256],
        |row| row.get(0),
    )
}

/// A pack this server built already, found by the hash of an upload: uploading a canonical
/// pack again keeps its bytes instead of encoding its pictures a second time.
fn rebuilt(conn: &Connection, sha256: &str) -> rusqlite::Result<Option<Stored>> {
    conn.query_row(
        "select sha256, id, name, version, licence, credit, notice, zip from player_packs
         where sha256 = ?1 limit 1",
        [sha256],
        |row| {
            Ok(Stored {
                sha256: row.get(0)?,
                id: row.get(1)?,
                name: row.get(2)?,
                version: row.get(3)?,
                licence: row.get(4)?,
                credit: row.get(5)?,
                notice: row.get(6)?,
                zip: row.get(7)?,
            })
        },
    )
    .optional()
}

/// A canonical pack about to be stored.
struct Stored {
    sha256: String,
    id: String,
    name: String,
    version: i64,
    licence: String,
    credit: String,
    notice: Option<String>,
    zip: Vec<u8>,
}

impl From<Canonical> for Stored {
    fn from(pack: Canonical) -> Self {
        Stored {
            sha256: pack.sha256,
            id: pack.info.id,
            name: pack.info.name,
            version: i64::try_from(pack.info.version).unwrap_or(i64::MAX),
            licence: pack.info.licence,
            credit: pack.info.credit,
            notice: pack.info.notice,
            zip: pack.zip,
        }
    }
}

/// What storing a pack came to.
enum Saved {
    Stored,
    /// More than [`MAX_PACKS`] packs or [`QUOTA`] bytes.
    OverQuota,
}

/// Stores `pack` for `user`, replacing their pack with the same id. If that one was shown to
/// friends, the new one is.
fn save(conn: &mut Connection, user: &str, pack: &Stored, now: i64) -> rusqlite::Result<Saved> {
    let tx = conn.transaction()?;
    let replaced: Option<String> = tx
        .query_row(
            "select sha256 from player_packs where user = ?1 and (id = ?2 or sha256 = ?3)",
            params![user, pack.id, pack.sha256],
            |row| row.get(0),
        )
        .optional()?;

    // The limits count every other pack the player keeps.
    let (count, bytes): (i64, i64) = tx.query_row(
        "select count(*), coalesce(sum(size), 0) from player_packs
         where user = ?1 and sha256 is not ?2",
        params![user, replaced],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let size = i64::try_from(pack.zip.len()).unwrap_or(i64::MAX);
    if count + 1 > MAX_PACKS || bytes + size > QUOTA {
        return Ok(Saved::OverQuota);
    }

    // Replace, keeping what friends see.
    let shown = match &replaced {
        Some(old) => {
            let shown: bool = tx.query_row(
                "select exists (select 1 from shown_packs where user = ?1 and sha256 = ?2)",
                params![user, old],
                |row| row.get(0),
            )?;
            tx.execute(
                "delete from player_packs where user = ?1 and sha256 = ?2",
                params![user, old],
            )?;
            shown
        }
        None => false,
    };
    tx.execute(
        "insert into player_packs
         (user, sha256, id, name, version, licence, credit, notice, size, zip, uploaded_at)
         values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            user,
            pack.sha256,
            pack.id,
            pack.name,
            pack.version,
            pack.licence,
            pack.credit,
            pack.notice,
            size,
            pack.zip,
            now
        ],
    )?;
    if shown {
        tx.execute(
            "update shown_packs set sha256 = ?2 where user = ?1",
            params![user, pack.sha256],
        )?;
    }
    tx.commit()?;
    Ok(Saved::Stored)
}

/// Checks and rebuilds `bytes` off the request threads, a few packs at a time.
async fn rebuild(bytes: Vec<u8>) -> Result<Result<Canonical, Refused>, String> {
    let _slot = REBUILDING
        .acquire()
        .await
        .map_err(|error| error.to_string())?;
    tokio::task::spawn_blocking(move || {
        ankiquest_packs::canonical(&bytes, poses(), crate::companions::official)
    })
    .await
    .map_err(|error| error.to_string())
}

async fn upload(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    request: Request,
) -> Response {
    // Authenticate before reading or decoding the body; the byte limit also covers chunked
    // bodies, which have no Content-Length.
    if !authorized(&app, &user, request.headers()) {
        return refuse(StatusCode::UNAUTHORIZED, "Connect your own account first.");
    }
    let Ok(bytes) = to_bytes(request.into_body(), MAX_ARCHIVE).await else {
        return refuse_for(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_large",
            "Choose a companion pack smaller than 32 MiB.",
        );
    };

    // Every attempt counts toward the day's limit, so refused packs cannot keep the server busy.
    let raw = ankiquest_packs::sha256(&bytes);
    let known = {
        let store = app.store.lock().unwrap();
        let checked = admit(&store.conn, &user, now_ms()).and_then(|admission| {
            Ok((
                admission,
                removed(&store.conn, &raw)?,
                rebuilt(&store.conn, &raw)?,
            ))
        });
        match checked {
            Ok((Admission::Throttled, _, _)) => {
                return refuse_for(
                    StatusCode::TOO_MANY_REQUESTS,
                    "throttled",
                    "You uploaded many companions today. Try again tomorrow.",
                );
            }
            Ok((Admission::Allowed, true, _)) => return removed_answer(),
            Ok((Admission::Allowed, false, known)) => known,
            Err(error) => return storage_failure(error),
        }
    };

    // A pack this server built is kept as it is; anything else is checked and rebuilt.
    let pack = match known {
        Some(pack) => pack,
        None => match rebuild(bytes.to_vec()).await {
            Ok(Ok(pack)) => Stored::from(pack),
            Ok(Err(refused)) => return refused_answer(&refused),
            Err(error) => {
                log_error!(user: user, "companion pack processing failed: {error}");
                return refuse(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "The companion could not be processed. Please try again.",
                );
            }
        },
    };

    // Store it, unless the rebuilt pack is one an administrator removed.
    let mut store = app.store.lock().unwrap();
    let saved = removed(&store.conn, &pack.sha256).and_then(|gone| {
        if gone {
            return Ok(None);
        }
        let saved = save(&mut store.conn, &user, &pack, now_ms())?;
        Ok(Some((saved, one(&store.conn, &user, &pack.sha256)?)))
    });
    match saved {
        Ok(None) => removed_answer(),
        Ok(Some((Saved::OverQuota, _))) => refuse_for(
            StatusCode::CONFLICT,
            "quota",
            "You keep as many companions as you can. Delete one first.",
        ),
        Ok(Some((Saved::Stored, Some(pack)))) => reply(StatusCode::OK, json!(pack)),
        Ok(Some((Saved::Stored, None))) => storage_failure("the stored pack is gone"),
        Err(error) => storage_failure(error),
    }
}

fn removed_answer() -> Response {
    refuse_for(
        StatusCode::FORBIDDEN,
        "removed",
        "This companion was removed and cannot be uploaded again.",
    )
}

fn refused_answer(refused: &Refused) -> Response {
    let status = match refused.reason {
        ankiquest_packs::Refusal::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        _ => StatusCode::BAD_REQUEST,
    };
    reply(
        status,
        json!({
            "error": "This companion pack cannot be used.",
            "reason": refused.reason.reason(),
            "detail": refused.detail,
        }),
    )
}

#[derive(Deserialize)]
struct Change {
    visibility: Visibility,
}

async fn change(
    State(app): State<Arc<App>>,
    Path((user, sha256)): Path<(String, String)>,
    headers: HeaderMap,
    Json(change): Json<Change>,
) -> Response {
    if !authorized(&app, &user, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut store = app.store.lock().unwrap();
    let result = (|| -> rusqlite::Result<Option<Pack>> {
        let tx = store.conn.transaction()?;
        let owned: bool = tx.query_row(
            "select exists (select 1 from player_packs where user = ?1 and sha256 = ?2)",
            params![user, sha256],
            |row| row.get(0),
        )?;
        if !owned {
            return Ok(None);
        }

        // One pack is shown to friends: showing this one replaces the last.
        match change.visibility {
            Visibility::Friends => tx.execute(
                "insert into shown_packs (user, sha256) values (?1, ?2)
                 on conflict (user) do update set sha256 = excluded.sha256",
                params![user, sha256],
            )?,
            Visibility::Owner => tx.execute(
                "delete from shown_packs where user = ?1 and sha256 = ?2",
                params![user, sha256],
            )?,
        };
        tx.commit()?;
        one(&store.conn, &user, &sha256)
    })();
    match result {
        Ok(Some(pack)) => reply(StatusCode::OK, json!(pack)),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => storage_failure(error),
    }
}

async fn delete(
    State(app): State<Arc<App>>,
    Path((user, sha256)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&app, &user, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut store = app.store.lock().unwrap();
    let result = (|| -> rusqlite::Result<usize> {
        let tx = store.conn.transaction()?;
        let deleted = tx.execute(
            "delete from player_packs where user = ?1 and sha256 = ?2",
            params![user, sha256],
        )?;
        tx.execute(
            "delete from shown_packs where user = ?1 and sha256 = ?2",
            params![user, sha256],
        )?;
        tx.commit()?;
        Ok(deleted)
    })();
    match result {
        Ok(0) => StatusCode::NOT_FOUND.into_response(),
        Ok(_) => no_store(StatusCode::NO_CONTENT),
        Err(error) => storage_failure(error),
    }
}

fn no_store(status: StatusCode) -> Response {
    (status, [(header::CACHE_CONTROL, "no-store")]).into_response()
}

/// The pack each of `viewer`'s friends shows, leaving out packs `viewer` reported.
fn shown_to(conn: &Connection, viewer: &str) -> rusqlite::Result<BTreeMap<String, Pack>> {
    let friends = crate::social::friends(conn, viewer)?;
    let mut statement = conn.prepare(&format!(
        "select {COLUMNS} from shown_packs s
         join player_packs p on p.user = s.user and p.sha256 = s.sha256
         where s.user = ?1 and not exists
             (select 1 from pack_reports r where r.sha256 = s.sha256 and r.reporter = ?2)"
    ))?;
    let mut shown = BTreeMap::new();
    for friend in friends {
        let pack = statement
            .query_row(params![friend, viewer], pack_row)
            .optional()?;
        if let Some(pack) = pack {
            let pack = Pack {
                visibility: None,
                uploaded_at: None,
                ..pack
            };
            shown.insert(friend, pack);
        }
    }
    Ok(shown)
}

async fn friends(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&app, &user, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match shown_to(&app.store.lock().unwrap().conn, &user) {
        Ok(shown) => reply(StatusCode::OK, json!({"friends": shown})),
        Err(error) => storage_failure(error),
    }
}

/// Who shows `sha256` to `viewer`: the first friend who does, by name.
fn shown_by(conn: &Connection, viewer: &str, sha256: &str) -> rusqlite::Result<Option<String>> {
    let friends = crate::social::friends(conn, viewer)?;
    let mut statement = conn.prepare(
        "select exists (select 1 from shown_packs s
             join player_packs p on p.user = s.user and p.sha256 = s.sha256
             where s.user = ?1 and s.sha256 = ?2)",
    )?;
    for friend in friends {
        if statement.query_row(params![friend, sha256], |row| row.get(0))? {
            return Ok(Some(friend));
        }
    }
    Ok(None)
}

/// The zip of `sha256`, if `viewer` owns it or a friend shows it.
fn readable(conn: &Connection, viewer: &str, sha256: &str) -> rusqlite::Result<Option<Vec<u8>>> {
    let owner = match shown_by(conn, viewer, sha256)? {
        Some(friend) => friend,
        None => viewer.to_string(),
    };
    conn.query_row(
        "select zip from player_packs where user = ?1 and sha256 = ?2",
        params![owner, sha256],
        |row| row.get(0),
    )
    .optional()
}

async fn file(
    State(app): State<Arc<App>>,
    Path((user, sha256)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&app, &user, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !is_hash(&sha256) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let zip = match readable(&app.store.lock().unwrap().conn, &user, &sha256) {
        Ok(Some(zip)) => zip,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(error) => return storage_failure(error),
    };
    let etag = format!("\"{sha256}\"");
    let unchanged = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(',').any(|tag| tag.trim() == etag));
    let headers = [
        (header::CONTENT_TYPE, "application/zip".to_string()),
        (header::ETAG, etag),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
        (
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"pack-{sha256}.zip\""),
        ),
    ];
    if unchanged {
        return (StatusCode::NOT_MODIFIED, headers).into_response();
    }
    (headers, zip).into_response()
}

#[derive(Deserialize)]
struct Report {
    reason: Reason,
}

async fn report(
    State(app): State<Arc<App>>,
    Path((user, sha256)): Path<(String, String)>,
    headers: HeaderMap,
    Json(report): Json<Report>,
) -> Response {
    if !authorized(&app, &user, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let store = app.store.lock().unwrap();
    let result = (|| -> rusqlite::Result<Option<(String, usize)>> {
        let Some(owner) = shown_by(&store.conn, &user, &sha256)? else {
            return Ok(None);
        };
        let added = store.conn.execute(
            "insert into pack_reports (sha256, reporter, reason, created_at)
             values (?1, ?2, ?3, ?4) on conflict do nothing",
            params![sha256, user, report.reason.name(), now_ms()],
        )?;
        Ok(Some((owner, added)))
    })();
    match result {
        Ok(Some((owner, added))) => {
            if added > 0 {
                log_warning!(
                    user: owner,
                    "companion pack {sha256} of {owner} reported by {user} as {}",
                    report.reason.name()
                );
            }
            no_store(StatusCode::NO_CONTENT)
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => storage_failure(error),
    }
}

async fn withdraw(
    State(app): State<Arc<App>>,
    Path((user, sha256)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&app, &user, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let result = app.store.lock().unwrap().conn.execute(
        "delete from pack_reports where sha256 = ?1 and reporter = ?2",
        params![sha256, user],
    );
    match result {
        Ok(_) => no_store(StatusCode::NO_CONTENT),
        Err(error) => storage_failure(error),
    }
}

/// Removes the pack `sha256` from every player who has it and keeps it from coming back
/// (`admin pack-remove`). Answers how many players had it.
pub(crate) fn remove(conn: &Connection, sha256: &str, now: i64) -> Result<usize, Error> {
    if !is_hash(sha256) {
        return Err(format!("{sha256} is not a pack hash: 64 lower-case hex digits").into());
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "insert into pack_removals (sha256, removed_at) values (?1, ?2) on conflict do nothing",
        params![sha256, now],
    )?;
    tx.execute("delete from shown_packs where sha256 = ?1", [sha256])?;
    let owners = tx.execute("delete from player_packs where sha256 = ?1", [sha256])?;
    tx.commit()?;
    Ok(owners)
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/packs/{user}", get(list).post(upload))
        .route("/api/packs/{user}/friends", get(friends))
        .route("/api/packs/{user}/file/{sha256}", get(file))
        .route(
            "/api/packs/{user}/reports/{sha256}",
            post(report).delete(withdraw),
        )
        .route("/api/packs/{user}/{sha256}", put(change).delete(delete))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use tests::sample_pack;
