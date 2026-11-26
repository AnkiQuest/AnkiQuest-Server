use crate::access::require_owner;
use crate::notifications::Outgoing;
use crate::store::{Error, Store};
use crate::{app::App, app::now_ms, web_util::store_error};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Debug, Serialize)]
struct Friend {
    user: String,
    display: String,
    enabled: bool,
    sent_today: bool,
    muted_by_me: bool,
}

#[derive(Debug, Serialize)]
struct Settings {
    receiving: bool,
    automatic_receiving: bool,
    friends: Vec<Friend>,
}

fn key(sender: &str, day: i64) -> String {
    format!("friend-nudge:{day}:{sender}")
}

fn sent(store: &Store, sender: &str, recipient: &str, day: i64) -> Result<bool, Error> {
    store.is_seen(recipient, &key(sender, day))
}

/// The three nudge choices for each of `friends`, in order — whether they are enabled to nudge
/// `sender`, whether `sender` nudged them this `day`, and whether `sender` muted them — read
/// with one statement per table instead of one per friend.
fn settings_for(
    store: &Store,
    sender: &str,
    friends: &[String],
    day: i64,
) -> Result<Vec<(bool, bool, bool)>, Error> {
    if friends.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = friends.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
    let receiving: HashSet<String> = {
        let sql = format!(
            "select user from player_settings where friend_nudges and user in ({placeholders})"
        );
        let mut statement = store.conn.prepare(&sql)?;
        statement
            .query_map(rusqlite::params_from_iter(friends), |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<HashSet<_>, _>>()?
    };
    let blocked: HashSet<String> = {
        let mut statement = store
            .conn
            .prepare("select recipient from friend_nudge_muted_senders where sender = ?1")?;
        statement
            .query_map([sender], |row| row.get::<_, String>(0))?
            .collect::<Result<HashSet<_>, _>>()?
    };
    let muted: HashSet<String> = {
        let mut statement = store
            .conn
            .prepare("select recipient from muted_people where sender = ?1")?;
        statement
            .query_map([sender], |row| row.get::<_, String>(0))?
            .collect::<Result<HashSet<_>, _>>()?
    };
    let muted_by_me: HashSet<String> = {
        let mut statement = store
            .conn
            .prepare("select sender from friend_nudge_muted_senders where recipient = ?1")?;
        statement
            .query_map([sender], |row| row.get::<_, String>(0))?
            .collect::<Result<HashSet<_>, _>>()?
    };
    let sent_today: HashSet<String> = {
        let mut statement = store.conn.prepare("select user from seen where key = ?1")?;
        statement
            .query_map([key(sender, day).as_str()], |row| row.get::<_, String>(0))?
            .collect::<Result<HashSet<_>, _>>()?
    };
    Ok(friends
        .iter()
        .map(|friend| {
            (
                receiving.contains(friend) && !blocked.contains(friend) && !muted.contains(friend),
                sent_today.contains(friend),
                muted_by_me.contains(friend),
            )
        })
        .collect())
}

fn send(
    store: &mut Store,
    sender: &str,
    display: &str,
    recipient: &str,
    now: i64,
) -> Result<StatusCode, Error> {
    if !store.friend_nudges_enabled(recipient, sender)? {
        return Ok(StatusCode::FORBIDDEN);
    }
    let day = store.clock(sender)?.day(now);
    if sent(store, sender, recipient, day)? {
        return Ok(StatusCode::CONFLICT);
    }
    // Called under the App store lock; the limit and delivery commit together.
    store.send_once(
        &Outgoing {
            to: recipient,
            from: sender,
            title: "Time for Anki?",
            body: &format!("{display} is cheering you on. Make a little time for Anki today!"),
            kind: "nudge",
        },
        &key(sender, day),
        day,
        now,
        false,
    )?;
    Ok(StatusCode::OK)
}

async fn settings(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Settings>, StatusCode> {
    require_owner(&app, &user, &headers)?;
    let store = app.store.lock().unwrap();
    let day = store.clock(&user).map_err(store_error)?.day(now_ms());
    let recipients = crate::shares::friends(&app, &store.conn, &user).map_err(store_error)?;
    let choices = settings_for(&store, &user, &recipients, day).map_err(store_error)?;
    let mut friends = Vec::new();
    for (recipient, (enabled, sent_today, muted_by_me)) in recipients.iter().zip(choices) {
        friends.push(Friend {
            display: app.display(recipient),
            enabled,
            sent_today,
            muted_by_me,
            user: recipient.clone(),
        });
    }
    friends.sort_by(|a, b| a.display.cmp(&b.display).then(a.user.cmp(&b.user)));
    Ok(Json(Settings {
        receiving: store.friend_nudges_receiving(&user).map_err(store_error)?,
        automatic_receiving: store.nudges_enabled(&user).map_err(store_error)?,
        friends,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    recipient: String,
}

async fn nudge(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
    Json(request): Json<Request>,
) -> Result<StatusCode, StatusCode> {
    require_owner(&app, &user, &headers)?;
    let mut store = app.store.lock().unwrap();
    if request.recipient == user
        || !app.is_user(&request.recipient)
        || !app
            .connected(&store.conn, &user, &request.recipient)
            .map_err(store_error)?
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let result = send(
        &mut store,
        &user,
        &app.display(&user),
        &request.recipient,
        now_ms(),
    )
    .map_err(store_error)?;
    if result != StatusCode::OK {
        return Err(result);
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Receiving {
    enabled: bool,
}

async fn receiving(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
    Json(request): Json<Receiving>,
) -> Result<StatusCode, StatusCode> {
    require_owner(&app, &user, &headers)?;
    app.store
        .lock()
        .unwrap()
        .set_friend_nudges(&user, request.enabled)
        .map_err(store_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn automatic_receiving(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
    Json(request): Json<Receiving>,
) -> Result<StatusCode, StatusCode> {
    require_owner(&app, &user, &headers)?;
    app.store
        .lock()
        .unwrap()
        .set_nudges(&user, request.enabled)
        .map_err(store_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn sender_receiving(
    State(app): State<Arc<App>>,
    Path((user, sender)): Path<(String, String)>,
    headers: HeaderMap,
    Json(request): Json<Receiving>,
) -> Result<StatusCode, StatusCode> {
    require_owner(&app, &user, &headers)?;
    if sender == user || !app.is_user(&sender) {
        return Err(StatusCode::BAD_REQUEST);
    }
    app.store
        .lock()
        .unwrap()
        .set_friend_nudge_muted(&user, &sender, !request.enabled)
        .map_err(store_error)?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/friend-nudges/{user}", get(settings).post(nudge))
        .route("/api/friend-nudges/{user}/receiving", post(receiving))
        .route(
            "/api/friend-nudges/{user}/automatic",
            post(automatic_receiving),
        )
        .route(
            "/api/friend-nudges/{user}/senders/{sender}",
            post(sender_receiving),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::cleanup;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;

    use tower::ServiceExt;

    #[tokio::test]
    async fn sender_routes_require_owner_auth_and_reject_self_or_unknown_recipients() {
        let (app, path) =
            crate::test_support::app_with(serde_json::json!({"configured_tokens": "enabled",
                "users": {
                "cerro":{"token":"cerro-secret"},"hill":{"token":"hill-secret"}
            }}));
        for (owner, token, recipient, expected) in [
            ("cerro", "", "hill", StatusCode::UNAUTHORIZED),
            ("cerro", "hill-secret", "hill", StatusCode::UNAUTHORIZED),
            ("cerro", "cerro-secret", "cerro", StatusCode::BAD_REQUEST),
            ("cerro", "cerro-secret", "missing", StatusCode::BAD_REQUEST),
            ("cerro", "cerro-secret", "hill", StatusCode::FORBIDDEN),
        ] {
            let response = routes()
                .with_state(app.clone())
                .oneshot(
                    HttpRequest::post(format!("/api/friend-nudges/{owner}"))
                        .header("Authorization", format!("Bearer {token}"))
                        .header("Content-Type", "application/json")
                        .body(Body::from(
                            serde_json::json!({"recipient":recipient}).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }
        app.store
            .lock()
            .unwrap()
            .set_friend_nudges("hill", true)
            .unwrap();
        for expected in [StatusCode::NO_CONTENT, StatusCode::CONFLICT] {
            let response = routes()
                .with_state(app.clone())
                .oneshot(
                    HttpRequest::post("/api/friend-nudges/cerro")
                        .header("Authorization", "Bearer cerro-secret")
                        .header("Content-Type", "application/json")
                        .body(Body::from(r#"{"recipient":"hill"}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }
        cleanup(app, path);
    }

    #[tokio::test]
    async fn changing_friend_nudges_does_not_disable_automatic_progress_nudges() {
        let (mut store, path) = crate::test_support::temporary_store();
        store.set_nudges("hill", true).unwrap();
        let config = serde_json::from_value(serde_json::json!({"configured_tokens": "enabled",
            "users": {
            "cerro":{"token":"cerro-secret"},"hill":{"token":"hill-secret"}
        }}))
        .unwrap();
        let app = Arc::new(crate::test_support::app_from(store, config));
        let response = routes()
            .with_state(app.clone())
            .oneshot(
                HttpRequest::post("/api/friend-nudges/hill/receiving")
                    .header("Authorization", "Bearer hill-secret")
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{"enabled":false}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(
            app.store.lock().unwrap().nudges_enabled("hill").unwrap(),
            "automatic progress reminders remain on when friend nudges are disabled"
        );
        cleanup(app, path);
    }

    #[tokio::test]
    async fn only_the_recipient_can_change_a_specific_friend_nudge_choice() {
        let (app, path) =
            crate::test_support::app_with(serde_json::json!({"configured_tokens": "enabled",
                "users": {
                "cerro":{"token":"cerro-secret"},"hill":{"token":"hill-secret"}
            }}));
        for (owner, sender, token, expected) in [
            ("hill", "cerro", "cerro-secret", StatusCode::UNAUTHORIZED),
            ("hill", "hill", "hill-secret", StatusCode::BAD_REQUEST),
            ("hill", "missing", "hill-secret", StatusCode::BAD_REQUEST),
            ("hill", "cerro", "hill-secret", StatusCode::NO_CONTENT),
        ] {
            let response = routes()
                .with_state(app.clone())
                .oneshot(
                    HttpRequest::post(format!("/api/friend-nudges/{owner}/senders/{sender}"))
                        .header("Authorization", format!("Bearer {token}"))
                        .header("Content-Type", "application/json")
                        .body(Body::from(r#"{"enabled":false}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }
        assert!(
            app.store
                .lock()
                .unwrap()
                .friend_nudge_muted("hill", "cerro")
                .unwrap()
        );
        cleanup(app, path);
    }

    #[test]
    fn nudges_require_opt_in_and_limit_each_friend_until_the_next_anki_day() {
        let (mut store, path) = crate::test_support::temporary_store();
        let now = 1_800_000_000_000;
        assert_eq!(
            send(&mut store, "cerro", "Cerro", "hill", now).unwrap(),
            StatusCode::FORBIDDEN
        );
        assert!(
            !sent(
                &store,
                "cerro",
                "hill",
                store.clock("cerro").unwrap().day(now)
            )
            .unwrap()
        );
        store.set_friend_nudges("hill", true).unwrap();
        store.set_friend_nudges("friend", true).unwrap();
        assert_eq!(
            send(&mut store, "cerro", "Cerro", "hill", now).unwrap(),
            StatusCode::OK
        );
        assert_eq!(
            send(&mut store, "cerro", "Cerro", "hill", now + 1000).unwrap(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            send(&mut store, "cerro", "Cerro", "friend", now).unwrap(),
            StatusCode::OK
        );
        assert_eq!(
            send(&mut store, "friend", "Friend", "hill", now).unwrap(),
            StatusCode::OK
        );
        let inbox = store.notifications("hill", now).unwrap();
        assert_eq!(inbox.len(), 2);
        assert_eq!(inbox[0].kind, "nudge");
        assert!(!inbox[0].sender.is_empty());
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert_eq!(
            send(&mut store, "cerro", "Cerro", "hill", now).unwrap(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            send(&mut store, "cerro", "Cerro", "hill", now + 86_400_000).unwrap(),
            StatusCode::OK
        );
        cleanup(store, path);
    }

    #[test]
    fn muting_one_friend_preserves_other_friends_and_automatic_nudges() {
        let (mut store, path) = crate::test_support::temporary_store();
        let now = 1_800_000_000_000;
        store.set_nudges("hill", true).unwrap();
        store.set_friend_nudges("hill", true).unwrap();
        assert_eq!(
            send(&mut store, "cerro", "Cerro", "hill", now).unwrap(),
            StatusCode::OK
        );
        let pending: i64 = store.conn.query_row(
            "select id from notifications where recipient = 'hill' and sender = 'cerro' and kind = 'nudge'",
            [],
            |row| row.get(0),
        ).unwrap();
        store.set_friend_nudge_muted("hill", "cerro", true).unwrap();
        assert!(!store.begin_push(pending, now).unwrap());
        assert_eq!(
            send(&mut store, "cerro", "Cerro", "hill", now + 86_400_000).unwrap(),
            StatusCode::FORBIDDEN,
        );
        assert_eq!(
            send(&mut store, "friend", "Friend", "hill", now).unwrap(),
            StatusCode::OK
        );
        assert!(store.nudges_enabled("hill").unwrap());
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert!(store.friend_nudge_muted("hill", "cerro").unwrap());
        store
            .set_friend_nudge_muted("hill", "cerro", false)
            .unwrap();
        assert_eq!(
            send(&mut store, "cerro", "Cerro", "hill", now + 86_400_000).unwrap(),
            StatusCode::OK,
        );
        cleanup(store, path);
    }

    #[test]
    fn nudges_for_friends_answers_what_the_single_queries_do() {
        let (mut store, path) = crate::test_support::temporary_store();
        let day = 20_000;
        store.set_friend_nudges("ana", true).unwrap();
        store.set_friend_nudges("bobby", false).unwrap();
        store.set_friend_nudges("cerro", true).unwrap();
        store.set_friend_nudges("hill", true).unwrap();
        // bobby and cerro no longer take nudges from hill; hill muted bobby.
        store.set_friend_nudge_muted("bobby", "hill", true).unwrap();
        store.set_friend_nudge_muted("cerro", "hill", true).unwrap();
        store.set_friend_nudge_muted("hill", "bobby", true).unwrap();
        store
            .conn
            .execute(
                "insert or ignore into muted_people (recipient, sender, created_at) values (?1, ?2, ?3)",
                rusqlite::params!["cerro", "hill", 0],
            )
            .unwrap();
        store.mark_seen("ana", &key("hill", day)).unwrap();

        let friends = vec!["ana".to_string(), "bobby".to_string(), "cerro".to_string()];
        let batched = settings_for(&store, "hill", &friends, day).unwrap();
        for (friend, batched) in friends.iter().zip(batched) {
            let single = (
                store.friend_nudges_enabled(friend, "hill").unwrap(),
                sent(&store, "hill", friend, day).unwrap(),
                store.friend_nudge_muted("hill", friend).unwrap(),
            );
            assert_eq!(batched, single, "{friend}");
        }
        assert!(
            settings_for(&store, "hill", &[], day).unwrap().is_empty(),
            "no friends answers nothing"
        );
        let config: crate::config::Config = serde_json::from_value(serde_json::json!({})).unwrap();
        cleanup(crate::test_support::app_from(store, config), path);
    }
}
