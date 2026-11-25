use super::*;
use crate::test_support::{cleanup, fixture, route};
use axum::body::Body;
use axum::http::Request;
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use std::io::Cursor;

/// A pack zip for companion `id`: every core pose drawn from one picture of `colour`.
pub(crate) fn sample_pack(id: &str, colour: u8) -> Vec<u8> {
    let image = RgbaImage::from_pixel(16, 16, Rgba([colour, 90, 160, 255]));
    let mut picture = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut picture, ImageFormat::Png)
        .unwrap();
    let poses: BTreeMap<&str, &str> = poses()
        .of(ankiquest_art::Kind::CoreMood)
        .into_iter()
        .map(|pose| (pose, "face.png"))
        .collect();
    let index = json!({
        "format": 1, "id": id, "name": "Mochi", "version": 1,
        "licence": "CC-BY-4.0", "credit": "Cerro", "poses": poses
    });
    let files = BTreeMap::from([
        ("companion.json".to_string(), index.to_string().into_bytes()),
        ("face.png".to_string(), picture.into_inner()),
    ]);
    ankiquest_packs::stored(&files)
}

/// The status, headers and body of `method` on `path` as `user`, with a raw `body`.
async fn send(
    app: &Arc<App>,
    method: &str,
    path: &str,
    user: &str,
    body: Vec<u8>,
    extra: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {user}-secret"));
    for (name, value) in extra {
        request = request.header(*name, *value);
    }
    let response = route(app, request.body(Body::from(body)).unwrap()).await;
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 64 << 20).await.unwrap();
    (status, headers, bytes.to_vec())
}

async fn json_of(
    app: &Arc<App>,
    method: &str,
    path: &str,
    user: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let body = if body.is_null() {
        Vec::new()
    } else {
        body.to_string().into_bytes()
    };
    let (status, _, bytes) = send(
        app,
        method,
        path,
        user,
        body,
        &[("content-type", "application/json")],
    )
    .await;
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

async fn upload_as(app: &Arc<App>, user: &str, pack: Vec<u8>) -> (StatusCode, serde_json::Value) {
    let (status, _, bytes) =
        send(app, "POST", &format!("/api/packs/{user}"), user, pack, &[]).await;
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

fn befriend(app: &App, a: &str, b: &str) {
    let (a, b) = crate::social::pair(a, b);
    app.store
        .lock()
        .unwrap()
        .conn
        .execute(
            "insert into friendships (a, b, requested_by, accepted, created_at)
             values (?1, ?2, ?1, 1, 0)",
            params![a, b],
        )
        .unwrap();
}

#[tokio::test]
async fn a_friend_sees_and_fetches_only_the_pack_shown_to_friends() {
    let (app, path) = fixture();
    befriend(&app, "hill", "cerro");

    // Uploaded packs start visible to their owner only.
    let (status, pack) = upload_as(&app, "cerro", sample_pack("mochi", 10)).await;
    assert_eq!(status, StatusCode::OK, "{pack}");
    assert_eq!(pack["visibility"], "owner");
    assert_eq!(pack["id"], "mochi");
    let sha = pack["sha256"].as_str().unwrap().to_string();
    let file = format!("/api/packs/hill/file/{sha}");
    let (_, friends) = json_of(&app, "GET", "/api/packs/hill/friends", "hill", json!(null)).await;
    assert_eq!(friends, json!({"friends": {}}));
    assert_eq!(
        send(&app, "GET", &file, "hill", vec![], &[]).await.0,
        StatusCode::NOT_FOUND
    );

    // Shown to friends: listed and served by hash to friends, not to anyone else.
    let change = format!("/api/packs/cerro/{sha}");
    let (status, shown) = json_of(
        &app,
        "PUT",
        &change,
        "cerro",
        json!({"visibility": "friends"}),
    )
    .await;
    assert_eq!(
        (status, &shown["visibility"]),
        (StatusCode::OK, &json!("friends"))
    );
    let (_, friends) = json_of(&app, "GET", "/api/packs/hill/friends", "hill", json!(null)).await;
    assert_eq!(friends["friends"]["cerro"]["sha256"], sha);
    assert!(friends["friends"]["cerro"].get("visibility").is_none());
    let (status, headers, zip) = send(&app, "GET", &file, "hill", vec![], &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ankiquest_packs::sha256(&zip), sha);
    assert_eq!(headers[header::CONTENT_TYPE], "application/zip");
    // Clients keep packs by hash themselves; shared caches never keep data answers.
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    let etag = headers[header::ETAG].to_str().unwrap().to_string();
    let again = send(
        &app,
        "GET",
        &file,
        "hill",
        vec![],
        &[("if-none-match", &etag)],
    )
    .await;
    assert_eq!(again.0, StatusCode::NOT_MODIFIED);
    let stranger = format!("/api/packs/friend/file/{sha}");
    assert_eq!(
        send(&app, "GET", &stranger, "friend", vec![], &[]).await.0,
        StatusCode::NOT_FOUND
    );
    let (_, none) = json_of(
        &app,
        "GET",
        "/api/packs/friend/friends",
        "friend",
        json!(null),
    )
    .await;
    assert_eq!(none, json!({"friends": {}}));

    // Back to the owner only: gone for friends at once.
    json_of(
        &app,
        "PUT",
        &change,
        "cerro",
        json!({"visibility": "owner"}),
    )
    .await;
    assert_eq!(
        send(&app, "GET", &file, "hill", vec![], &[]).await.0,
        StatusCode::NOT_FOUND
    );
    let own = format!("/api/packs/cerro/file/{sha}");
    assert_eq!(
        send(&app, "GET", &own, "cerro", vec![], &[]).await.0,
        StatusCode::OK
    );
    cleanup(app, path);
}

#[tokio::test]
async fn every_request_needs_the_players_own_token() {
    let (app, path) = fixture();
    let sha = "0".repeat(64);
    for (method, uri) in [
        ("GET", "/api/packs/cerro".to_string()),
        ("POST", "/api/packs/cerro".to_string()),
        ("GET", "/api/packs/cerro/friends".to_string()),
        ("GET", format!("/api/packs/cerro/file/{sha}")),
        ("PUT", format!("/api/packs/cerro/{sha}")),
        ("DELETE", format!("/api/packs/cerro/{sha}")),
        ("POST", format!("/api/packs/cerro/reports/{sha}")),
        ("DELETE", format!("/api/packs/cerro/reports/{sha}")),
    ] {
        let body = json!({"visibility": "friends", "reason": "other"})
            .to_string()
            .into_bytes();
        let (status, _, _) = send(
            &app,
            method,
            &uri,
            "hill",
            body,
            &[("content-type", "application/json")],
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
    cleanup(app, path);
}

#[tokio::test]
async fn refused_packs_say_why() {
    let (app, path) = fixture();
    let (status, body) = upload_as(&app, "cerro", sample_pack("aki", 1)).await;
    assert_eq!(
        (status, &body["reason"]),
        (StatusCode::BAD_REQUEST, &json!("reserved_id"))
    );
    let (status, body) = upload_as(&app, "cerro", b"not a zip".to_vec()).await;
    assert_eq!(
        (status, &body["reason"]),
        (StatusCode::BAD_REQUEST, &json!("bad_entry_name"))
    );
    let (status, body) = upload_as(&app, "cerro", vec![0; MAX_ARCHIVE + 1]).await;
    assert_eq!(
        (status, &body["reason"]),
        (StatusCode::PAYLOAD_TOO_LARGE, &json!("too_large"))
    );
    cleanup(app, path);
}

#[tokio::test]
async fn a_pack_with_the_same_id_replaces_the_last_and_stays_shown() {
    let (app, path) = fixture();
    let (_, first) = upload_as(&app, "cerro", sample_pack("mochi", 1)).await;
    let sha = first["sha256"].as_str().unwrap();
    json_of(
        &app,
        "PUT",
        &format!("/api/packs/cerro/{sha}"),
        "cerro",
        json!({"visibility": "friends"}),
    )
    .await;

    let (status, second) = upload_as(&app, "cerro", sample_pack("mochi", 2)).await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(second["sha256"], first["sha256"]);
    assert_eq!(second["visibility"], "friends");
    let (_, list) = json_of(&app, "GET", "/api/packs/cerro", "cerro", json!(null)).await;
    assert_eq!(list["packs"].as_array().unwrap().len(), 1);
    assert_eq!(list["used"], second["size"]);
    assert_eq!(list["max_packs"], MAX_PACKS);

    // Uploading the stored pack again keeps its bytes and hash.
    let own = format!(
        "/api/packs/cerro/file/{}",
        second["sha256"].as_str().unwrap()
    );
    let (_, _, zip) = send(&app, "GET", &own, "cerro", vec![], &[]).await;
    let (status, again) = upload_as(&app, "cerro", zip).await;
    assert_eq!(
        (status, &again["sha256"]),
        (StatusCode::OK, &second["sha256"])
    );
    cleanup(app, path);
}

#[tokio::test]
async fn players_keep_a_limited_number_of_packs_and_uploads() {
    let (app, path) = fixture();
    for n in 0..MAX_PACKS {
        let (status, _) = upload_as(&app, "cerro", sample_pack(&format!("pack_{n}"), 0)).await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, body) = upload_as(&app, "cerro", sample_pack("one_more", 0)).await;
    assert_eq!(
        (status, &body["reason"]),
        (StatusCode::CONFLICT, &json!("quota"))
    );

    // Refused uploads count toward the day's limit too.
    for _ in MAX_PACKS + 1..UPLOADS_PER_DAY {
        upload_as(&app, "cerro", b"junk".to_vec()).await;
    }
    let (status, body) = upload_as(&app, "cerro", b"junk".to_vec()).await;
    assert_eq!(
        (status, &body["reason"]),
        (StatusCode::TOO_MANY_REQUESTS, &json!("throttled"))
    );
    cleanup(app, path);
}

#[tokio::test]
async fn a_report_hides_the_pack_for_the_reporter_until_withdrawn() {
    let (app, path) = fixture();
    befriend(&app, "hill", "cerro");
    let (_, pack) = upload_as(&app, "cerro", sample_pack("mochi", 3)).await;
    let sha = pack["sha256"].as_str().unwrap();
    json_of(
        &app,
        "PUT",
        &format!("/api/packs/cerro/{sha}"),
        "cerro",
        json!({"visibility": "friends"}),
    )
    .await;

    let report = format!("/api/packs/hill/reports/{sha}");
    let (status, _) = json_of(
        &app,
        "POST",
        &report,
        "hill",
        json!({"reason": "offensive"}),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = json_of(&app, "POST", &report, "hill", json!({"reason": "other"})).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, friends) = json_of(&app, "GET", "/api/packs/hill/friends", "hill", json!(null)).await;
    assert_eq!(friends, json!({"friends": {}}));
    let reason: String = app
        .store
        .lock()
        .unwrap()
        .conn
        .query_row("select reason from pack_reports", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        reason, "offensive",
        "reporting twice keeps the first report"
    );

    // Someone who cannot see it cannot report it; withdrawing shows it again.
    let stranger = format!("/api/packs/friend/reports/{sha}");
    let (status, _) = json_of(
        &app,
        "POST",
        &stranger,
        "friend",
        json!({"reason": "other"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    json_of(&app, "DELETE", &report, "hill", json!(null)).await;
    let (_, friends) = json_of(&app, "GET", "/api/packs/hill/friends", "hill", json!(null)).await;
    assert_eq!(friends["friends"]["cerro"]["sha256"], sha);
    cleanup(app, path);
}

#[tokio::test]
async fn a_removed_pack_is_gone_for_everyone_and_cannot_come_back() {
    let (app, path) = fixture();
    befriend(&app, "hill", "cerro");
    let (_, pack) = upload_as(&app, "cerro", sample_pack("mochi", 4)).await;
    let sha = pack["sha256"].as_str().unwrap().to_string();
    json_of(
        &app,
        "PUT",
        &format!("/api/packs/cerro/{sha}"),
        "cerro",
        json!({"visibility": "friends"}),
    )
    .await;
    let (_, _, zip) = send(
        &app,
        "GET",
        &format!("/api/packs/hill/file/{sha}"),
        "hill",
        vec![],
        &[],
    )
    .await;

    let owners = remove(&app.store.lock().unwrap().conn, &sha, now_ms()).unwrap();
    assert_eq!(owners, 1);
    let (_, friends) = json_of(&app, "GET", "/api/packs/hill/friends", "hill", json!(null)).await;
    assert_eq!(friends, json!({"friends": {}}));
    let (_, list) = json_of(&app, "GET", "/api/packs/cerro", "cerro", json!(null)).await;
    assert_eq!(list["packs"], json!([]));

    // Neither the original upload nor the rebuilt pack comes back.
    for upload in [sample_pack("mochi", 4), zip] {
        let (status, body) = upload_as(&app, "hill", upload).await;
        assert_eq!(
            (status, &body["reason"]),
            (StatusCode::FORBIDDEN, &json!("removed"))
        );
    }
    assert!(remove(&app.store.lock().unwrap().conn, "not-a-hash", now_ms()).is_err());
    cleanup(app, path);
}

#[tokio::test]
async fn deleting_a_pack_stops_showing_it() {
    let (app, path) = fixture();
    befriend(&app, "hill", "cerro");
    let (_, pack) = upload_as(&app, "cerro", sample_pack("mochi", 5)).await;
    let sha = pack["sha256"].as_str().unwrap();
    let own = format!("/api/packs/cerro/{sha}");
    json_of(&app, "PUT", &own, "cerro", json!({"visibility": "friends"})).await;

    assert_eq!(
        send(&app, "DELETE", &own, "cerro", vec![], &[]).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send(&app, "DELETE", &own, "cerro", vec![], &[]).await.0,
        StatusCode::NOT_FOUND
    );
    let (status, _) = json_of(&app, "PUT", &own, "cerro", json!({"visibility": "friends"})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let shown: i64 = app
        .store
        .lock()
        .unwrap()
        .conn
        .query_row("select count(*) from shown_packs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(shown, 0);
    cleanup(app, path);
}
