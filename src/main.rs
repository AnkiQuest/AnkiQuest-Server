/// A file built from `web/` and `static/` by build.rs.
macro_rules! web {
    ($name:literal) => {
        include_str!(concat!(env!("OUT_DIR"), "/web/", $name))
    };
}

#[macro_use]
mod admin;
mod access;
mod accounts;
mod aki;
#[cfg(test)]
mod api_examples;
mod app;
mod avatars;
mod boards;
mod capabilities;
mod cards;
mod challenges;
mod cli;
mod companions;
mod competition;
mod config;
mod conquests;
mod countdowns;
mod crash_reports;
mod crash_signature;
#[cfg(test)]
mod day_verdicts;
mod deck_copies;
mod decks;
mod dev;
mod feedback;
mod freezes;
mod friend_nudges;
mod game;
mod goals;
mod i18n;
mod inbox;
mod messages;
mod migrations;
mod notifications;
mod oauth;
mod observations;
mod privacy;
mod reciprocity;
mod reminders;
mod robots;
mod shares;
mod social;
mod store;
mod study;
#[cfg(test)]
mod study_time;
mod subscriptions;
mod synced_settings;
#[cfg(test)]
mod test_support;
mod two_factor;
mod web_util;
mod weekly_challenges;
mod wins;
mod worker;

use app::{App, load_player, now_ms};
use axum::Router;
use axum::extract::Path as UrlPath;
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use cli::parse_args;
use config::Config;
use game::Periods;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use store::{Error, Store};
use web_util::store_error;

/// One log line for every request the server refuses or fails: a 4xx answer is a warning,
/// a 5xx an error, with the method and path but never the query, which carries sign-in
/// secrets. What succeeded stays silent, so the log shows what to look at: input the
/// server rejected, store and invariant failures, and bugs that answered 500. Tests ask
/// the same question of [`rejection_line`] directly instead, because the suite shares
/// one pending-entry buffer its own rejections would fill.
async fn log_rejections(
    method: axum::http::Method,
    uri: axum::http::Uri,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let response = next.run(request).await;
    if let Some((level, message)) = rejection_line(&method, &uri, response.status().as_u16()) {
        #[cfg(not(test))]
        match level {
            "error" => log_error!("{message}"),
            _ => log_warning!("{message}"),
        }
        #[cfg(test)]
        let _ = (level, message);
    }
    response
}

/// The level and line one answer records, or none when it served. The line names the
/// method and the path, and never the query, which carries sign-in secrets.
fn rejection_line(
    method: &axum::http::Method,
    uri: &axum::http::Uri,
    status: u16,
) -> Option<(&'static str, String)> {
    if status < 400 {
        return None;
    }
    let level = if status >= 500 { "error" } else { "warning" };
    Some((
        level,
        format!("{} {} answered {}", method, uri.path(), status),
    ))
}

async fn index() -> Html<&'static str> {
    Html(web!("index.html"))
}

async fn community_page() -> Html<&'static str> {
    Html(web!("community.html"))
}

async fn personal_page() -> Html<&'static str> {
    Html(web!("personal.html"))
}

/// The dashboard is one page; `/day`, `/month` and the rest pick a leaderboard period.
async fn period_page(UrlPath(period): UrlPath<String>) -> Result<Html<&'static str>, StatusCode> {
    if Periods::NAMES.contains(&period.as_str()) {
        Ok(index().await)
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

async fn manifest() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/manifest+json")],
        include_str!("../static/manifest.webmanifest"),
    )
}

async fn icon() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "image/svg+xml")],
        include_str!("../static/icon.svg"),
    )
}

/// The listening socket systemd passes when a `.socket` unit starts the server.
/// The port then belongs to the socket unit, so the service's own address
/// rules can shut it out of loopback without refusing nginx.
#[cfg(unix)]
fn inherited_listener() -> Option<std::net::TcpListener> {
    use std::os::fd::FromRawFd;
    let pid: u32 = std::env::var("LISTEN_PID").ok()?.parse().ok()?;
    if pid != std::process::id() || std::env::var("LISTEN_FDS").ok()? != "1" {
        return None;
    }
    // SAFETY: systemd passes exactly one descriptor, at 3, and nothing else here owns it.
    Some(unsafe { std::net::TcpListener::from_raw_fd(3) })
}

#[cfg(not(unix))]
fn inherited_listener() -> Option<std::net::TcpListener> {
    None
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let admin_at = args.iter().position(|a| a == "admin").filter(|&i| i <= 1);
    let given = match admin_at {
        Some(at) => args[..at].first().cloned(),
        None => parse_args(&args)?,
    };
    let path = given
        .or_else(|| std::env::var("ANKIQUEST_CONFIG").ok())
        .unwrap_or_else(|| "ankiquest.json".into());
    let mut config = Config::read(&path)?;

    if let Some(at) = admin_at {
        let command = match &args[at + 1..] {
            [] => None,
            [help] if matches!(help.as_str(), "help" | "--help" | "-h") => None,
            rest => Some(admin::parse(rest)),
        };
        let output = match command {
            None => Ok(format!("{}\n", admin::USAGE)),
            Some(Ok(admin::Command::AdoptPlayers)) => config.load_tokens().and_then(|()| {
                let players: Vec<admin::Configured> = config
                    .users
                    .iter()
                    .map(|(user, c)| admin::Configured {
                        user: user.clone(),
                        display: c.display.clone(),
                        token: config.token(user).map(str::to_string),
                    })
                    .collect();
                admin::adopt(&config.state_dir, &players, now_ms())
            }),
            Some(Ok(admin::Command::SignIns)) => admin::sign_ins(
                &config.state_dir,
                &config.token_players(),
                &config.oauth.named(),
            ),
            Some(Ok(command)) if command.is_about_people() => {
                let backups = std::env::var_os("ANKIQUEST_BACKUP_DIR")
                    .map_or_else(|| config.state_dir.join("backups"), Into::into);
                admin::people(
                    &admin::Setup {
                        state_dir: &config.state_dir,
                        backups: &backups,
                        configured: config.users.keys().cloned().collect(),
                        sync_base: config.sync_base.as_deref(),
                    },
                    command,
                    now_ms(),
                )
            }
            Some(command) => {
                command.and_then(|c| admin::run(&config.state_dir, config.settings(), c, now_ms()))
            }
        };
        match output {
            Ok(output) => print!("{output}"),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(2);
            }
        }
        return Ok(());
    }

    game::set_review_weighting(config.review_weighting);

    config.check()?;

    config.load_tokens()?;

    config.oauth.load_secrets(config.public_url.as_deref())?;
    let access = access::Access::from_config(&config)?;
    let store = Store::open(&config.state_dir)?;
    game::set_streak_rule(admin::streak_rule(
        &store.conn,
        config.settings().effective(&store.conn)?.streak_minutes,
        now_ms(),
    )?);
    let accounts = accounts::Directory::load(&store.conn)?;
    let mut players = HashMap::new();
    for user in store.users()? {
        players.insert(user.clone(), load_player(&store, &user)?);
    }
    let week = config.week()?;
    let app = Arc::new(App {
        week,
        config,
        access,
        accounts,
        players: RwLock::new(players),
        standings: Default::default(),
        heartbeat: Default::default(),
        rank_weeks: Default::default(),
        crash_reports: Default::default(),
        store: Mutex::new(store),
    });

    privacy::warn_about_unknown_players(&app);
    worker::spawn(app.clone());

    let router = router(app.clone());

    let listener = match inherited_listener() {
        Some(listener) => {
            listener.set_nonblocking(true)?;
            tokio::net::TcpListener::from_std(listener)?
        }
        None => tokio::net::TcpListener::bind(&app.config.addr).await?,
    };
    println!("ankiquest listening on http://{}", listener.local_addr()?);
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}

fn router(app: Arc<App>) -> Router {
    Router::new()
        .merge(aki::routes())
        .merge(avatars::routes())
        .merge(friend_nudges::routes())
        .merge(shares::routes())
        .merge(wins::routes())
        .merge(goals::routes())
        .merge(conquests::routes())
        .merge(social::routes())
        .merge(privacy::routes())
        .merge(two_factor::routes())
        .merge(dev::routes())
        .merge(oauth::routes())
        .merge(competition::routes())
        .merge(challenges::routes())
        .merge(reminders::routes())
        .merge(decks::routes())
        .merge(subscriptions::routes())
        .merge(deck_copies::routes())
        .merge(freezes::routes())
        .merge(inbox::routes())
        .merge(messages::routes())
        .merge(study::routes())
        .merge(observations::routes())
        .merge(boards::routes())
        .merge(cards::routes())
        .merge(capabilities::routes())
        .merge(synced_settings::routes())
        .merge(crash_reports::routes())
        .route("/api/health", get(worker::health))
        .route("/", get(index))
        .route("/records", get(index))
        .route("/community", get(community_page))
        .route("/today", get(personal_page))
        .route("/history", get(personal_page))
        .route("/settings", get(personal_page))
        .route("/friends", get(personal_page))
        .route("/join/{code}", get(personal_page))
        .route("/{period}", get(period_page))
        .route("/manifest.webmanifest", get(manifest))
        .route("/icon.svg", get(icon))
        .route("/site.css", get(access::site_css))
        .route("/site.js", get(access::site_js))
        .route("/i18n.wasm", get(access::i18n_wasm))
        .route("/personal.css", get(access::personal_css))
        .route("/personal.js", get(access::personal_js))
        .route("/login", get(access::login))
        .route("/auth/status", get(access::status))
        .route(
            "/auth/session",
            post(access::session).layer(axum::extract::DefaultBodyLimit::max(4096)),
        )
        .route("/auth/logout", post(access::logout))
        .route(
            "/api/accounts",
            post(accounts::signup).layer(axum::extract::DefaultBodyLimit::max(4096)),
        )
        .route(
            "/api/accounts/tokens",
            post(accounts::login)
                .delete(accounts::logout)
                .layer(axum::extract::DefaultBodyLimit::max(4096)),
        )
        .route(
            "/api/devices/{user}",
            get(accounts::list_devices)
                .post(accounts::change_devices)
                .layer(axum::extract::DefaultBodyLimit::max(4096)),
        )
        .route(
            "/api/companion/{user}",
            get(companions::get).post(companions::set),
        )
        .route(
            "/api/unlocks/{user}",
            get(companions::get_unlocks)
                .post(companions::add_unlock)
                .layer(axum::extract::DefaultBodyLimit::max(4096)),
        )
        .route("/api/language/{user}", get(i18n::get).post(i18n::set))
        .route(robots::PATH, get(robots::robots))
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            reciprocity::mark,
        ))
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            access::gate,
        ))
        .layer(axum::middleware::from_fn(log_rejections))
        .with_state(app)
}

#[cfg(test)]
mod rejection_log_tests {
    use super::rejection_line;
    use axum::http::Method;
    use axum::http::Uri;

    #[test]
    fn refusals_answer_one_line_without_the_query() {
        let login: Uri = "/login".parse().unwrap();
        assert!(
            rejection_line(&Method::GET, &login, 200).is_none(),
            "served pages stay silent"
        );
        assert!(
            rejection_line(&Method::GET, &login, 302).is_none(),
            "redirects stay silent"
        );
        let refused: Uri = "/api/decks/alice?token=secret".parse().unwrap();
        let (level, line) = rejection_line(&Method::GET, &refused, 401).unwrap();
        assert_eq!(level, "warning");
        assert_eq!(
            line, "GET /api/decks/alice answered 401",
            "no query, which holds secrets"
        );
        let failed: Uri = "/api/study/hill".parse().unwrap();
        let (level, line) = rejection_line(&Method::POST, &failed, 500).unwrap();
        assert_eq!(level, "error");
        assert_eq!(line, "POST /api/study/hill answered 500");
    }
}
