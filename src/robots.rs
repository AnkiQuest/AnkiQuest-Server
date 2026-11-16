//! `/robots.txt`: no crawling, except link previews of the pages players share on purpose.

use axum::http::header;
use axum::response::IntoResponse;

pub(crate) const PATH: &str = "/robots.txt";

/// How long crawlers may keep the rules, in seconds.
const MAX_AGE: &str = "max-age=86400";

pub(crate) async fn robots() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, MAX_AGE),
        ],
        include_str!("../static/robots.txt"),
    )
}

#[cfg(test)]
mod tests {
    use crate::test_support::*;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;

    async fn get(app: &Arc<App>, path: &str) -> axum::response::Response {
        route(app, Request::get(path).body(Body::empty()).unwrap()).await
    }

    #[tokio::test]
    async fn robots_txt_keeps_crawlers_out_but_lets_link_previews_in() {
        // Served on a private site too, where every other page needs a sign-in.
        let (app, path) = app_with(serde_json::json!({
            "private_site": true,
            "users": {"alice": {"token": "alice-token"}}
        }));
        let response = get(&app, "/robots.txt").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/plain")
        );
        let text = String::from_utf8(
            to_bytes(response.into_body(), 1 << 16)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        let groups: Vec<&str> = text.split("\n\n").collect();
        let group = |agent: &str| {
            groups
                .iter()
                .find(|group| {
                    group
                        .lines()
                        .any(|line| line == format!("User-agent: {agent}"))
                })
                .unwrap_or_else(|| panic!("no group for {agent}:\n{text}"))
                .to_string()
        };

        // Everyone, and the AI and scraper crawlers by name, may crawl nothing.
        for agent in [
            "*",
            "GPTBot",
            "ChatGPT-User",
            "CCBot",
            "ClaudeBot",
            "anthropic-ai",
            "Google-Extended",
            "PerplexityBot",
            "Bytespider",
            "Amazonbot",
            "Applebot-Extended",
            "meta-externalagent",
        ] {
            let rules = group(agent);
            assert!(rules.lines().any(|line| line == "Disallow: /"), "{agent}");
            assert!(!rules.contains("Allow: /w/"), "{agent}");
        }

        // Link-preview bots may fetch shared pages and their cards, and nothing else.
        for agent in [
            "Twitterbot",
            "facebookexternalhit",
            "Slackbot",
            "Discordbot",
            "TelegramBot",
            "WhatsApp",
        ] {
            let rules = group(agent);
            assert!(rules.lines().any(|line| line == "Allow: /w/"), "{agent}");
            assert!(rules.lines().any(|line| line == "Disallow: /"), "{agent}");
        }
        cleanup(app, path);
    }

    #[tokio::test]
    async fn pages_with_player_data_ask_not_to_be_indexed() {
        let (app, path) = fixture();
        for page in ["/", "/api/leaderboard", "/community"] {
            let response = get(&app, page).await;
            assert_eq!(response.headers()["x-robots-tag"], "noindex", "{page}");
        }
        cleanup(app, path);
    }
}
