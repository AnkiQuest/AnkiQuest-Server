//! The server's JSON configuration and the checks it gets at startup.

use crate::app::now_ms;
use crate::game::{self, Week};
use crate::store::Error;
use crate::{admin, oauth, privacy};
use chrono::Datelike;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Deserialize, Clone, Default)]
pub(crate) struct UserConfig {
    pub(crate) display: Option<String>,
    pub(crate) ntfy_topic: Option<String>,
    /// Read through [`Config::token`], which honours [`ConfiguredTokens`].
    token: Option<String>,
    token_file: Option<PathBuf>,
}

impl UserConfig {
    /// Whether the configuration gives this player a token, inline or as a file.
    fn has_token(&self) -> bool {
        self.token.is_some() || self.token_file.is_some()
    }
}

/// Whether players in `users` sign in with a token from the configuration: a user base
/// declared in the configuration instead of accounts. Operators opt in; off by default.
#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConfiguredTokens {
    /// Players sign in with accounts only; a configured token is a startup error.
    #[default]
    Disabled,
    /// Configured players sign in with their `token` or `token_file`.
    Enabled,
}

#[derive(Deserialize, Clone)]
pub(crate) struct Config {
    #[serde(default = "default_addr")]
    pub(crate) addr: String,
    pub(crate) sync_base: Option<PathBuf>,
    #[serde(default = "default_state")]
    pub(crate) state_dir: PathBuf,
    pub(crate) ntfy: Option<String>,
    pub(crate) public_url: Option<String>,
    #[serde(default)]
    pub(crate) private_site: bool,
    pub(crate) site_password_file: Option<PathBuf>,
    #[serde(default)]
    pub(crate) site_trust_proxy: bool,
    #[serde(default = "default_week_timezone")]
    pub(crate) week_timezone: String,
    #[serde(default = "default_week_rollover_hour")]
    pub(crate) week_rollover_hour: u32,
    #[serde(default)]
    pub(crate) competition_start_date: Option<String>,
    #[serde(default)]
    pub(crate) review_weighting: game::ReviewWeighting,
    #[serde(default = "default_log_retention_days")]
    pub(crate) log_retention_days: u32,
    /// Review minutes a day needs to count toward the streak; 0 lets any review count.
    #[serde(default)]
    pub(crate) streak_minutes: u32,
    #[serde(default)]
    pub(crate) users: HashMap<String, UserConfig>,
    #[serde(default)]
    pub(crate) configured_tokens: ConfiguredTokens,
    /// Lets anyone create an account with a username and password.
    #[serde(default)]
    pub(crate) registration: bool,
    /// Players who can open the dev console: announcements, statistics and recent errors.
    #[serde(default)]
    pub(crate) devs: Vec<String>,
    /// Who runs this server, for the privacy notice.
    #[serde(default)]
    pub(crate) privacy: privacy::Notice,
    /// Sign-in with Google, GitHub and GitLab, each off until configured.
    #[serde(default)]
    pub(crate) oauth: oauth::Settings,
}

fn default_log_retention_days() -> u32 {
    admin::DEFAULT_LOG_RETENTION_DAYS
}

fn default_week_timezone() -> String {
    "UTC".into()
}

fn default_week_rollover_hour() -> u32 {
    Week::RESET_HOUR
}

fn default_addr() -> String {
    "127.0.0.1:8097".into()
}

fn default_state() -> PathBuf {
    "state".into()
}

impl Config {
    pub(crate) fn settings(&self) -> admin::Settings {
        admin::Settings {
            review_weighting: self.review_weighting,
            log_retention_days: self.log_retention_days,
            streak_minutes: self.streak_minutes,
        }
    }

    /// Reads the configuration at `path`, refusing tokens the operator did not opt into.
    pub(crate) fn read(path: &str) -> Result<Self, Error> {
        let config: Self = serde_json::from_slice(
            &std::fs::read(path).map_err(|e| format!("cannot read config {path}: {e}"))?,
        )?;
        config.check_tokens()?;
        Ok(config)
    }

    /// Rejects a `token` or `token_file` while [`ConfiguredTokens::Disabled`], so an operator
    /// never loses or keeps those sign-ins without deciding to.
    fn check_tokens(&self) -> Result<(), Error> {
        let named = self.token_players();
        if self.configured_tokens == ConfiguredTokens::Enabled || named.is_empty() {
            return Ok(());
        }
        Err(format!(
            "users {} have a token, but configured_tokens is \"disabled\": set \
             configured_tokens to \"enabled\" (NixOS: services.ankiquest.configuredTokens) \
             to keep these sign-ins, or remove their token and token_file",
            named.join(", ")
        )
        .into())
    }

    /// The token `user` signs in with from the configuration: none unless the operator opted
    /// in, and never an empty one.
    pub(crate) fn token(&self, user: &str) -> Option<&str> {
        self.tokens()
            .find(|(name, _)| *name == user)
            .map(|(_, token)| token)
    }

    /// Each configured player and the token they sign in with; see [`Config::token`].
    pub(crate) fn tokens(&self) -> impl Iterator<Item = (&str, &str)> {
        let enabled = self.configured_tokens == ConfiguredTokens::Enabled;
        self.users
            .iter()
            .filter(move |_| enabled)
            .filter_map(|(name, user)| Some((name.as_str(), user.token.as_deref()?)))
            .filter(|(_, token)| !token.is_empty())
    }

    /// Players the configuration gives a token, inline or as a file, whether loaded or not.
    pub(crate) fn token_players(&self) -> Vec<String> {
        let mut players: Vec<String> = self
            .users
            .iter()
            .filter(|(_, user)| user.has_token())
            .map(|(name, _)| name.clone())
            .collect();
        players.sort_unstable();
        players
    }

    /// Gives `user` the configured token `token`, as `token` in the file would.
    #[cfg(test)]
    pub(crate) fn set_token(&mut self, user: &str, token: &str) {
        self.users.entry(user.into()).or_default().token = Some(token.into());
    }

    /// Rejects a competition start outside 1970 to today.
    pub(crate) fn check(&self) -> Result<(), Error> {
        if let Some(date) = &self.competition_start_date {
            let date = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
                .map_err(|_| "competition_start_date must be YYYY-MM-DD")?;
            if date.year() < 1970
                || date
                    > chrono::DateTime::from_timestamp_millis(now_ms())
                        .unwrap()
                        .date_naive()
            {
                return Err("competition_start_date must be between 1970 and today".into());
            }
        }
        Ok(())
    }

    /// Reads each `token_file` into its user's token.
    pub(crate) fn load_tokens(&mut self) -> Result<(), Error> {
        for (name, user) in &mut self.users {
            if let Some(file) = &user.token_file {
                let token = std::fs::read_to_string(file)
                    .map_err(|e| format!("cannot read token for {name}: {e}"))?;
                user.token = Some(token.trim().to_string());
            }
        }
        Ok(())
    }

    /// The competition week from `week_timezone` and `week_rollover_hour`.
    pub(crate) fn week(&self) -> Result<Week, Error> {
        let week = Week {
            tz: self
                .week_timezone
                .parse()
                .map_err(|e| format!("week_timezone {:?}: {e}", self.week_timezone))?,
            rollover_hour: self.week_rollover_hour,
        };
        if week.rollover_hour > 23 {
            return Err("week_rollover_hour must be between 0 and 23".into());
        }
        Ok(week)
    }
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn a_config_with_the_removed_remind_hour_still_loads() {
        let config: Config = serde_json::from_str(r#"{"remind_hour": 20}"#).unwrap();
        assert!(config.check().is_ok());
    }

    /// Reads `json` as a configuration file, the way the server starts.
    fn read(json: serde_json::Value) -> Result<Config, String> {
        let (store, dir) = crate::test_support::temporary_store();
        let path = dir.join("config.json");
        std::fs::write(&path, json.to_string()).unwrap();
        let config = Config::read(path.to_str().unwrap()).map_err(|e| e.to_string());
        crate::test_support::cleanup(store, dir);
        config
    }

    #[test]
    fn configured_tokens_stop_the_server_without_the_opt_in() {
        // Off by default: a token or a token file alone is a startup error.
        for user in [
            serde_json::json!({"token": "hill-secret"}),
            serde_json::json!({"token_file": "hill.token"}),
        ] {
            let error = read(serde_json::json!({"users": {"hill": user}}))
                .err()
                .expect("a configured token without the opt-in must not load");
            assert!(error.contains("hill"), "{error}");
            assert!(error.contains("configured_tokens"), "{error}");
        }
        let error = read(serde_json::json!({
            "configured_tokens": "disabled",
            "users": {"hill": {"token": "hill-secret"}}
        }))
        .err()
        .expect("disabled refuses configured tokens");
        assert!(error.contains("configured_tokens"), "{error}");
    }

    #[test]
    fn configured_tokens_load_with_the_opt_in() {
        let config = read(serde_json::json!({
            "configured_tokens": "enabled",
            "users": {"hill": {"token": "hill-secret"}}
        }));
        assert!(config.is_ok(), "{:?}", config.err());
    }

    #[test]
    fn players_without_tokens_need_no_opt_in() {
        let config = read(serde_json::json!({
            "users": {"hill": {"display": "Hill", "ntfy_topic": "hill-topic"}}
        }));
        assert!(config.is_ok(), "{:?}", config.err());
    }
}
