//! Hard cards learned at last: a page to share each one, and telling friends about it, which
//! the owner can take back for a day, from friends who have not read it yet.
//! Only what [`Conquest`] carries is shared, never the card itself, unless its owner
//! chooses to add a label saying what the card was. The label also appears on the card and
//! page of the matching `conquered` win (see `wins::shared_words`).
use crate::access::require_owner;
use crate::game::{self, Conquest};
use crate::notifications::Outgoing;
use crate::store::Error;
use crate::{access, access::authorized, app::App, app::now_ms, web_util::store_error};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Html;
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Long enough for a word with its translation, short enough to stay a label.
pub const MAX_LABEL: usize = 120;

/// How long after telling friends the owner can take it back.
pub const UNDO_MS: i64 = 24 * 60 * 60 * 1000;

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists conquest_labels (
             user text not null,
             at integer not null,
             label text not null,
             primary key (user, at)
         ) without rowid;
         create table if not exists conquest_tells (
             user text not null,
             at integer not null,
             told_at integer not null,
             primary key (user, at)
         ) without rowid;
         create table if not exists conquest_tell_rows (
             user text not null,
             at integer not null,
             notification integer not null,
             primary key (user, at, notification)
         ) without rowid;",
    )?;
    Ok(())
}

pub(crate) fn label_of(conn: &Connection, user: &str, at: i64) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "select label from conquest_labels where user = ?1 and at = ?2",
        params![user, at],
        |r| r.get(0),
    )
    .optional()
}

/// One line of plain text, or `None` when nothing printable is left.
fn clean_label(raw: &str) -> Option<String> {
    crate::web_util::clean_line(raw, MAX_LABEL)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LabelUpdate {
    pub(crate) label: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct Shared {
    user: String,
    display: String,
    pub(crate) conquest: Conquest,
    /// What the card was, when its owner chose to say.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) label: Option<String>,
    /// Present for the owner only: whether friends have been told, and how many there are.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) told: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) friends: Option<usize>,
    /// For the owner only, once the worker has kept the conquest as a win: its id, so the
    /// app can share the win's card (`POST /api/share/{user}` with `to: "link"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) win: Option<i64>,
    /// For the owner only, while telling friends can still be taken back: until when (ms).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) undo_until: Option<i64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Told {
    pub(crate) told: usize,
}

#[derive(Debug, Serialize)]
pub(crate) struct Untold {
    /// The friends whose unread row was taken back.
    pub(crate) retracted: usize,
}

/// Until when the owner can take back telling friends about the card conquered at `at`:
/// a day after telling them, unless a friend has cheered or answered it since.
fn undo_until(conn: &Connection, user: &str, at: i64, now: i64) -> rusqlite::Result<Option<i64>> {
    let Some(told_at) = conn
        .query_row(
            "select told_at from conquest_tells where user = ?1 and at = ?2",
            params![user, at],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
    else {
        return Ok(None);
    };
    let answered: bool = conn.query_row(
        "select exists (select 1 from conquest_tell_rows t join notifications n on n.id = t.notification
             where t.user = ?1 and t.at = ?2 and (n.replied = 1
                 or exists (select 1 from reactions r where r.notification = n.id)))",
        params![user, at],
        |r| r.get(0),
    )?;
    let until = told_at + UNDO_MS;
    Ok((!answered && now < until).then_some(until))
}

fn told_key(at: i64) -> String {
    format!("conquest-told:{at}")
}

fn find(app: &App, user: &str, at: i64) -> Option<Conquest> {
    let players = app.players.read().unwrap();
    let reviews = players.get(user)?.reviews.clone();
    drop(players);
    game::conquests(&reviews, now_ms())
        .into_iter()
        .find(|c| c.at == at)
}

async fn page() -> Html<&'static str> {
    Html(web!("conquered.html"))
}

pub(crate) async fn show(
    State(app): State<Arc<App>>,
    Path((user, at)): Path<(String, i64)>,
    headers: HeaderMap,
) -> Result<Json<Shared>, StatusCode> {
    let viewer = access::viewer(&app, &headers);
    if !app
        .visible(&app.store.lock().unwrap().conn, &viewer, &user)
        .map_err(store_error)?
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let conquest = find(&app, &user, at).ok_or(StatusCode::NOT_FOUND)?;
    let label = {
        let conn = &app.store.lock().unwrap().conn;
        // Profiles became public by default after some labels were written for friends only.
        if app
            .visible_by_choice(conn, &viewer, &user)
            .map_err(store_error)?
        {
            label_of(conn, &user, at).map_err(store_error)?
        } else {
            None
        }
    };
    let (told, friends, win, undo) = if authorized(&app, &user, &headers) {
        let store = app.store.lock().unwrap();
        let told = store.is_seen(&user, &told_key(at)).map_err(store_error)?;
        let friends = crate::social::friends(&store.conn, &user)
            .map_err(store_error)?
            .len();
        let win = own_win(&store, &user, &conquest).map_err(store_error)?;
        let undo = undo_until(&store.conn, &user, at, now_ms()).map_err(store_error)?;
        (Some(told), Some(friends), win, undo)
    } else {
        (None, None, None, None)
    };
    Ok(Json(Shared {
        display: app.display(&user),
        user,
        conquest,
        label,
        told,
        friends,
        win,
        undo_until: undo,
    }))
}

/// The owner's win for `conquest`, so its card can be shared. A conquest the server already
/// announced without keeping a win, from before wins were kept, gets one now, quietly and with
/// no stats of a day long gone. One not announced yet waits for the worker, which keeps it.
fn own_win(
    store: &crate::store::Store,
    user: &str,
    conquest: &Conquest,
) -> Result<Option<i64>, Error> {
    let event = game::conquest_event(conquest);
    if let Some(id) = crate::wins::id_of(&store.conn, user, &event.key)? {
        return Ok(Some(id));
    }
    if !store.is_seen(user, &event.key)? {
        return Ok(None);
    }
    let day = store.clock(user)?.day(conquest.at);
    Ok(crate::wins::keep(
        &store.conn,
        user,
        &event.key,
        day,
        conquest.at,
        &event.title,
        &event.body,
        &serde_json::json!({}),
    )?)
}

/// Where the win for a conquered card stands, for sharing it as a moment.
pub(crate) enum ConquestWin {
    /// No such conquest of the player's.
    Missing,
    /// Not announced yet; the worker keeps its win shortly.
    Pending,
    Kept(i64),
}

/// The win for the card `user` conquered at `at`, kept now when it was announced before wins
/// were (see [`own_win`]).
pub(crate) fn conquest_win(app: &App, user: &str, at: i64) -> Result<ConquestWin, Error> {
    let Some(conquest) = find(app, user, at) else {
        return Ok(ConquestWin::Missing);
    };
    let store = app.store.lock().unwrap();
    Ok(match own_win(&store, user, &conquest)? {
        Some(id) => ConquestWin::Kept(id),
        None => ConquestWin::Pending,
    })
}

/// Sends the conquest to each friend's activity, once, however often it is asked.
pub(crate) async fn tell(
    State(app): State<Arc<App>>,
    Path((user, at)): Path<(String, i64)>,
    headers: HeaderMap,
) -> Result<Json<Told>, StatusCode> {
    require_owner(&app, &user, &headers)?;
    let conquest = find(&app, &user, at).ok_or(StatusCode::NOT_FOUND)?;
    let display = app.display(&user);
    let title = if conquest.leech {
        format!("{display} tamed a leech")
    } else {
        format!("{display} conquered a hard card")
    };
    let now = now_ms();
    let mut store = app.store.lock().unwrap();
    let body = match label_of(&store.conn, &user, at).map_err(store_error)? {
        Some(label) => format!(
            "They forgot “{label}” {} times and learned it anyway.",
            conquest.lapses
        ),
        None => format!(
            "They forgot it {} times and learned it anyway.",
            conquest.lapses
        ),
    };
    let day = store.clock(&user).map_err(store_error)?.day(now);
    let friends = crate::social::friends(&store.conn, &user).map_err(store_error)?;
    let win =
        crate::wins::id_of(&store.conn, &user, &format!("conquered:{at}")).map_err(store_error)?;
    let tx = store.conn.transaction().map_err(store_error)?;
    for friend in &friends {
        if crate::social::muted(&tx, friend, &user).map_err(store_error)? {
            continue;
        }

        // A recipient already reached by a win delivery keeps that first send.
        // Do not adopt its row or marker; each route's Undo owns its own sends.
        if tx
            .query_row(
                "select exists (select 1 from win_deliveries where win = ?1 and recipient = ?2)",
                params![win, friend],
                |row| row.get::<_, bool>(0),
            )
            .map_err(store_error)?
        {
            continue;
        }

        // Once per friend, as `Store::send_once`, keeping the row so it can be taken back.
        if tx
            .execute(
                "insert or ignore into seen (user, key) values (?1, ?2)",
                params![friend, friend_key(&user, at)],
            )
            .map_err(store_error)?
            == 0
        {
            continue;
        }
        let id = crate::notifications::insert_notification(
            &tx,
            &Outgoing {
                to: friend,
                from: &user,
                title: &title,
                body: &body,
                kind: "conquest",
            },
            day,
            now,
            false,
        )
        .map_err(store_error)?;
        tx.execute(
            "insert or ignore into conquest_tell_rows (user, at, notification) values (?1, ?2, ?3)",
            params![user, at, id],
        )
        .map_err(store_error)?;
    }
    if tx
        .execute(
            "insert or ignore into seen (user, key) values (?1, ?2)",
            params![user, told_key(at)],
        )
        .map_err(store_error)?
        > 0
    {
        tx.execute(
            "insert or replace into conquest_tells (user, at, told_at) values (?1, ?2, ?3)",
            params![user, at, now],
        )
        .map_err(store_error)?;
    }
    tx.commit().map_err(store_error)?;
    Ok(Json(Told {
        told: friends.len(),
    }))
}

fn friend_key(user: &str, at: i64) -> String {
    format!("conquest:{user}:{at}")
}

/// Takes telling friends back, within [`UNDO_MS`] and until a friend cheered or answered it:
/// rows friends have not read yet leave their activity, and the owner can tell them again.
/// Rows already read stay, and those friends are not told twice. Asking again, or about a
/// card friends were never told of, takes nothing back; too late is `409`.
pub(crate) async fn untell(
    State(app): State<Arc<App>>,
    Path((user, at)): Path<(String, i64)>,
    headers: HeaderMap,
) -> Result<Json<Untold>, StatusCode> {
    require_owner(&app, &user, &headers)?;
    find(&app, &user, at).ok_or(StatusCode::NOT_FOUND)?;
    let mut store = app.store.lock().unwrap();
    if !store.is_seen(&user, &told_key(at)).map_err(store_error)? {
        return Ok(Json(Untold { retracted: 0 }));
    }
    if undo_until(&store.conn, &user, at, now_ms())
        .map_err(store_error)?
        .is_none()
    {
        return Err(StatusCode::CONFLICT);
    }
    let tx = store.conn.transaction().map_err(store_error)?;
    let unread: Vec<(i64, String)> = tx
        .prepare(
            "select n.id, n.recipient from conquest_tell_rows t join notifications n on n.id = t.notification
             where t.user = ?1 and t.at = ?2 and n.read_at is null",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![user, at], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect()
        })
        .map_err(store_error)?;
    for (id, recipient) in &unread {
        tx.execute("delete from notifications where id = ?1", [id])
            .map_err(store_error)?;
        tx.execute(
            "delete from seen where user = ?1 and key = ?2",
            params![recipient, friend_key(&user, at)],
        )
        .map_err(store_error)?;
    }
    for sql in [
        "delete from conquest_tell_rows where user = ?1 and at = ?2",
        "delete from conquest_tells where user = ?1 and at = ?2",
    ] {
        tx.execute(sql, params![user, at]).map_err(store_error)?;
    }
    tx.execute(
        "delete from seen where user = ?1 and key = ?2",
        params![user, told_key(at)],
    )
    .map_err(store_error)?;
    tx.commit().map_err(store_error)?;
    Ok(Json(Untold {
        retracted: unread.len(),
    }))
}

/// Says what the card was. Only its owner can, and only for a card they conquered.
pub(crate) async fn set_label(
    State(app): State<Arc<App>>,
    Path((user, at)): Path<(String, i64)>,
    headers: HeaderMap,
    Json(update): Json<LabelUpdate>,
) -> Result<StatusCode, StatusCode> {
    require_owner(&app, &user, &headers)?;
    let label = clean_label(&update.label).ok_or(StatusCode::BAD_REQUEST)?;
    find(&app, &user, at).ok_or(StatusCode::NOT_FOUND)?;
    app.store
        .lock()
        .unwrap()
        .conn
        .execute(
            "insert into conquest_labels (user, at, label) values (?1, ?2, ?3)
             on conflict (user, at) do update set label = excluded.label",
            params![user, at, label],
        )
        .map_err(store_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Takes the label back. Friends already told keep the message they received.
pub(crate) async fn remove_label(
    State(app): State<Arc<App>>,
    Path((user, at)): Path<(String, i64)>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    require_owner(&app, &user, &headers)?;
    app.store
        .lock()
        .unwrap()
        .conn
        .execute(
            "delete from conquest_labels where user = ?1 and at = ?2",
            params![user, at],
        )
        .map_err(store_error)?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/conquered/{user}/{at}", get(page))
        .route("/api/conquests/{user}/{at}", get(show))
        .route("/api/conquests/{user}/{at}/tell", post(tell).delete(untell))
        .route(
            "/api/conquests/{user}/{at}/label",
            post(set_label).delete(remove_label),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::study::upload;
    use crate::test_support::*;
    use crate::{conquests, privacy};

    /// A card forgotten four times that then earns a 30-day interval, ending a minute ago.
    fn hard_card(cid: i64) -> Vec<Review> {
        let mut answers = vec![(0, 0, 1)];
        for _ in 0..4 {
            answers.extend([(1, 3, -600), (2, -600, 1)]);
        }
        answers.push((1, 1, 30));
        let start = now_ms() - 60_000 - answers.len() as i64 * 1000;
        answers
            .iter()
            .enumerate()
            .map(|(i, &(kind, last_ivl, ivl))| Review {
                id: start + i as i64 * 1000,
                cid,
                last_ivl,
                ivl: Some(ivl),
                ease: None,
                time_ms: 8_000,
                kind,
            })
            .collect()
    }

    /// One real conquest and two friends, before the worker keeps its win.
    async fn conquest_sharing_case() -> (Arc<App>, PathBuf, i64) {
        let (app, path) = fixture();
        with_reviews(&app, "cerro", 1);
        with_reviews(&app, "friend", 1);
        upload_as(
            &app,
            "hill",
            plain_upload(reviews_at(now_ms() - 3_600_000, 2)),
        )
        .await;
        let rows = hard_card(77);
        let at = rows.last().unwrap().id;
        upload_as(&app, "hill", plain_upload(rows)).await;
        app.store
            .lock()
            .unwrap()
            .conn
            .execute_batch(
                "insert into friendships (a, b, requested_by, accepted, created_at)
             values ('cerro', 'hill', 'hill', 1, 0), ('friend', 'hill', 'hill', 1, 0);",
            )
            .unwrap();
        (app, path, at)
    }

    fn keep_conquest(app: &App, at: i64) {
        crate::wins::create(
            &app.store.lock().unwrap().conn,
            "hill",
            &format!("conquered:{at}"),
            0,
            at,
            "Card conquered",
            "",
            &serde_json::json!({}),
        )
        .unwrap()
        .unwrap();
    }

    async fn tell_conquest(app: &Arc<App>, at: i64, method: &str) -> serde_json::Value {
        let (status, response) = call(
            app,
            method,
            &format!("/api/conquests/hill/{at}/tell"),
            "hill-secret",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{response}");
        response
    }

    async fn share_conquest(app: &Arc<App>, at: i64, friends: &[&str]) -> serde_json::Value {
        let (status, response) = call(
            app,
            "POST",
            "/api/share/hill",
            "hill-secret",
            serde_json::json!({"moment": {"conquest": at}, "to": {"friends": friends}}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{response}");
        response
    }

    fn conquest_rows(app: &App, recipient: &str) -> i64 {
        app.store
            .lock()
            .unwrap()
            .conn
            .query_row(
                "select count(*) from notifications where sender = 'hill' and recipient = ?1
             and kind in ('conquest', 'share')",
                [recipient],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[tokio::test]
    async fn tell_then_share_is_once() {
        let (app, path, at) = conquest_sharing_case().await;
        // Released clients can tell friends before the worker creates the win.
        assert_eq!(tell_conquest(&app, at, "POST").await["told"], 2);
        keep_conquest(&app, at);
        let shared = share_conquest(&app, at, &["cerro", "friend"]).await;
        assert_eq!(
            shared["recipients"], 0,
            "Tell already delivered this conquest"
        );
        assert!(shared.get("undo_until").is_none());
        assert_eq!(tell_conquest(&app, at, "POST").await["told"], 2);
        assert_eq!(conquest_rows(&app, "cerro"), 1);
        assert_eq!(conquest_rows(&app, "friend"), 1);
        cleanup(app, path);
    }

    #[tokio::test]
    async fn share_then_tell_is_once() {
        let (app, path, at) = conquest_sharing_case().await;
        keep_conquest(&app, at);
        assert_eq!(share_conquest(&app, at, &["cerro"]).await["recipients"], 1);
        assert_eq!(tell_conquest(&app, at, "POST").await["told"], 2);
        assert_eq!(
            conquest_rows(&app, "cerro"),
            1,
            "Share already reached Cerro"
        );
        assert_eq!(
            conquest_rows(&app, "friend"),
            1,
            "Tell reaches the other friend"
        );
        assert_eq!(
            share_conquest(&app, at, &["cerro", "friend"]).await["recipients"],
            0
        );
        cleanup(app, path);
    }

    #[tokio::test]
    async fn conquest_undo_keeps_other_send() {
        let (app, path, at) = conquest_sharing_case().await;
        keep_conquest(&app, at);
        let first = share_conquest(&app, at, &["cerro"]).await;
        // Keep the actual receipt: each send is distinct without rewriting timestamps.
        let until = first["undo_until"].as_i64().unwrap();
        tell_conquest(&app, at, "POST").await;
        assert_eq!(tell_conquest(&app, at, "DELETE").await["retracted"], 1);
        assert_eq!(
            conquest_rows(&app, "cerro"),
            1,
            "Tell Undo leaves the chosen send"
        );
        assert_eq!(conquest_rows(&app, "friend"), 0);
        assert_eq!(share_conquest(&app, at, &["friend"]).await["recipients"], 1);
        let (status, result) = call(
            &app,
            "DELETE",
            &format!("/api/share/hill/{}?until={until}", first["win"]),
            "hill-secret",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(result["retracted"], 1);
        assert_eq!(conquest_rows(&app, "cerro"), 0);
        assert_eq!(
            conquest_rows(&app, "friend"),
            1,
            "Undo leaves the later send"
        );
        assert_eq!(share_conquest(&app, at, &["cerro"]).await["recipients"], 1);
        cleanup(app, path);
    }

    #[tokio::test]
    async fn read_tell_stays_shared() {
        let (app, path, at) = conquest_sharing_case().await;
        tell_conquest(&app, at, "POST").await;
        keep_conquest(&app, at);
        app.store
            .lock()
            .unwrap()
            .conn
            .execute(
                "update notifications set read_at = 1
             where recipient = 'cerro' and kind = 'conquest'",
                [],
            )
            .unwrap();
        assert_eq!(tell_conquest(&app, at, "DELETE").await["retracted"], 1);
        assert_eq!(
            share_conquest(&app, at, &["cerro", "friend"]).await["recipients"],
            1,
            "only the unread, retracted delivery can be sent again"
        );
        assert_eq!(conquest_rows(&app, "cerro"), 1);
        assert_eq!(conquest_rows(&app, "friend"), 1);
        cleanup(app, path);
    }

    #[tokio::test]
    async fn cheered_tell_stays_shared() {
        let (app, path, at) = conquest_sharing_case().await;
        tell_conquest(&app, at, "POST").await;
        keep_conquest(&app, at);
        app.store
            .lock()
            .unwrap()
            .conn
            .execute(
                "insert into reactions (reactor, notification, sender, created_at)
             select recipient, id, sender, 0 from notifications
             where recipient = 'cerro' and kind = 'conquest'",
                [],
            )
            .unwrap();
        let (status, _) = call(
            &app,
            "DELETE",
            &format!("/api/conquests/hill/{at}/tell"),
            "hill-secret",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(share_conquest(&app, at, &["cerro"]).await["recipients"], 0);
        assert_eq!(conquest_rows(&app, "cerro"), 1);
        cleanup(app, path);
    }

    #[tokio::test]
    async fn answered_tell_stays_shared() {
        let (app, path, at) = conquest_sharing_case().await;
        tell_conquest(&app, at, "POST").await;
        keep_conquest(&app, at);
        {
            let mut store = app.store.lock().unwrap();
            let row = store
                .notifications("cerro", now_ms())
                .unwrap()
                .into_iter()
                .find(|row| row.kind == "conquest")
                .unwrap();
            assert_eq!(
                store
                    .reply("cerro", "Cerro", row.id, "Well done", now_ms())
                    .unwrap()
                    .as_deref(),
                Some("hill")
            );
        }

        // An answered Tell cannot be undone or bypassed by the share sheet.
        let (status, _) = call(
            &app,
            "DELETE",
            &format!("/api/conquests/hill/{at}/tell"),
            "hill-secret",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(share_conquest(&app, at, &["cerro"]).await["recipients"], 0);
        assert_eq!(conquest_rows(&app, "cerro"), 1);
        assert_eq!(conquest_rows(&app, "friend"), 1);
        cleanup(app, path);
    }

    fn queue_conquest(app: &App, at: i64) {
        crate::wins::update_sharing(
            &app.store.lock().unwrap().conn,
            "hill",
            crate::wins::SharingUpdate {
                policies: [("conquered".to_string(), "auto".to_string())].into(),
                daily_cap: None,
            },
        )
        .unwrap()
        .expect("valid automatic policy");
        keep_conquest(app, at);
    }

    #[tokio::test]
    async fn tell_then_auto_is_once() {
        let (app, path, at) = conquest_sharing_case().await;
        assert_eq!(tell_conquest(&app, at, "POST").await["told"], 2);
        queue_conquest(&app, at);
        assert_eq!(
            crate::shares::share_automatically(
                &app,
                &mut app.store.lock().unwrap(),
                "hill",
                now_ms()
            )
            .unwrap(),
            0,
            "the automatic queue must respect the earlier Tell"
        );
        assert_eq!(conquest_rows(&app, "cerro"), 1);
        assert_eq!(conquest_rows(&app, "friend"), 1);
        cleanup(app, path);
    }

    #[tokio::test]
    async fn auto_then_tell_is_once() {
        let (app, path, at) = conquest_sharing_case().await;
        queue_conquest(&app, at);
        assert_eq!(
            crate::shares::share_automatically(
                &app,
                &mut app.store.lock().unwrap(),
                "hill",
                now_ms()
            )
            .unwrap(),
            2
        );
        assert_eq!(tell_conquest(&app, at, "POST").await["told"], 2);
        assert_eq!(tell_conquest(&app, at, "DELETE").await["retracted"], 0);
        assert_eq!(
            share_conquest(&app, at, &["cerro", "friend"]).await["recipients"],
            0,
            "Tell and its Undo must leave the automatic deliveries alone"
        );
        assert_eq!(conquest_rows(&app, "cerro"), 1);
        assert_eq!(conquest_rows(&app, "friend"), 1);
        cleanup(app, path);
    }

    #[tokio::test]
    async fn only_the_owner_labels_a_conquered_card_and_friends_hear_what_it_was() {
        let (app, path) = fixture();
        // Viewers of others' study have studied themselves; see crate::reciprocity.
        with_reviews(&app, "cerro", 1);
        upload_as(
            &app,
            "hill",
            plain_upload(reviews_at(now_ms() - 3_600_000, 2)),
        )
        .await;
        let rows = hard_card(78);
        let at = rows.last().unwrap().id;
        upload_as(&app, "hill", plain_upload(rows)).await;
        app.store
            .lock()
            .unwrap()
            .conn
            .execute("insert into friendships (a, b, requested_by, accepted, created_at) values ('cerro', 'hill', 'hill', 1, 0)", [])
            .unwrap();
        let set = |user: &str, at: i64, label: &str| {
            conquests::set_label(
                State(app.clone()),
                UrlPath(("hill".into(), at)),
                headers(user),
                Json(conquests::LabelUpdate {
                    label: label.into(),
                }),
            )
        };
        let show = || {
            conquests::show(
                State(app.clone()),
                UrlPath(("hill".into(), at)),
                headers("cerro"),
            )
        };
        assert_eq!(
            show().await.unwrap().0.label,
            None,
            "nothing is shown until chosen"
        );
        assert_eq!(
            set("cerro", at, "das Eichhörnchen").await.err(),
            Some(StatusCode::UNAUTHORIZED)
        );
        assert_eq!(
            set("hill", at, " \n\t ").await.err(),
            Some(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            set("hill", at, &"x".repeat(conquests::MAX_LABEL + 1))
                .await
                .err(),
            Some(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            set("hill", at + 1, "das Eichhörnchen").await.err(),
            Some(StatusCode::NOT_FOUND),
            "only a card that was conquered"
        );
        assert_eq!(
            set("hill", at, "  das\nEichhörnchen\u{7}  ").await.unwrap(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            show().await.unwrap().0.label.as_deref(),
            Some("das Eichhörnchen"),
            "one plain line"
        );

        let told = conquests::tell(
            State(app.clone()),
            UrlPath(("hill".into(), at)),
            headers("hill"),
        )
        .await
        .unwrap();
        assert_eq!(told.0.told, 1);
        let inbox = app
            .store
            .lock()
            .unwrap()
            .notifications("cerro", now_ms())
            .unwrap();
        let told = inbox.iter().find(|n| n.kind == "conquest").unwrap();
        assert_eq!(
            told.body,
            "They forgot “das Eichhörnchen” 4 times and learned it anyway."
        );

        let remove = |user: &str| {
            conquests::remove_label(
                State(app.clone()),
                UrlPath(("hill".into(), at)),
                headers(user),
            )
        };
        assert_eq!(remove("cerro").await.err(), Some(StatusCode::UNAUTHORIZED));
        assert_eq!(remove("hill").await.unwrap(), StatusCode::NO_CONTENT);
        assert_eq!(show().await.unwrap().0.label, None);

        set("hill", at, "der Igel").await.unwrap();
        let store = app.store.lock().unwrap();
        privacy::forget(&store.conn, "hill").unwrap();
        let left: i64 = store
            .conn
            .query_row(
                "select (select count(*) from conquest_labels)
                      + (select count(*) from seen where key like 'conquest:hill:%')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            left, 0,
            "deleting the account takes labels and friends' told-markers"
        );
        drop(store);
        cleanup(app, path);
    }

    #[tokio::test]
    async fn labels_reach_strangers_only_from_profiles_chosen_public() {
        let (app, path) = fixture();
        // Cerro stays a stranger to Ana; both viewers have studied, see crate::reciprocity.
        with_reviews(&app, "cerro", 1);
        with_reviews(&app, "hill", 1);
        crate::accounts::create(&app.store.lock().unwrap().conn, "ana", "Ana", "", 0).unwrap();
        app.accounts.insert("ana", "Ana");
        let token = app.issue_token("ana", "Phone").unwrap();
        let mut ana = HeaderMap::new();
        ana.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        let rows = hard_card(79);
        let at = rows.last().unwrap().id;
        let _ = upload(
            State(app.clone()),
            UrlPath("ana".into()),
            ana.clone(),
            Json(plain_upload(rows)),
        )
        .await
        .unwrap();
        conquests::set_label(
            State(app.clone()),
            UrlPath(("ana".into(), at)),
            ana.clone(),
            Json(conquests::LabelUpdate {
                label: "la ardilla".into(),
            }),
        )
        .await
        .unwrap();
        let label = |viewer: HeaderMap| async {
            conquests::show(State(app.clone()), UrlPath(("ana".into(), at)), viewer)
                .await
                .unwrap()
                .0
                .label
        };
        assert_eq!(
            label(headers("cerro")).await,
            None,
            "the conquest is public by default, what the card was is not"
        );
        assert_eq!(
            conquests::show(
                State(app.clone()),
                UrlPath(("ana".into(), at)),
                HeaderMap::new()
            )
            .await
            .err(),
            Some(StatusCode::NOT_FOUND),
            "visitors who are not signed in see no one's study"
        );
        assert_eq!(label(headers("hill")).await, None);
        assert_eq!(label(ana.clone()).await.as_deref(), Some("la ardilla"));
        app.store
            .lock()
            .unwrap()
            .conn
            .execute("insert into friendships (a, b, requested_by, accepted, created_at) values ('ana', 'hill', 'ana', 1, 0)", [])
            .unwrap();
        assert_eq!(label(headers("hill")).await.as_deref(), Some("la ardilla"));
        app.store
            .lock()
            .unwrap()
            .conn
            .execute(
                "insert into visibility (user, public) values ('ana', 1)",
                [],
            )
            .unwrap();
        assert_eq!(
            label(headers("cerro")).await.as_deref(),
            Some("la ardilla"),
            "a profile its player made public shares labels as before"
        );
        cleanup(app, path);
    }

    #[tokio::test]
    async fn a_conquered_card_is_announced_shared_and_told_to_friends_once() {
        let (app, path) = fixture();
        // Viewers of others' study have studied themselves; see crate::reciprocity.
        with_reviews(&app, "cerro", 1);
        upload_as(
            &app,
            "hill",
            plain_upload(reviews_at(now_ms() - 3_600_000, 2)),
        )
        .await;
        let rows = hard_card(77);
        let at = rows.last().unwrap().id;
        let response = upload_as(&app, "hill", plain_upload(rows.clone())).await;
        assert_eq!(response.profile.conquests.len(), 1);
        assert!(
            response
                .feedback
                .headlines
                .iter()
                .any(|h| *h == "🏆 Card conquered after 4 lapses"),
            "{:?}",
            response.feedback
        );
        let resent = upload_as(&app, "hill", plain_upload(rows)).await;
        assert!(
            !resent
                .feedback
                .headlines
                .iter()
                .any(|h| h.to_string().starts_with('🏆')),
            "resending history does not announce it again"
        );

        app.store
            .lock()
            .unwrap()
            .conn
            .execute("insert into friendships (a, b, requested_by, accepted, created_at) values ('cerro', 'hill', 'hill', 1, 0)", [])
            .unwrap();
        let show = |viewer: &str, at: i64| {
            conquests::show(
                State(app.clone()),
                UrlPath(("hill".into(), at)),
                headers(viewer),
            )
        };
        let mine = show("hill", at).await.unwrap().0;
        assert_eq!(
            (mine.conquest.lapses, mine.told, mine.friends),
            (4, Some(false), Some(1))
        );
        let theirs = show("cerro", at).await.unwrap().0;
        assert_eq!(
            (theirs.told, theirs.friends),
            (None, None),
            "only the owner sees sharing state"
        );
        assert_eq!(mine.win, None, "no win until the worker keeps one");
        let win = crate::wins::create(
            &app.store.lock().unwrap().conn,
            "hill",
            &format!("conquered:{at}"),
            0,
            now_ms(),
            "Card conquered",
            "",
            &serde_json::json!({}),
        )
        .unwrap();
        assert!(win.is_some());
        assert_eq!(show("hill", at).await.unwrap().0.win, win);
        assert_eq!(
            show("cerro", at).await.unwrap().0.win,
            None,
            "only the owner can share the win's card"
        );
        assert_eq!(
            show("hill", at + 1).await.err(),
            Some(StatusCode::NOT_FOUND)
        );

        let tell = |user: &str| {
            conquests::tell(
                State(app.clone()),
                UrlPath(("hill".into(), at)),
                headers(user),
            )
        };
        app.store
            .lock()
            .unwrap()
            .conn
            .execute_batch(
                "insert into friendships (a, b, requested_by, accepted, created_at) values ('friend', 'hill', 'hill', 1, 0);
                 insert into muted_people (recipient, sender, created_at) values ('friend', 'hill', 0);",
            )
            .unwrap();
        assert_eq!(tell("cerro").await.err(), Some(StatusCode::UNAUTHORIZED));
        assert_eq!(
            tell("hill").await.unwrap().0.told,
            2,
            "the count does not give a mute away"
        );
        assert_eq!(tell("hill").await.unwrap().0.told, 2);
        assert!(
            app.store
                .lock()
                .unwrap()
                .notifications("friend", now_ms())
                .unwrap()
                .is_empty(),
            "a friend who muted hill hears nothing"
        );
        let inbox = app
            .store
            .lock()
            .unwrap()
            .notifications("cerro", now_ms())
            .unwrap();
        let told: Vec<_> = inbox.iter().filter(|n| n.kind == "conquest").collect();
        assert_eq!(told.len(), 1, "telling twice does not repeat it: {inbox:?}");
        assert_eq!(told[0].title, "hill conquered a hard card");
        assert_eq!(show("hill", at).await.unwrap().0.told, Some(true));
        cleanup(app, path);
    }

    #[tokio::test]
    async fn an_old_conquest_gets_a_quiet_win_and_telling_friends_can_be_taken_back() {
        let (app, path) = fixture();
        // Viewers of others' study have studied themselves; see crate::reciprocity.
        with_reviews(&app, "cerro", 1);
        upload_as(
            &app,
            "hill",
            plain_upload(reviews_at(now_ms() - 3_600_000, 2)),
        )
        .await;
        let rows = hard_card(78);
        let at = rows.last().unwrap().id;
        upload_as(&app, "hill", plain_upload(rows)).await;
        // Announced before wins were kept: seen, with no win. Even an automatic policy
        // must not send it to friends now.
        app.store
            .lock()
            .unwrap()
            .conn
            .execute_batch(&format!(
                "insert or ignore into seen (user, key) values ('hill', 'conquered:{at}');
                 insert into sharing_settings (user, policies) values ('hill', '{{\"conquered\":\"auto\"}}');
                 insert into friendships (a, b, requested_by, accepted, created_at) values ('cerro', 'hill', 'hill', 1, 0);
                 insert into friendships (a, b, requested_by, accepted, created_at) values ('friend', 'hill', 'hill', 1, 0);"
            ))
            .unwrap();
        let show = |viewer: &str| {
            conquests::show(
                State(app.clone()),
                UrlPath(("hill".into(), at)),
                headers(viewer),
            )
        };
        let win = show("hill").await.unwrap().0.win.expect("a win kept now");
        assert_eq!(show("hill").await.unwrap().0.win, Some(win), "kept once");
        assert_eq!(show("cerro").await.unwrap().0.win, None);
        {
            let store = app.store.lock().unwrap();
            let kept = crate::wins::win(&store.conn, "hill", win).unwrap();
            assert_eq!(
                (kept.kind.as_str(), kept.created_at, kept.stats.clone()),
                ("conquered", at, serde_json::json!({}))
            );
            let queued: i64 = store
                .conn
                .query_row("select count(*) from win_auto_queue", [], |r| r.get(0))
                .unwrap();
            assert_eq!(queued, 0, "an old conquest is not sent to friends");
        }

        let tell = || {
            conquests::tell(
                State(app.clone()),
                UrlPath(("hill".into(), at)),
                headers("hill"),
            )
        };
        let untell = |user: &str| {
            conquests::untell(
                State(app.clone()),
                UrlPath(("hill".into(), at)),
                headers(user),
            )
        };
        let told = |user: &str| {
            app.store
                .lock()
                .unwrap()
                .notifications(user, now_ms())
                .unwrap()
                .into_iter()
                .filter(|n| n.kind == "conquest")
                .collect::<Vec<_>>()
        };
        assert_eq!(
            untell("hill").await.unwrap().0.retracted,
            0,
            "nothing told yet"
        );
        assert_eq!(show("hill").await.unwrap().0.undo_until, None);
        let before = now_ms();
        assert_eq!(tell().await.unwrap().0.told, 2);
        let until = show("hill")
            .await
            .unwrap()
            .0
            .undo_until
            .expect("undo offered");
        assert!(until >= before + conquests::UNDO_MS && until <= now_ms() + conquests::UNDO_MS);
        assert_eq!(
            show("cerro").await.unwrap().0.undo_until,
            None,
            "owner only"
        );
        // Friend reads it; Cerro has not yet.
        app.store
            .lock()
            .unwrap()
            .conn
            .execute(
                "update notifications set read_at = 1 where recipient = 'friend' and kind = 'conquest'",
                [],
            )
            .unwrap();
        assert_eq!(untell("cerro").await.err(), Some(StatusCode::UNAUTHORIZED));
        assert_eq!(untell("hill").await.unwrap().0.retracted, 1);
        assert!(told("cerro").is_empty(), "the unread row is gone");
        assert_eq!(told("friend").len(), 1, "a row already read stays");
        let shown = show("hill").await.unwrap().0;
        assert_eq!((shown.told, shown.undo_until), (Some(false), None));
        assert_eq!(untell("hill").await.unwrap().0.retracted, 0, "idempotent");

        assert_eq!(tell().await.unwrap().0.told, 2);
        assert_eq!(
            told("cerro").len(),
            1,
            "telling again reaches who was untold"
        );
        assert_eq!(told("friend").len(), 1, "and not who read it");
        let row = told("cerro")[0].id;
        app.store
            .lock()
            .unwrap()
            .conn
            .execute(
                "insert into reactions (reactor, notification, sender, created_at) values ('cerro', ?1, 'hill', 0)",
                [row],
            )
            .unwrap();
        assert_eq!(
            show("hill").await.unwrap().0.undo_until,
            None,
            "a cheer keeps it"
        );
        assert_eq!(untell("hill").await.err(), Some(StatusCode::CONFLICT));
        app.store
            .lock()
            .unwrap()
            .conn
            .execute_batch(&format!(
                "delete from reactions;
                 update conquest_tells set told_at = {} where user = 'hill'",
                now_ms() - conquests::UNDO_MS - 1
            ))
            .unwrap();
        assert_eq!(
            untell("hill").await.err(),
            Some(StatusCode::CONFLICT),
            "a day later"
        );
        assert_eq!(told("cerro").len(), 1);

        let store = app.store.lock().unwrap();
        privacy::forget(&store.conn, "hill").unwrap();
        let left: i64 = store
            .conn
            .query_row(
                "select (select count(*) from conquest_tells) + (select count(*) from conquest_tell_rows)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 0);
        drop(store);
        cleanup(app, path);
    }
}
