//! Public, embedded mascot art. Only these fixed assets are served before sign-in.
use crate::app::App;
use axum::{
    Router,
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::get,
};
use std::sync::Arc;

// Every companion picture, by the path it is served at: masters as they are, face icons and
// Anki-tan's pictures made smaller. build.rs finds each through the art submodule's manifest.
include!(concat!(env!("OUT_DIR"), "/art/pictures.rs"));

pub(crate) fn public_asset(path: &str) -> bool {
    PICTURES.iter().any(|(served, _)| *served == path)
}

async fn asset(uri: Uri) -> Response {
    let Some((_, bytes)) = PICTURES.iter().find(|(served, _)| *served == uri.path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    (
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        *bytes,
    )
        .into_response()
}

pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/aki/{name}", get(asset))
        .route("/ankilope/{name}", get(asset))
        .route("/anki-tan/{name}", get(asset))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn get(path: &str) -> Response {
        asset(path.parse().unwrap()).await
    }

    #[tokio::test]
    async fn only_named_images_are_public_and_have_png_content() {
        for name in [
            "welcome",
            "review",
            "celebrate",
            "streak",
            "freeze",
            "winner",
            "face",
            "face-192",
            "face-32",
        ] {
            let path = format!("/aki/{name}.png");
            assert!(public_asset(&path));
            let response = get(&path).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
            let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
                .await
                .unwrap();
            assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        }
        for name in ["config.json", "..%2Fconfig.json", "FACE.png", "unknown.png"] {
            let path = format!("/aki/{name}");
            assert!(!public_asset(&path));
            assert_eq!(get(&path).await.status(), StatusCode::NOT_FOUND);
        }
        for name in ["welcome", "study", "celebrate", "face", "face-192"] {
            let path = format!("/ankilope/{name}.png");
            assert!(public_asset(&path));
            let response = get(&path).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
        }
        // #61: Anki-tan's pictures, exported at web size from her masters or the ones their
        // fallbacks reach.
        let names = ["welcome", "review", "happy", "streak", "frozen"];
        for name in names.into_iter().chain(["face-192"]) {
            let path = format!("/anki-tan/{name}.png");
            assert!(public_asset(&path), "{path}");
            let response = get(&path).await;
            assert_eq!(response.status(), StatusCode::OK, "{path}");
        }

        // Two of her pictures have the same bytes exactly when the pinned release's manifest
        // and the pose fixture lead both to the same master.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let manifest = ankiquest_art::Manifest::read(&root.join(ankiquest_art::MASTERS)).unwrap();
        let fixture = std::fs::read_to_string(root.join(ankiquest_art::POSES)).unwrap();
        let poses = ankiquest_art::Poses::parse(&fixture).unwrap();
        let master = |pose: &str| {
            let shown = poses.resolve(pose, |pose| manifest.has("anki_tan", pose))?;
            Some(manifest.master("anki_tan", shown)?.file.clone())
        };
        let bytes = |name: &str| {
            let path = format!("/anki-tan/{name}.png");
            PICTURES
                .iter()
                .find(|(served, _)| *served == path)
                .unwrap()
                .1
        };
        for one in names {
            for other in names {
                assert_eq!(
                    bytes(one) == bytes(other),
                    master(one) == master(other),
                    "{one} and {other}"
                );
            }
        }
        assert!(!public_asset("/ankilope/config.json"));
        assert!(!public_asset("/ankilope/winner.png"));
        assert_eq!(
            get("/ankilope/unknown.png").await.status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn masters_are_served_as_they_are() {
        // `celebrate` keeps the name wins store and shows the happy master, byte for byte.
        let response = get("/aki/celebrate.png").await;
        let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap();
        let masters = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(ankiquest_art::MASTERS);
        let master = std::fs::read(masters.join("aki/happy.png")).unwrap();
        assert_eq!(bytes.as_ref(), master.as_slice());
    }
}
