//! Two-factor authentication for accounts: codes from an authenticator app (TOTP, RFC 6238)
//! and single-use recovery codes, asked for when an account signs in with its password.
//!
//! It is optional and needs a password, since it only guards password sign-in. Signing in
//! with Google, GitHub or GitLab relies on that provider's own second factor. Device tokens
//! issued before it was turned on keep working; a new one, by password, needs a code.

use crate::app::{App, now_ms};
use crate::web_util::{hex, random_bytes, refuse, reply};
use crate::{Error, access};
use axum::Router;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use totp_rs::{Algorithm, Builder, Secret, Totp};

/// The capability clients look for before they offer two-factor settings.
pub(crate) const FEATURE: &str = "two_factor";

// The RFC 6238 parameters every common authenticator app uses unless told otherwise.
const ALGORITHM: Algorithm = Algorithm::SHA1;
const ALGORITHM_NAME: &str = "SHA1";
const DIGITS: u8 = 6;
const STEP_SECONDS: u64 = 30;
/// Codes from one step either side of now still count, for clocks that drift (RFC 6238 5.2).
const SKEW_STEPS: u16 = 1;
/// 160 bits, as RFC 4226 section 4 recommends.
const SECRET_BYTES: usize = 20;
const ISSUER: &str = "AnkiQuest";

// Recovery codes: ten of them, each ten characters shown as two groups of five, from an
// alphabet without look-alikes (no 0, 1, i, l, o).
const RECOVERY_CODES: usize = 10;
const RECOVERY_GROUP: usize = 5;
const RECOVERY_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
/// Longer input is refused before any work is done.
const MAX_CODE_CHARS: usize = 64;

// Wrong codes an account may send before it has to wait. Per account, since an attacker who
// has the password can come from many addresses; the per-address limit applies as well.
const MAX_CODE_FAILURES: u32 = 5;
const CODE_WINDOW_MS: i64 = 15 * 60 * 1000;
const MAX_TRACKED_ACCOUNTS: usize = 4096;

// Messages, shared by the sign-in refusals and the settings actions.
const ENTER_CODE: &str = "Enter the code from your authenticator app.";
const WRONG_CODE: &str = "That code is not right.";
const TOO_MANY_CODES: &str = "Too many wrong codes. Try again in 15 minutes.";

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists two_factor (
             user text primary key references accounts (user) on delete cascade,
             secret text not null,
             state text not null check (state in ('pending', 'on')),
             last_step integer not null default 0,
             updated_at integer not null
         ) without rowid;
         create table if not exists two_factor_recovery (
             code_hash text primary key,
             user text not null references accounts (user) on delete cascade
         ) without rowid;
         create index if not exists two_factor_recovery_user on two_factor_recovery (user);",
    )?;
    Ok(())
}

/// Where an account's two-factor setup stands.
#[derive(Debug, PartialEq)]
enum Enrolment {
    Off,
    /// A secret was handed out and no code from it has been confirmed yet.
    Pending {
        secret: String,
    },
    /// Sign-in asks for a code. `last_step` is the newest step a code was accepted for, so the
    /// same code cannot be used twice.
    On {
        secret: String,
        last_step: u64,
    },
}

impl Enrolment {
    fn name(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Pending { .. } => "pending",
            Self::On { .. } => "on",
        }
    }
}

fn enrolment(conn: &Connection, user: &str) -> Result<Enrolment, Error> {
    let row = conn
        .query_row(
            "select secret, state, last_step from two_factor where user = ?1",
            [user],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?;
    Ok(match row {
        None => Enrolment::Off,
        Some((secret, state, _)) if state == "pending" => Enrolment::Pending { secret },
        Some((secret, _, last_step)) => Enrolment::On {
            secret,
            last_step: u64::try_from(last_step).unwrap_or_default(),
        },
    })
}

/// Whether sign-in by password asks `user` for a second factor.
pub(crate) fn enabled(conn: &Connection, user: &str) -> Result<bool, Error> {
    Ok(matches!(enrolment(conn, user)?, Enrolment::On { .. }))
}

fn totp(secret: &str) -> Result<Totp, Error> {
    let secret = Secret::try_from_base32(secret).map_err(|_| "unreadable two-factor secret")?;
    Builder::new()
        .with_algorithm(ALGORITHM)
        .with_digits(DIGITS)
        .with_skew(SKEW_STEPS)
        .with_step_duration(STEP_SECONDS)
        .with_secret(secret)
        .build()
        .map_err(|e| e.to_string().into())
}

/// The code an authenticator app shows for `secret` at `now`, for tests elsewhere.
#[cfg(test)]
pub(crate) fn code_at(secret: &str, now: i64) -> String {
    let seconds = u64::try_from(now / 1000).unwrap_or_default();
    totp(secret).unwrap().generate(seconds).to_string()
}

fn new_secret() -> Result<String, Error> {
    let bytes = random_bytes("two-factor randomness unavailable")?;
    Ok(Secret::from(bytes[..SECRET_BYTES].to_vec()).to_base32())
}

/// The address authenticator apps read from the QR code.
fn otpauth_uri(user: &str, secret: &str) -> String {
    format!(
        "otpauth://totp/{ISSUER}:{user}?secret={secret}&issuer={ISSUER}&algorithm={ALGORITHM_NAME}&digits={DIGITS}&period={STEP_SECONDS}"
    )
}

/// What a player typed, without the spaces and dashes apps and recovery codes show.
fn normalize(code: &str) -> String {
    code.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .flat_map(char::to_lowercase)
        .collect()
}

/// The step a six-digit `code` belongs to, if it is right for `secret` around `now`.
fn totp_step(secret: &str, code: &str, now: i64) -> Result<Option<u64>, Error> {
    let seconds = u64::try_from(now / 1000).unwrap_or_default();
    Ok(totp(secret)?.check(code, seconds))
}

fn recovery_hash(code: &str) -> String {
    hex(&Sha256::digest(normalize(code).as_bytes()))
}

fn recovery_code() -> Result<String, Error> {
    // Rejection sampling keeps every character equally likely.
    let limit = RECOVERY_ALPHABET.len() * (256 / RECOVERY_ALPHABET.len());
    let mut chars = Vec::with_capacity(2 * RECOVERY_GROUP);
    while chars.len() < 2 * RECOVERY_GROUP {
        for byte in random_bytes("recovery code randomness unavailable")? {
            if usize::from(byte) < limit && chars.len() < 2 * RECOVERY_GROUP {
                chars.push(RECOVERY_ALPHABET[usize::from(byte) % RECOVERY_ALPHABET.len()]);
            }
        }
    }
    let (first, second) = chars.split_at(RECOVERY_GROUP);
    Ok(format!(
        "{}-{}",
        String::from_utf8_lossy(first),
        String::from_utf8_lossy(second)
    ))
}

/// Replaces `user`'s recovery codes with new ones and returns them; only their hashes are kept.
fn new_recovery_codes(conn: &Connection, user: &str) -> Result<Vec<String>, Error> {
    let codes = (0..RECOVERY_CODES)
        .map(|_| recovery_code())
        .collect::<Result<Vec<_>, _>>()?;
    conn.execute("delete from two_factor_recovery where user = ?1", [user])?;
    for code in &codes {
        conn.execute(
            "insert into two_factor_recovery (code_hash, user) values (?1, ?2)",
            params![recovery_hash(code), user],
        )?;
    }
    Ok(codes)
}

fn recovery_left(conn: &Connection, user: &str) -> Result<i64, Error> {
    Ok(conn.query_row(
        "select count(*) from two_factor_recovery where user = ?1",
        [user],
        |r| r.get(0),
    )?)
}

/// Whether a code is the right second factor for an account that has two-factor on.
#[derive(Debug, PartialEq)]
enum Checked {
    Accepted,
    Refused,
}

/// Checks `code` against `user`'s authenticator, or spends one of their recovery codes. Holds
/// the store lock throughout, so two requests cannot both use one code.
fn check_code(conn: &Connection, user: &str, code: &str, now: i64) -> Result<Checked, Error> {
    let Enrolment::On { secret, last_step } = enrolment(conn, user)? else {
        return Ok(Checked::Refused);
    };
    let code = normalize(code);
    if code.chars().count() > MAX_CODE_CHARS {
        return Ok(Checked::Refused);
    }

    // Six digits are an authenticator code; a code is only good once, so its step must be newer.
    if code.len() == usize::from(DIGITS) && code.bytes().all(|b| b.is_ascii_digit()) {
        let Some(step) = totp_step(&secret, &code, now)?.filter(|step| *step > last_step) else {
            return Ok(Checked::Refused);
        };
        conn.execute(
            "update two_factor set last_step = ?2, updated_at = ?3 where user = ?1",
            params![user, i64::try_from(step).unwrap_or(i64::MAX), now],
        )?;
        return Ok(Checked::Accepted);
    }

    // Anything else may be a recovery code, which is spent by using it.
    let spent = conn.execute(
        "delete from two_factor_recovery where user = ?1 and code_hash = ?2",
        params![user, recovery_hash(&code)],
    )?;
    Ok(if spent == 1 {
        Checked::Accepted
    } else {
        Checked::Refused
    })
}

/// Recent wrong codes per account.
#[derive(Default)]
pub(crate) struct Attempts {
    failures: Mutex<HashMap<String, (i64, u32)>>,
}

impl Attempts {
    fn locked(&self, user: &str, now: i64) -> bool {
        let mut failures = self.failures.lock().unwrap();
        failures.retain(|_, (started, _)| now >= *started && now - *started < CODE_WINDOW_MS);
        failures
            .get(user)
            .is_some_and(|(_, count)| *count >= MAX_CODE_FAILURES)
    }

    fn failure(&self, user: &str, now: i64) {
        let mut failures = self.failures.lock().unwrap();
        if !failures.contains_key(user)
            && failures.len() >= MAX_TRACKED_ACCOUNTS
            && let Some(oldest) = failures
                .iter()
                .min_by_key(|(_, (started, _))| *started)
                .map(|(user, _)| user.clone())
        {
            failures.remove(&oldest);
        }
        let entry = failures.entry(user.to_string()).or_insert((now, 0));
        entry.1 = entry.1.saturating_add(1);
    }

    fn clear(&self, user: &str) {
        self.failures.lock().unwrap().remove(user);
    }
}

/// Where a password sign-in stands: the password is checked first, then, for an account that
/// has two-factor on, the code. Only `SignedIn` may issue a session or a device token.
#[derive(Debug, PartialEq)]
pub(crate) enum SignInStep {
    /// The username or the password is not right.
    PasswordRefused,
    /// The password is right; the account has two-factor on and no code came with it.
    AwaitingSecondFactor,
    /// The password is right; the code is wrong or was used before.
    SecondFactorRefused,
    /// The password is right; the account sent too many wrong codes lately.
    SecondFactorLocked,
    /// The password and, where needed, the code are right.
    SignedIn,
}

impl SignInStep {
    /// Whether this attempt counts toward the per-address limit on sign-in failures.
    pub(crate) fn failed(&self) -> bool {
        matches!(self, Self::PasswordRefused | Self::SecondFactorRefused)
    }

    /// The answer for an attempt that did not sign in. `second_factor` tells clients that
    /// support it to ask for a code; older clients show the message.
    pub(crate) fn refusal(&self) -> Response {
        let (status, message) = match self {
            Self::PasswordRefused => (
                StatusCode::UNAUTHORIZED,
                "Username or password not recognized.",
            ),
            Self::AwaitingSecondFactor => (StatusCode::UNAUTHORIZED, ENTER_CODE),
            Self::SecondFactorRefused => (StatusCode::UNAUTHORIZED, WRONG_CODE),
            Self::SecondFactorLocked => (StatusCode::TOO_MANY_REQUESTS, TOO_MANY_CODES),
            Self::SignedIn => (StatusCode::OK, ""),
        };
        if *self == Self::PasswordRefused {
            return refuse(status, message);
        }
        reply(
            status,
            serde_json::json!({"error": message, "second_factor": true}),
        )
    }
}

/// Checks a password sign-in for `user`, with `code` when the client has asked for one.
pub(crate) async fn sign_in(
    app: &App,
    user: &str,
    password: &str,
    code: Option<&str>,
) -> SignInStep {
    if !crate::accounts::password_matches(app, user, password).await {
        return SignInStep::PasswordRefused;
    }
    let now = now_ms();
    let store = app.store.lock().unwrap();

    // Accounts without two-factor, and a store that cannot say, sign in on the password
    // alone only in the first case.
    match enabled(&store.conn, user) {
        Ok(false) => return SignInStep::SignedIn,
        Ok(true) => {}
        Err(error) => {
            log_error!("two-factor: {error}");
            return SignInStep::SecondFactorRefused;
        }
    }
    if app.access.two_factor.locked(user, now) {
        return SignInStep::SecondFactorLocked;
    }
    let Some(code) = code.filter(|code| !code.trim().is_empty()) else {
        return SignInStep::AwaitingSecondFactor;
    };
    match check_code(&store.conn, user, code, now) {
        Ok(Checked::Accepted) => {
            app.access.two_factor.clear(user);
            SignInStep::SignedIn
        }
        Ok(Checked::Refused) => {
            app.access.two_factor.failure(user, now);
            SignInStep::SecondFactorRefused
        }
        Err(error) => {
            log_error!("two-factor: {error}");
            SignInStep::SecondFactorRefused
        }
    }
}

fn internal(error: Error) -> Response {
    log_error!("two-factor: {error}");
    refuse(
        StatusCode::INTERNAL_SERVER_ERROR,
        "That could not be saved. Please try again.",
    )
}

/// Two-factor needs an account with a password: it only guards password sign-in.
fn available(app: &App, conn: &Connection, user: &str) -> Result<bool, Error> {
    Ok(app.accounts.contains(user) && crate::accounts::has_password(conn, user)?)
}

fn status_of(app: &App, user: &str) -> Result<serde_json::Value, Error> {
    let store = app.store.lock().unwrap();
    Ok(serde_json::json!({
        "available": available(app, &store.conn, user)?,
        "state": enrolment(&store.conn, user)?.name(),
        "recovery_codes_left": recovery_left(&store.conn, user)?,
    }))
}

/// `{"available", "state": "off" | "pending" | "on", "recovery_codes_left"}`.
async fn status(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !access::authorized(&app, &user, &headers) {
        return refuse(StatusCode::UNAUTHORIZED, "Sign in first.");
    }
    match status_of(&app, &user) {
        Ok(status) => reply(StatusCode::OK, status),
        Err(error) => internal(error),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    action: String,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

/// The settings actions. Each answers with the new status; `begin` adds the secret and its
/// `otpauth_uri`, and `enable` and `recovery_codes` add the new `recovery_codes`, which are
/// shown this once.
#[derive(Clone, Copy, PartialEq)]
enum Action {
    /// A new secret to scan, replacing one not confirmed yet.
    Begin,
    /// Turns two-factor on with the password and a code from the new secret.
    Enable,
    /// Turns it off with the password and a code or a recovery code.
    Disable,
    /// New recovery codes, with the password and a code or a recovery code.
    RecoveryCodes,
}

impl Action {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "begin" => Some(Self::Begin),
            "enable" => Some(Self::Enable),
            "disable" => Some(Self::Disable),
            "recovery_codes" => Some(Self::RecoveryCodes),
            _ => None,
        }
    }
}

/// The password check every action but `begin` needs, counted like sign-in failures.
async fn confirm_password(
    app: &App,
    user: &str,
    headers: &HeaderMap,
    peer: SocketAddr,
    password: Option<&str>,
) -> Option<Response> {
    let client = access::client_ip(&app.config, headers, peer);
    let now = now_ms();
    if app.access.throttled(client, now) {
        return Some(refuse(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many attempts. Try again in one minute.",
        ));
    }
    let Some(password) = password else {
        return Some(refuse(StatusCode::BAD_REQUEST, "Enter your password."));
    };
    if !crate::accounts::password_matches(app, user, password).await {
        app.access.failure(client, now);
        return Some(refuse(StatusCode::FORBIDDEN, "That password is not right."));
    }
    None
}

async fn change(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(user): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !access::authorized(&app, &user, &headers) {
        return refuse(StatusCode::UNAUTHORIZED, "Sign in first.");
    }
    let Ok(change) = serde_json::from_slice::<Change>(&body) else {
        return refuse(StatusCode::BAD_REQUEST, "Send an action.");
    };
    let Some(action) = Action::parse(&change.action) else {
        return refuse(StatusCode::BAD_REQUEST, "Unknown action.");
    };
    match available(&app, &app.store.lock().unwrap().conn, &user) {
        Ok(true) => {}
        Ok(false) => {
            return refuse(
                StatusCode::CONFLICT,
                "Set a password first: two-factor authentication protects password sign-in.",
            );
        }
        Err(error) => return internal(error),
    }
    if action == Action::Begin {
        return begin(&app, &user);
    }
    if let Some(refused) =
        confirm_password(&app, &user, &headers, peer, change.password.as_deref()).await
    {
        return refused;
    }
    let Some(code) = change.code.as_deref() else {
        return refuse(StatusCode::BAD_REQUEST, ENTER_CODE);
    };
    let now = now_ms();
    if app.access.two_factor.locked(&user, now) {
        return refuse(StatusCode::TOO_MANY_REQUESTS, TOO_MANY_CODES);
    }
    let done = if action == Action::Enable {
        enable(&app, &user, code, now)
    } else {
        with_code(&app, &user, action, code, now)
    };
    match done {
        Ok(Some(mut answer)) => {
            app.access.two_factor.clear(&user);
            match status_of(&app, &user) {
                Ok(status) => {
                    answer
                        .as_object_mut()
                        .unwrap()
                        .extend(status.as_object().unwrap().clone());
                    reply(StatusCode::OK, answer)
                }
                Err(error) => internal(error),
            }
        }
        Ok(None) => {
            app.access.two_factor.failure(&user, now);
            refuse(StatusCode::FORBIDDEN, WRONG_CODE)
        }
        Err(error) => internal(error),
    }
}

fn begin(app: &App, user: &str) -> Response {
    let store = app.store.lock().unwrap();
    match enrolment(&store.conn, user) {
        Ok(Enrolment::On { .. }) => {
            return refuse(
                StatusCode::CONFLICT,
                "Two-factor authentication is already on.",
            );
        }
        Ok(_) => {}
        Err(error) => return internal(error),
    }
    let Ok(secret) = new_secret() else {
        return internal("two-factor randomness unavailable".into());
    };
    if let Err(error) = store.conn.execute(
        "insert or replace into two_factor (user, secret, state, last_step, updated_at)
         values (?1, ?2, 'pending', 0, ?3)",
        params![user, secret, now_ms()],
    ) {
        return internal(error.into());
    }
    drop(store);
    match status_of(app, user) {
        Ok(mut status) => {
            status["secret"] = secret.clone().into();
            status["otpauth_uri"] = otpauth_uri(user, &secret).into();
            reply(StatusCode::OK, status)
        }
        Err(error) => internal(error),
    }
}

/// Turns two-factor on when `code` comes from the pending secret. None when it does not.
fn enable(app: &App, user: &str, code: &str, now: i64) -> Result<Option<serde_json::Value>, Error> {
    let store = app.store.lock().unwrap();
    let Enrolment::Pending { secret } = enrolment(&store.conn, user)? else {
        return Ok(None);
    };
    let code = normalize(code);
    if code.len() != usize::from(DIGITS) {
        return Ok(None);
    }
    let Some(step) = totp_step(&secret, &code, now)? else {
        return Ok(None);
    };
    let transaction = store.conn.unchecked_transaction()?;
    transaction.execute(
        "update two_factor set state = 'on', last_step = ?2, updated_at = ?3 where user = ?1",
        params![user, i64::try_from(step).unwrap_or(i64::MAX), now],
    )?;
    let codes = new_recovery_codes(&transaction, user)?;
    transaction.commit()?;
    Ok(Some(serde_json::json!({"recovery_codes": codes})))
}

/// `Disable` or `RecoveryCodes` once `code` is a right code or recovery code. None when not.
fn with_code(
    app: &App,
    user: &str,
    action: Action,
    code: &str,
    now: i64,
) -> Result<Option<serde_json::Value>, Error> {
    let store = app.store.lock().unwrap();
    if check_code(&store.conn, user, code, now)? == Checked::Refused {
        return Ok(None);
    }
    if action == Action::RecoveryCodes {
        let codes = new_recovery_codes(&store.conn, user)?;
        return Ok(Some(serde_json::json!({"recovery_codes": codes})));
    }
    store
        .conn
        .execute("delete from two_factor where user = ?1", [user])?;
    store
        .conn
        .execute("delete from two_factor_recovery where user = ?1", [user])?;
    Ok(Some(serde_json::json!({})))
}

/// The pending secret as a QR code, for the settings page to show while setting up.
async fn qr(State(app): State<Arc<App>>, Path(user): Path<String>, headers: HeaderMap) -> Response {
    if !access::authorized(&app, &user, &headers) {
        return refuse(StatusCode::UNAUTHORIZED, "Sign in first.");
    }
    let secret = match enrolment(&app.store.lock().unwrap().conn, &user) {
        Ok(Enrolment::Pending { secret }) => secret,
        Ok(_) => return refuse(StatusCode::NOT_FOUND, "Start setting up first."),
        Err(error) => return internal(error),
    };
    let Ok(code) = qrcode::QrCode::new(otpauth_uri(&user, &secret)) else {
        return internal("the QR code could not be drawn".into());
    };
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(QR_PIXELS, QR_PIXELS)
        .build();
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        svg,
    )
        .into_response()
}

/// The smallest the QR code is drawn, in CSS pixels; phone cameras read it easily at this size.
const QR_PIXELS: u32 = 200;

pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new()
        .route(
            "/api/two-factor/{user}",
            get(status)
                .post(change)
                .layer(axum::extract::DefaultBodyLimit::max(4096)),
        )
        .route("/api/two-factor/{user}/qr.svg", get(qr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{app_with, call, cleanup, json_response, route_from, sign_up};
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::{Value, json};

    const PASSWORD: &str = "long enough pw";
    /// A moment well inside one 30-second step.
    const START_MS: i64 = 1_793_183_400_000;

    fn app() -> (Arc<App>, std::path::PathBuf) {
        crate::app::fix_now_ms(Some(START_MS));
        app_with(json!({
            "registration": true,
            "users": {"alice": {"token": "alice-token"}}
        }))
    }

    /// The code an authenticator app would show for `secret` now.
    fn code_now(secret: &str) -> String {
        code_at(secret, now_ms())
    }

    /// Moves the pinned clock on one step, so the next code is a new one.
    fn next_step() {
        let step_ms = i64::try_from(STEP_SECONDS).unwrap() * 1000;
        crate::app::fix_now_ms(Some(now_ms() + step_ms));
    }

    async fn post(app: &Arc<App>, uri: &str, body: Value, ip: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .header("x-ankiquest-csrf", "1")
            .body(Body::from(body.to_string()))
            .unwrap();
        json_response(route_from(app, request, &format!("{ip}:40000")).await).await
    }

    async fn device_sign_in(app: &Arc<App>, code: Option<&str>, ip: &str) -> (StatusCode, Value) {
        let mut body = json!({"user": "ana", "password": PASSWORD, "device": "Pixel"});
        if let Some(code) = code {
            body["code"] = code.into();
        }
        post(app, "/api/accounts/tokens", body, ip).await
    }

    /// Turns two-factor on for Ana, and returns the secret and the recovery codes.
    async fn turn_on(app: &Arc<App>, token: &str) -> (String, Vec<String>) {
        let (status, begun) = call(
            app,
            "POST",
            "/api/two-factor/ana",
            token,
            json!({"action": "begin"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{begun}");
        let secret = begun["secret"].as_str().unwrap().to_string();
        let (status, on) = call(
            app,
            "POST",
            "/api/two-factor/ana",
            token,
            json!({"action": "enable", "password": PASSWORD, "code": code_now(&secret)}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{on}");
        let codes = on["recovery_codes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|code| code.as_str().unwrap().to_string())
            .collect();
        next_step();
        (secret, codes)
    }

    #[test]
    fn codes_follow_rfc_6238_with_authenticator_app_defaults() {
        // RFC 6238 appendix B, SHA-1, the last six of its eight digits.
        let secret = Secret::from(b"12345678901234567890".to_vec()).to_base32();
        let totp = totp(&secret).unwrap();
        for (seconds, code) in [
            (59, "287082"),
            (1_111_111_109, "081804"),
            (1_234_567_890, "005924"),
        ] {
            assert_eq!(totp.generate(seconds).to_string(), code, "{seconds}");
        }
        assert_eq!((ALGORITHM_NAME, DIGITS, STEP_SECONDS), ("SHA1", 6, 30));
        assert_eq!(new_secret().unwrap().len(), 32, "160 bits in base32");
        assert_eq!(
            otpauth_uri("ana", "ABC"),
            "otpauth://totp/AnkiQuest:ana?secret=ABC&issuer=AnkiQuest&algorithm=SHA1&digits=6&period=30"
        );
    }

    #[test]
    fn recovery_codes_are_two_groups_of_five_and_typed_loosely() {
        let code = recovery_code().unwrap();
        assert_eq!(code.len(), 11);
        assert_eq!(&code[5..6], "-");
        assert!(
            code.bytes()
                .all(|b| b == b'-' || RECOVERY_ALPHABET.contains(&b))
        );
        assert_ne!(recovery_code().unwrap(), code);
        assert_eq!(
            recovery_hash(&code),
            recovery_hash(&format!(" {} ", code.to_uppercase().replace('-', " "))),
        );
    }

    #[tokio::test]
    async fn setting_up_shows_a_secret_and_turns_on_only_with_a_right_code() {
        let (app, path) = app();
        let token = sign_up(&app, "ana").await;
        let (status, off) = call(&app, "GET", "/api/two-factor/ana", &token, Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            off,
            json!({"available": true, "state": "off", "recovery_codes_left": 0})
        );

        // Begin hands out a secret, its address and a QR code, and nothing changes at sign-in.
        let (_, begun) = call(
            &app,
            "POST",
            "/api/two-factor/ana",
            &token,
            json!({"action": "begin"}),
        )
        .await;
        assert_eq!(begun["state"], "pending");
        let secret = begun["secret"].as_str().unwrap().to_string();
        assert!(
            begun["otpauth_uri"]
                .as_str()
                .unwrap()
                .contains(&format!("secret={secret}&"))
        );
        let qr = crate::test_support::route(
            &app,
            Request::builder()
                .uri("/api/two-factor/ana/qr.svg")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(qr.status(), StatusCode::OK);
        assert_eq!(qr.headers()[header::CONTENT_TYPE], "image/svg+xml");
        assert_eq!(
            device_sign_in(&app, None, "192.0.2.10").await.0,
            StatusCode::OK
        );

        // Enabling needs the password and a code from the new secret.
        let enable = |password: &str, code: &str| json!({"action": "enable", "password": password, "code": code});
        let (status, _) = call(
            &app,
            "POST",
            "/api/two-factor/ana",
            &token,
            enable("wrong password", &code_now(&secret)),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, refused) = call(
            &app,
            "POST",
            "/api/two-factor/ana",
            &token,
            enable(PASSWORD, "000000"),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(refused["error"], WRONG_CODE);
        let (status, on) = call(
            &app,
            "POST",
            "/api/two-factor/ana",
            &token,
            enable(PASSWORD, &code_now(&secret)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{on}");
        assert_eq!(on["state"], "on");
        assert_eq!(
            on["recovery_codes"].as_array().unwrap().len(),
            RECOVERY_CODES
        );
        assert_eq!(on["recovery_codes_left"], RECOVERY_CODES);

        // Recovery codes are kept only as hashes; the QR code is gone once set up.
        let first = on["recovery_codes"][0].as_str().unwrap();
        let stored: Vec<String> = app
            .store
            .lock()
            .unwrap()
            .conn
            .prepare("select code_hash from two_factor_recovery")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(stored.contains(&recovery_hash(first)));
        assert!(!stored.iter().any(|hash| hash.contains(first)));
        let (status, _) = call(
            &app,
            "GET",
            "/api/two-factor/ana/qr.svg",
            &token,
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = call(
            &app,
            "POST",
            "/api/two-factor/ana",
            &token,
            json!({"action": "begin"}),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "already on");
        cleanup(app, path);
    }

    #[tokio::test]
    async fn device_sign_in_asks_for_a_code_and_accepts_each_code_once() {
        let (app, path) = app();
        let token = sign_up(&app, "ana").await;
        let (secret, recovery) = turn_on(&app, &token).await;

        // The password alone is not enough; clients that know `second_factor` ask for a code.
        let (status, asked) = device_sign_in(&app, None, "192.0.2.10").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(asked, json!({"error": ENTER_CODE, "second_factor": true}));
        let (status, wrong) = device_sign_in(&app, Some("123456"), "192.0.2.10").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(wrong, json!({"error": WRONG_CODE, "second_factor": true}));
        let (status, wrong_password) = post(
            &app,
            "/api/accounts/tokens",
            json!({"user": "ana", "password": "wrong password", "code": code_now(&secret)}),
            "192.0.2.10",
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(
            wrong_password.get("second_factor").is_none(),
            "a wrong password says nothing about the second factor"
        );

        // A right code signs in once; the same code again does not.
        let code = code_now(&secret);
        let (status, signed_in) = device_sign_in(&app, Some(&code), "192.0.2.10").await;
        assert_eq!(status, StatusCode::OK, "{signed_in}");
        assert!(signed_in["token"].is_string());
        assert_eq!(
            device_sign_in(&app, Some(&code), "192.0.2.10").await.0,
            StatusCode::UNAUTHORIZED,
            "a code is good once"
        );

        // A recovery code, typed in capitals with a space, works once too.
        let typed = recovery[0].to_uppercase().replace('-', " ");
        assert_eq!(
            device_sign_in(&app, Some(&typed), "192.0.2.10").await.0,
            StatusCode::OK
        );
        assert_eq!(
            device_sign_in(&app, Some(&typed), "192.0.2.10").await.0,
            StatusCode::UNAUTHORIZED
        );
        let (_, status) = call(&app, "GET", "/api/two-factor/ana", &token, Value::Null).await;
        assert_eq!(status["recovery_codes_left"], RECOVERY_CODES - 1);

        // The device token from before two-factor was turned on still works.
        let (status, _) = call(&app, "GET", "/api/devices/ana", &token, Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        cleanup(app, path);
    }

    #[tokio::test]
    async fn website_sign_in_asks_for_a_code_too() {
        let (app, path) = app();
        let token = sign_up(&app, "ana").await;
        let (secret, _) = turn_on(&app, &token).await;
        let session = |code: Option<String>| {
            let mut body = json!({"user": "ana", "password": PASSWORD});
            if let Some(code) = code {
                body["code"] = code.into();
            }
            body
        };
        let (status, asked) = post(&app, "/auth/session", session(None), "192.0.2.10").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(asked["second_factor"], true);
        let (status, _) = post(
            &app,
            "/auth/session",
            session(Some("000000".into())),
            "192.0.2.10",
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let request = Request::builder()
            .method("POST")
            .uri("/auth/session")
            .header("content-type", "application/json")
            .header("x-ankiquest-csrf", "1")
            .body(Body::from(session(Some(code_now(&secret))).to_string()))
            .unwrap();
        let response = route_from(&app, request, "192.0.2.10:40000").await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(response.headers().contains_key(header::SET_COOKIE));
        cleanup(app, path);
    }

    #[tokio::test]
    async fn wrong_codes_lock_the_account_from_any_address() {
        let (app, path) = app();
        let token = sign_up(&app, "ana").await;
        let (secret, _) = turn_on(&app, &token).await;
        for attempt in 0..MAX_CODE_FAILURES {
            let ip = format!("198.51.100.{attempt}");
            assert_eq!(
                device_sign_in(&app, Some("000000"), &ip).await.0,
                StatusCode::UNAUTHORIZED
            );
        }
        let (status, locked) = device_sign_in(&app, Some(&code_now(&secret)), "203.0.113.9").await;
        assert_eq!(
            status,
            StatusCode::TOO_MANY_REQUESTS,
            "even a right code waits"
        );
        assert_eq!(locked["second_factor"], true);

        // The window passes.
        crate::app::fix_now_ms(Some(now_ms() + CODE_WINDOW_MS));
        assert_eq!(
            device_sign_in(&app, Some(&code_now(&secret)), "203.0.113.9")
                .await
                .0,
            StatusCode::OK
        );
        cleanup(app, path);
    }

    #[tokio::test]
    async fn turning_off_and_new_recovery_codes_need_the_password_and_a_code() {
        let (app, path) = app();
        let token = sign_up(&app, "ana").await;
        let (secret, recovery) = turn_on(&app, &token).await;
        let change = |action: &str, password: &str, code: &str| json!({"action": action, "password": password, "code": code});

        // New recovery codes replace the old ones.
        let (status, renewed) = call(
            &app,
            "POST",
            "/api/two-factor/ana",
            &token,
            change("recovery_codes", PASSWORD, &code_now(&secret)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{renewed}");
        assert_eq!(renewed["recovery_codes_left"], RECOVERY_CODES);
        let (status, _) = call(
            &app,
            "POST",
            "/api/two-factor/ana",
            &token,
            change("disable", PASSWORD, &recovery[0]),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "the old codes are gone");

        // Off needs the password, then a code; a new recovery code does.
        let fresh = renewed["recovery_codes"][0].as_str().unwrap();
        let (status, _) = call(
            &app,
            "POST",
            "/api/two-factor/ana",
            &token,
            change("disable", "wrong password", fresh),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, off) = call(
            &app,
            "POST",
            "/api/two-factor/ana",
            &token,
            change("disable", PASSWORD, fresh),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{off}");
        assert_eq!(
            off,
            json!({"available": true, "state": "off", "recovery_codes_left": 0})
        );
        assert_eq!(
            device_sign_in(&app, None, "192.0.2.10").await.0,
            StatusCode::OK
        );
        cleanup(app, path);
    }

    #[tokio::test]
    async fn a_failed_change_keeps_the_codes_and_the_enrolment() {
        let (app, path) = app();
        let token = sign_up(&app, "ana").await;
        let (secret, recovery) = turn_on(&app, &token).await;
        let change = |action: &str, code: &str| json!({"action": action, "password": PASSWORD, "code": code});
        let sql = |statement: &str| {
            app.store
                .lock()
                .unwrap()
                .conn
                .execute_batch(statement)
                .unwrap();
        };
        let left = || {
            usize::try_from(recovery_left(&app.store.lock().unwrap().conn, "ana").unwrap()).unwrap()
        };

        // Writing the new recovery codes fails after the old ones were deleted.
        sql(
            "create temp trigger fail before insert on two_factor_recovery
             begin select raise(abort, 'disk full'); end",
        );
        let (status, _) = call(
            &app,
            "POST",
            "/api/two-factor/ana",
            &token,
            change("recovery_codes", &recovery[0]),
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        sql("drop trigger fail");
        assert_eq!(
            left(),
            RECOVERY_CODES,
            "the old codes and the one sent survive"
        );

        // Turning off fails after the enrolment was deleted.
        sql(
            "create temp trigger fail before delete on two_factor_recovery
             begin select raise(abort, 'disk full'); end",
        );
        let code = code_now(&secret);
        let (status, _) = call(
            &app,
            "POST",
            "/api/two-factor/ana",
            &token,
            change("disable", &code),
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        sql("drop trigger fail");
        assert!(enabled(&app.store.lock().unwrap().conn, "ana").unwrap());

        // Nothing was spent: the same codes still work.
        let (status, renewed) = call(
            &app,
            "POST",
            "/api/two-factor/ana",
            &token,
            change("recovery_codes", &recovery[0]),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{renewed}");
        let (status, off) = call(
            &app,
            "POST",
            "/api/two-factor/ana",
            &token,
            change("disable", &code),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{off}");
        cleanup(app, path);
    }

    #[tokio::test]
    async fn players_without_a_password_cannot_turn_it_on() {
        let (app, path) = app();
        let (status, configured) = call(
            &app,
            "GET",
            "/api/two-factor/alice",
            "alice-token",
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(configured["available"], false);
        let (status, _) = call(
            &app,
            "POST",
            "/api/two-factor/alice",
            "alice-token",
            json!({"action": "begin"}),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);

        // An account that signs in only with a provider has no password to protect.
        crate::accounts::create(&app.store.lock().unwrap().conn, "bo", "Bo", "", 0).unwrap();
        app.accounts.insert("bo", "Bo");
        let bo = app.issue_token("bo", "Phone").unwrap();
        let (status, _) = call(
            &app,
            "POST",
            "/api/two-factor/bo",
            &bo,
            json!({"action": "begin"}),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        let (status, _) = call(
            &app,
            "GET",
            "/api/two-factor/ana",
            "alice-token",
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "only the player's own");
        cleanup(app, path);
    }

    #[tokio::test]
    async fn deleting_the_account_removes_its_two_factor_rows() {
        let (app, path) = app();
        let token = sign_up(&app, "ana").await;
        turn_on(&app, &token).await;
        {
            let store = app.store.lock().unwrap();
            store
                .conn
                .execute("delete from accounts where user = 'ana'", [])
                .unwrap();
            for table in ["two_factor", "two_factor_recovery"] {
                let left: i64 = store
                    .conn
                    .query_row(&format!("select count(*) from {table}"), [], |r| r.get(0))
                    .unwrap();
                assert_eq!(left, 0, "{table}");
            }
        }
        cleanup(app, path);
    }
}
