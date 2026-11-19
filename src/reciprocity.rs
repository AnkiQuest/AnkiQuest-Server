//! Players see other players' study only after uploading their own, as Anki Leaderboard does.
//!
//! "Uploaded" means the server holds at least one of the player's reviews. An upload with no
//! reviews does not count, so signing up and syncing an empty collection opens nothing. The
//! rule has no time window: a forged review is as easy to send this week as any other, so a
//! window would add friction without stopping anyone. Visitors who are not signed in see no
//! one, except through what a player published on purpose: a `/w/` link page. The shared
//! community password of a private site counts as uploaded: its operator chose to show
//! that community its board.

use crate::access;
use crate::app::App;
use crate::social::Viewer;
use axum::extract::{Request, State};
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;
use serde::Serialize;
use std::collections::BTreeSet;
use std::sync::Arc;

/// The response header that says why the caller sees no other players.
pub(crate) const HEADER: &str = "x-ankiquest-locked";

/// The capability that says this server keeps others' study from players who have not uploaded.
pub(crate) const FEATURE: &str = "upload_first";

/// Notification kinds that report another player's study: shared moments, finished decks and
/// conquered cards. Messages, nudges, invitations and reactions carry none.
pub(crate) const STUDY_NOTICES: &str = "'share','completion','conquest'";

/// Paths whose answers show other players' study, and the upload that opens them. Their
/// responses carry [`HEADER`] while the caller is locked.
const MARKED: &[&str] = &[
    "/api/leaderboard",
    "/api/records",
    "/api/rank/",
    "/api/profile/",
    "/api/preview/",
    "/api/activity/",
    "/api/notifications/",
    "/api/community",
    "/api/winners",
    "/api/conquests/",
    "/api/reviews/",
];

/// Why a caller sees no other players' study.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Lock {
    /// A signed-in player the server holds no review from yet.
    UploadFirst,
    /// Nobody is signed in.
    SignIn,
}

impl Lock {
    fn as_str(self) -> &'static str {
        match self {
            Lock::UploadFirst => "upload_first",
            Lock::SignIn => "sign_in",
        }
    }
}

impl App {
    /// Why `viewer` may not see other players' study, or None when they may.
    pub(crate) fn lock(&self, viewer: &Viewer) -> Option<Lock> {
        match &viewer.member {
            Some(member) => self.member_lock(member),
            None if viewer.community => None,
            None => Some(Lock::SignIn),
        }
    }

    /// Why the signed-in `member` may not see other players' study, or None when they may.
    pub(crate) fn member_lock(&self, member: &str) -> Option<Lock> {
        let players = self.players.read().unwrap();
        let uploaded = players
            .get(member)
            .is_some_and(|player| !player.reviews.is_empty());
        (!uploaded).then_some(Lock::UploadFirst)
    }

    /// Leaves only the viewer in `users` while the viewer may not see others.
    pub(crate) fn withhold(&self, viewer: &Viewer, users: &mut BTreeSet<String>) {
        if self.lock(viewer).is_none() {
            return;
        }
        users.retain(|user| viewer.member.as_deref() == Some(user.as_str()));
    }
}

/// Marks the answers of [`MARKED`] paths with [`HEADER`] while the caller is locked. It runs
/// after the handler, so the answer to the first upload already leaves the header out.
pub(crate) async fn mark(State(app): State<Arc<App>>, request: Request, next: Next) -> Response {
    let path = request.uri().path();
    if !MARKED.iter().any(|marked| path.starts_with(marked)) {
        return next.run(request).await;
    }
    let headers = request.headers().clone();
    let mut response = next.run(request).await;
    if let Some(lock) = app.lock(&access::viewer(&app, &headers)) {
        response
            .headers_mut()
            .insert(HEADER, HeaderValue::from_static(lock.as_str()));
    }
    response
}

#[cfg(test)]
mod tests {
    use crate::test_support::*;
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::{Value, json};

    const LOCKED: &str = super::HEADER;

    fn fixture() -> (Arc<App>, PathBuf) {
        app_with(json!({
            "registration": true,
            "configured_tokens": "enabled",
            "users": {"alice": {"display": "Alice", "token": "alice-token"}}
        }))
    }

    /// `method` on `path`, as `token` or as nobody, and the lock header, status and JSON.
    async fn ask(
        app: &Arc<App>,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Value,
    ) -> (Option<String>, StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let body = if body.is_null() {
            Body::empty()
        } else {
            Body::from(body.to_string())
        };
        let response = route(app, request.body(body).unwrap()).await;
        let locked = response
            .headers()
            .get(LOCKED)
            .map(|value| value.to_str().unwrap().to_string());
        let (status, json) = json_response(response).await;
        (locked, status, json)
    }

    async fn get(app: &Arc<App>, path: &str, token: Option<&str>) -> (Option<String>, Value) {
        let (locked, status, body) = ask(app, "GET", path, token, Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        (locked, body)
    }

    /// Uploads five reviews made a minute ago, or none at all.
    async fn study(app: &Arc<App>, user: &str, token: &str, count: i64) {
        let now = now_ms();
        let reviews: Vec<Value> = (0..count)
            .map(|i| json!({"id": now - 60_000 + i, "cid": 1_000 + i, "last_ivl": 1, "time_ms": 8_000, "kind": 1}))
            .collect();
        let body = json!({"reviews": reviews, "clock": {"offset_west_min": 0, "rollover_hour": 4}});
        let path = format!("/api/reviews/{user}");
        let (_, status, answer) = ask(app, "POST", &path, Some(token), body).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
    }

    fn users(rows: &Value) -> Vec<&str> {
        rows.as_array()
            .unwrap()
            .iter()
            .map(|row| row["user"].as_str().unwrap())
            .collect()
    }

    fn holders(records: &Value) -> Vec<&str> {
        records
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|record| users(&record["holders"]))
            .collect()
    }

    fn kinds(notices: &Value) -> Vec<&str> {
        notices
            .as_array()
            .unwrap()
            .iter()
            .map(|notice| notice["kind"].as_str().unwrap())
            .collect()
    }

    /// What `token`'s player sees of Alice, who studies, on every board, profile and feed.
    struct Seen {
        locked: Vec<Option<String>>,
        friends: Value,
        global: Value,
        records: Value,
        rank: Value,
        profile: StatusCode,
        activity: Value,
        notifications: Value,
    }

    async fn seen(app: &Arc<App>, user: &str, token: &str) -> Seen {
        let me = Some(token);
        let (a, friends) = get(app, "/api/leaderboard?scope=friends", me).await;
        let (b, global) = get(app, "/api/leaderboard?scope=global", me).await;
        let (c, records) = get(app, "/api/records?scope=global", me).await;
        let rank_path = format!("/api/rank/{user}");
        let (d, _, rank) = ask(app, "POST", &rank_path, me, json!({"previous": []})).await;
        let (e, profile, _) = ask(app, "GET", "/api/profile/alice", me, Value::Null).await;
        let (f, activity) = get(app, &format!("/api/activity/{user}"), me).await;
        let (g, notifications) = get(app, &format!("/api/notifications/{user}"), me).await;
        Seen {
            locked: vec![a, b, c, d, e, f, g],
            friends,
            global,
            records,
            rank,
            profile,
            activity,
            notifications,
        }
    }

    #[tokio::test]
    async fn a_player_sees_others_only_after_uploading_a_review() {
        let (app, path) = fixture();
        study(&app, "alice", "alice-token", 5).await;
        let ana = sign_up(&app, "ana").await;

        // Ana and Alice are friends, and Alice shared a moment and sent a message.
        for (user, token, other) in [
            ("ana", ana.as_str(), "alice"),
            ("alice", "alice-token", "ana"),
        ] {
            let body = json!({"action": "befriend", "user": other});
            let (_, status, answer) = ask(
                &app,
                "POST",
                &format!("/api/social/{user}"),
                Some(token),
                body,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{answer}");
        }
        app.store
            .lock()
            .unwrap()
            .conn
            .execute(
                "insert into notifications (recipient, sender, title, body, day, created_at, kind)
                 values ('ana', 'alice', 'Alice', '30-day streak', 1, ?1, 'share'),
                        ('ana', 'alice', 'Alice', 'Hello', 1, ?1, 'message')",
                [now_ms()],
            )
            .unwrap();

        // A fresh player, and one whose upload held no reviews, sees only themselves.
        for upload in [false, true] {
            if upload {
                study(&app, "ana", &ana, 0).await;
            }
            let seen = seen(&app, "ana", &ana).await;
            assert!(!users(&seen.friends).contains(&"alice"), "{}", seen.friends);
            assert!(!users(&seen.global).contains(&"alice"), "{}", seen.global);
            assert!(holders(&seen.records).is_empty(), "{}", seen.records);
            assert!(!users_in(&seen.rank["order"]).contains(&"alice"));
            assert_eq!(seen.profile, StatusCode::NOT_FOUND);
            assert_eq!(kinds(&seen.activity["items"]), ["message"]);
            assert_eq!(seen.activity["locked"], "upload_first");
            assert_eq!(kinds(&seen.notifications), ["message"]);
            for locked in &seen.locked {
                assert_eq!(locked.as_deref(), Some("upload_first"));
            }
        }

        // One real review opens everything.
        study(&app, "ana", &ana, 1).await;
        let seen = seen(&app, "ana", &ana).await;
        assert!(seen.locked.iter().all(Option::is_none), "{:?}", seen.locked);
        assert!(users(&seen.friends).contains(&"alice"));
        assert!(users(&seen.global).contains(&"alice"));
        assert!(holders(&seen.records).contains(&"alice"));
        assert!(users_in(&seen.rank["order"]).contains(&"alice"));
        assert_eq!(seen.profile, StatusCode::OK);
        assert_eq!(kinds(&seen.activity["items"]), ["message", "share"]);
        assert!(seen.activity.get("locked").is_none());
        assert_eq!(kinds(&seen.notifications), ["share", "message"]);
        cleanup(app, path);
    }

    fn users_in(order: &Value) -> Vec<&str> {
        order
            .as_array()
            .unwrap()
            .iter()
            .map(|user| user.as_str().unwrap())
            .collect()
    }

    #[tokio::test]
    async fn friend_goals_hide_other_members_progress_until_the_first_review() {
        let (app, path) = fixture();
        let ana = sign_up(&app, "ana").await;
        for (user, token, other) in [
            ("ana", ana.as_str(), "alice"),
            ("alice", "alice-token", "ana"),
        ] {
            let body = json!({"action": "befriend", "user": other});
            ask(
                &app,
                "POST",
                &format!("/api/social/{user}"),
                Some(token),
                body,
            )
            .await;
        }
        // Reviews count toward a goal from its start, so the clock moves on after creating it.
        let start = now_ms();
        let clock = FixedClock::at(start);
        let goal = json!({"title": "Together", "kind": "reviews", "cooperative": true,
            "target": 100, "duration_days": 7, "recipients": ["ana"]});
        let (_, status, answer) = ask(
            &app,
            "POST",
            "/api/community/challenges/alice",
            Some("alice-token"),
            goal,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        let id = answer["challenges"][0]["id"].as_i64().unwrap();
        let accept = json!({"action": "accept"});
        let path_accept = format!("/api/community/challenges/ana/{id}");
        let (_, status, answer) = ask(&app, "POST", &path_accept, Some(&ana), accept).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        clock.set(start + 120_000);
        study(&app, "alice", "alice-token", 5).await;

        let progress = |list: &Value| {
            let goal = &list["challenges"][0];
            let alice = goal["members"]
                .as_array()
                .unwrap()
                .iter()
                .find(|member| member["user"] == "alice")
                .unwrap()["progress"]
                .as_u64()
                .unwrap();
            (goal["progress"].as_u64().unwrap(), alice)
        };
        let (locked, list) = get(&app, "/api/community/challenges/ana", Some(&ana)).await;
        assert_eq!(progress(&list), (0, 0));
        assert_eq!(list["locked"], "upload_first");
        assert_eq!(locked.as_deref(), Some("upload_first"));

        study(&app, "ana", &ana, 1).await;
        let (locked, list) = get(&app, "/api/community/challenges/ana", Some(&ana)).await;
        assert_eq!(locked, None);
        assert!(list.get("locked").is_none());
        assert_eq!(progress(&list), (6, 5));
        cleanup(app, path);
    }

    #[tokio::test]
    async fn visitors_who_are_not_signed_in_see_no_ones_study() {
        let (app, path) = fixture();
        study(&app, "alice", "alice-token", 5).await;
        for board in ["/api/leaderboard", "/api/leaderboard?scope=global"] {
            let (locked, rows) = get(&app, board, None).await;
            assert_eq!(rows, json!([]), "{board}");
            assert_eq!(locked.as_deref(), Some("sign_in"));
        }
        let (locked, records) = get(&app, "/api/records", None).await;
        assert_eq!(locked.as_deref(), Some("sign_in"));
        assert!(holders(&records).is_empty(), "{records}");
        let (locked, status, _) = ask(&app, "GET", "/api/profile/alice", None, Value::Null).await;
        assert_eq!(locked.as_deref(), Some("sign_in"));
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (locked, status, rank) = ask(
            &app,
            "POST",
            "/api/rank/alice",
            None,
            json!({"previous": []}),
        )
        .await;
        assert_eq!(locked.as_deref(), Some("sign_in"));
        assert_eq!(status, StatusCode::OK);
        assert_eq!(rank["order"], json!([]));

        let (locked, community) = get(&app, "/api/community", None).await;
        assert_eq!(locked.as_deref(), Some("sign_in"));
        assert_eq!(community["locked"], "sign_in");
        assert_eq!(community["players"], json!([]));
        for kind in ["calendar", "weeks", "seasons"] {
            for period in community[kind].as_array().unwrap() {
                assert_eq!(period["standings"], json!([]), "{kind}");
                assert_eq!(period["winners"], json!([]), "{kind}");
            }
        }
        let (_, winners) = get(&app, "/api/winners", None).await;
        assert_eq!(winners["lifetime"]["players"], json!([]));

        // Alice herself still sees her own study and everyone in her community.
        let (locked, community) = get(&app, "/api/community", Some("alice-token")).await;
        assert_eq!(locked, None);
        assert_ne!(community["players"], json!([]));
        cleanup(app, path);
    }
}
