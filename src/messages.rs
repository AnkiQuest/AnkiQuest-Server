//! Messages between friends: a short text that lands in the friend's inbox as a `message` row,
//! which they answer with `POST /api/reply/{user}` like any other row.

use crate::access::require_owner;
use crate::app::{App, now_ms};
use crate::game::DAY_MS;
use crate::notifications::{Outgoing, insert_notification, message_title, withhold};
use crate::store::{Error, Store};
use crate::web_util::{failed_for, normalize_user};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Messages one player may send in any 24 hours, to all friends together.
const MAX_MESSAGES_PER_DAY: u32 = 30;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageRequest {
    to: String,
    message: String,
}

#[derive(Debug, Serialize)]
struct MessageResponse {
    sent_to: String,
}

/// What became of a message.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Delivered,
    /// The recipient muted the sender: the message is stored but withheld from them, so the
    /// sender sees it sent and counted like a delivered one.
    Muted,
    NotFriends,
    DailyLimit,
}

impl Outcome {
    fn status(&self) -> Result<(), StatusCode> {
        match self {
            Self::Delivered | Self::Muted => Ok(()),
            Self::NotFriends => Err(StatusCode::FORBIDDEN),
            Self::DailyLimit => Err(StatusCode::TOO_MANY_REQUESTS),
        }
    }
}

/// Delivers `text` from `sender` to their friend `to`, within the sender's daily limit.
fn deliver(
    store: &mut Store,
    sender: &str,
    display: &str,
    to: &str,
    text: &str,
    now: i64,
) -> Result<Outcome, Error> {
    let tx = store.conn.transaction()?;
    if !crate::social::friends(&tx, sender)?.contains(to) {
        return Ok(Outcome::NotFriends);
    }
    let sent: u32 = tx.query_row(
        "select count(*) from notifications
         where sender = ?1 and kind = 'message' and created_at > ?2",
        params![sender, now - DAY_MS],
        |r| r.get(0),
    )?;
    if sent >= MAX_MESSAGES_PER_DAY {
        return Ok(Outcome::DailyLimit);
    }

    // A `message` row with a sender is replyable, as messages sent from the command line are.
    let day = crate::store::clock_in(&tx, to)?.day(now);
    let id = insert_notification(
        &tx,
        &Outgoing {
            to,
            from: sender,
            title: &message_title(display),
            body: text,
            kind: "message",
        },
        day,
        now,
        false,
    )?;

    // A muted sender's message is kept only for their own sent list and daily count.
    let outcome = if crate::social::muted(&tx, to, sender)? {
        withhold(&tx, id)?;
        Outcome::Muted
    } else {
        Outcome::Delivered
    };
    tx.commit()?;
    Ok(outcome)
}

/// Sends a friend a message, so a conversation can start without a notification to answer.
async fn send(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
    Json(request): Json<MessageRequest>,
) -> Result<Json<MessageResponse>, StatusCode> {
    require_owner(&app, &user, &headers)?;
    let to = normalize_user(&request.to);
    if to == user || !app.is_user(&to) {
        return Err(StatusCode::NOT_FOUND);
    }
    let text = request.message.trim();
    if !crate::decks::valid_message(text) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let outcome = deliver(
        &mut app.store.lock().unwrap(),
        &user,
        &app.display(&user),
        &to,
        text,
        now_ms(),
    )
    .map_err(failed_for("message from", &user))?;
    outcome.status()?;
    Ok(Json(MessageResponse {
        sent_to: app.display(&to),
    }))
}

pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/messages/{user}", post(send))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{call, cleanup, sign_up};
    use serde_json::{Value, json};
    use std::path::PathBuf;

    /// Ana and Bobby are friends; Cyril is a stranger to both.
    async fn friends() -> (Arc<App>, PathBuf, String, String, String) {
        let (app, path) = crate::test_support::app_with(json!({"registration": true}));
        let ana = sign_up(&app, "ana").await;
        let bo = sign_up(&app, "bobby").await;
        let cy = sign_up(&app, "cyril").await;
        for (user, token, other) in [("ana", &ana, "bobby"), ("bobby", &bo, "ana")] {
            let (status, _) = call(
                &app,
                "POST",
                &format!("/api/social/{user}"),
                token,
                json!({"action": "befriend", "user": other}),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
        }
        (app, path, ana, bo, cy)
    }

    async fn message(app: &Arc<App>, from: &str, token: &str, to: &str, text: &str) -> StatusCode {
        call(
            app,
            "POST",
            &format!("/api/messages/{from}"),
            token,
            json!({"to": to, "message": text}),
        )
        .await
        .0
    }

    async fn messages(app: &Arc<App>, user: &str, token: &str) -> Vec<Value> {
        let (status, activity) = call(
            app,
            "GET",
            &format!("/api/activity/{user}?category=messages"),
            token,
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{activity}");
        activity["items"].as_array().unwrap().clone()
    }

    #[tokio::test]
    async fn friends_message_each_other_and_the_recipient_replies() {
        let (app, path, ana, bo, _) = friends().await;

        // A message reaches the friend's inbox, replyable, and the sender's sent list.
        let (status, sent) = call(
            &app,
            "POST",
            "/api/messages/ana",
            &ana,
            json!({"to": "bobby", "message": "  Hola, Bobby  "}),
        )
        .await;
        assert_eq!(
            (status, &sent),
            (StatusCode::OK, &json!({"sent_to": "bobby"}))
        );
        let inbox = messages(&app, "bobby", &bo).await;
        assert_eq!(inbox.len(), 1);
        let row = &inbox[0];
        assert_eq!(
            (&row["kind"], &row["sender"], &row["body"], &row["replied"]),
            (
                &json!("message"),
                &json!("ana"),
                &json!("Hola, Bobby"),
                &json!(false)
            )
        );
        let (_, outbox) = call(&app, "GET", "/api/activity/ana/sent", &ana, Value::Null).await;
        assert_eq!(outbox["items"][0]["recipient"], "bobby");
        assert_eq!(outbox["items"][0]["body"], "Hola, Bobby");

        // Bobby answers it with the reply endpoint; Ana sees what he answered.
        let (status, replied) = call(
            &app,
            "POST",
            "/api/reply/bobby",
            &bo,
            json!({"notification": row["id"], "message": "Hi Ana"}),
        )
        .await;
        assert_eq!(
            (status, replied["sent_to"].as_str()),
            (StatusCode::OK, Some("ana"))
        );
        let answer = messages(&app, "ana", &ana).await;
        assert_eq!(answer[0]["kind"], "reply");
        assert_eq!(answer[0]["reply_to"]["body"], "Hola, Bobby");
        cleanup(app, path);
    }

    #[tokio::test]
    async fn only_friends_get_messages_within_the_length_cap() {
        let (app, path, ana, _, cy) = friends().await;
        assert_eq!(
            message(&app, "ana", &ana, "cyril", "Hi").await,
            StatusCode::FORBIDDEN,
            "strangers cannot be messaged"
        );
        assert_eq!(messages(&app, "cyril", &cy).await.len(), 0);
        assert_eq!(
            message(&app, "ana", &ana, "nobody", "Hi").await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            message(&app, "ana", &ana, "ana", "Hi").await,
            StatusCode::NOT_FOUND
        );
        for text in [
            "",
            "   ",
            "two\nlines",
            &"x".repeat(crate::decks::MAX_MESSAGE + 1),
        ] {
            assert_eq!(
                message(&app, "ana", &ana, "bobby", text).await,
                StatusCode::BAD_REQUEST,
                "{text:?}"
            );
        }
        assert_eq!(
            message(&app, "ana", &cy, "bobby", "Hi").await,
            StatusCode::UNAUTHORIZED,
            "only the sender sends as themselves"
        );
        cleanup(app, path);
    }

    #[tokio::test]
    async fn muting_stops_messages_and_replies_without_telling_the_sender() {
        let (app, path, ana, bo, _) = friends().await;
        assert_eq!(
            message(&app, "bobby", &bo, "ana", "Before").await,
            StatusCode::OK
        );
        let before = messages(&app, "ana", &ana).await[0]["id"].clone();
        let (status, _) = call(
            &app,
            "POST",
            "/api/social/bobby",
            &bo,
            json!({"action": "mute", "user": "ana"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Neither a new message nor a reply to his older one reaches Bobby.
        assert_eq!(
            message(&app, "ana", &ana, "bobby", "Hi").await,
            StatusCode::OK
        );
        let (status, _) = call(
            &app,
            "POST",
            "/api/reply/ana",
            &ana,
            json!({"notification": before, "message": "Reply"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(messages(&app, "bobby", &bo).await.len(), 0);
        let (_, unread) = call(&app, "GET", "/api/activity/bobby", &bo, Value::Null).await;
        assert_eq!(unread["unread_count"], 0);
        assert!(
            app.store
                .lock()
                .unwrap()
                .take_deck_deliveries(now_ms())
                .unwrap()
                .iter()
                .all(|delivery| delivery.user != "bobby"),
            "nothing is pushed to Bobby"
        );

        // Ana's sent list shows both like any other, so she cannot tell she was muted.
        let (_, outbox) = call(&app, "GET", "/api/activity/ana/sent", &ana, Value::Null).await;
        let sent: Vec<_> = outbox["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    row["kind"].clone(),
                    row["body"].clone(),
                    row["read_at"].clone(),
                )
            })
            .collect();
        assert_eq!(
            sent,
            [
                (json!("reply"), json!("Reply"), Value::Null),
                (json!("message"), json!("Hi"), Value::Null),
            ]
        );
        cleanup(app, path);
    }

    #[tokio::test]
    async fn messages_to_a_player_who_muted_the_sender_count_toward_the_limit() {
        let (app, path, ana, bo, _) = friends().await;
        let (status, _) = call(
            &app,
            "POST",
            "/api/social/bobby",
            &bo,
            json!({"action": "mute", "user": "ana"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        for _ in 0..MAX_MESSAGES_PER_DAY {
            assert_eq!(
                message(&app, "ana", &ana, "bobby", "Hi").await,
                StatusCode::OK
            );
        }
        assert_eq!(
            message(&app, "ana", &ana, "bobby", "One more").await,
            StatusCode::TOO_MANY_REQUESTS,
            "the limit is the same as for a delivered message"
        );
        cleanup(app, path);
    }

    #[tokio::test]
    async fn a_sender_has_a_daily_message_limit() {
        let (app, path, ana, bo, _) = friends().await;
        for _ in 0..MAX_MESSAGES_PER_DAY {
            assert_eq!(
                message(&app, "ana", &ana, "bobby", "Hi").await,
                StatusCode::OK
            );
        }
        assert_eq!(
            message(&app, "ana", &ana, "bobby", "One more").await,
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            message(&app, "bobby", &bo, "ana", "Hi").await,
            StatusCode::OK,
            "the limit is the sender's own"
        );

        // A day later the sender may write again.
        app.store
            .lock()
            .unwrap()
            .conn
            .execute(
                "update notifications set created_at = created_at - ?1 where sender = 'ana'",
                [crate::game::DAY_MS],
            )
            .unwrap();
        assert_eq!(
            message(&app, "ana", &ana, "bobby", "Hi").await,
            StatusCode::OK
        );
        cleanup(app, path);
    }
}
