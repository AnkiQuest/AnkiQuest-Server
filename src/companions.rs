//! A player's visual study companion, shared by the website and installed clients.
use crate::access::require_owner;
use crate::game::{Event, Profile};
use crate::i18n::english;
use crate::{
    App,
    store::{Error, Store},
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

pub const DEFAULT: &str = "aki";

/// The cow, shown as Bluma.
pub(crate) const COW: &str = "cow";

/// Every stored companion. Keep [`TABLE`]'s check in step; changing it needs a migration step.
const COMPANIONS: [&str; 4] = ["aki", "ankilope", COW, "none"];

pub(crate) const TABLE: &str = "create table if not exists companion_preferences (
    user text primary key,
    companion text not null check (companion in ('aki', 'ankilope', 'cow', 'none'))
) without rowid;";

pub fn valid(value: &str) -> bool {
    COMPANIONS.contains(&value)
}

/// Something a player unlocks once and keeps. Unlocks are not settings: settings sync does not
/// touch them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unlock {
    /// Ankilope, earned at a level.
    Ankilope,
    /// The cow, shown as Bluma, earned by hours of study.
    Cow,
}

/// Whether the clients released before milestones offered a companion to everyone, and so
/// have its art.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Released {
    /// Offered to every player, so a save from those clients is still accepted.
    Offered,
    /// Never offered by them, nor drawn: their inbox leaves out its moment ([`Drawable`]).
    Hidden,
}

impl Unlock {
    const ALL: [Unlock; 2] = [Unlock::Ankilope, Unlock::Cow];

    pub(crate) fn id(self) -> &'static str {
        match self {
            Unlock::Ankilope => "ankilope",
            Unlock::Cow => COW,
        }
    }

    /// The companion's name, as both clients show it.
    fn name(self) -> &'static str {
        match self {
            Unlock::Ankilope => "Ankilope",
            Unlock::Cow => "Bluma",
        }
    }

    fn released(self) -> Released {
        match self {
            Unlock::Ankilope => Released::Offered,
            Unlock::Cow => Released::Hidden,
        }
    }

    fn parse(id: &str) -> Option<Unlock> {
        Self::ALL.into_iter().find(|unlock| unlock.id() == id)
    }

    /// The moment a player gets when this unlock is granted.
    fn event(self) -> Event {
        let milestone = milestone(self);
        let goal = milestone.goal.to_string();
        let body = match milestone.kind {
            Kind::Level => english("You reached level {0}.", &[&goal]),
            Kind::StudyHours => english("You have studied for {0} hours in total.", &[&goal]),
        };
        Event {
            key: format!("{MOMENT_PREFIX}{}", self.id()),
            title: english("{0} joined you!", &[self.name()]),
            body,
        }
    }
}

/// The unlock `companion` needs before a player may choose it.
fn needed(companion: &str) -> Option<Unlock> {
    Unlock::parse(companion)
}

/// Whose art draws `companion`. The cow has no art yet (#88), so it borrows Ankilope's, which
/// `cards::cow_art` tints purple.
pub(crate) fn art_owner(companion: &str) -> &str {
    if companion == COW {
        "ankilope"
    } else {
        companion
    }
}

/// The companion an unlock moment's event key names, such as `cow` for `companion:cow`.
pub(crate) fn of_moment(key: &str) -> Option<&'static str> {
    key.strip_prefix(MOMENT_PREFIX)
        .and_then(Unlock::parse)
        .map(Unlock::id)
}

/// How unlock moments' event keys start; the rest is the companion.
const MOMENT_PREFIX: &str = "companion:";

/// A client that sends this header with [`MILESTONES`] shows locked companions itself, so the
/// server refuses a locked one. Clients from before milestones don't send it.
const CLIENT_HEADER: &str = "x-ankiquest-companions";
const MILESTONES: &str = "milestones";

/// A client lists in this header the companions it can draw, such as `aki, ankilope, cow`, so
/// the inbox leaves out unlock moments about any other. Clients released before it send none.
const ART_HEADER: &str = "x-ankiquest-companion-art";

/// The capability that says this server reads [`ART_HEADER`].
pub(crate) const ART_FEATURE: &str = "companion_art";

/// The unlocks whose moments a client can show, so that it never announces a companion it
/// would draw with another's art. One bit per unlock, in [`Unlock::ALL`]'s order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Drawable(u8);

impl Drawable {
    /// Every unlock, for callers that draw them all.
    pub(crate) fn all() -> Self {
        Self::keeping(|_| true)
    }

    /// The unlocks a request's client names in [`ART_HEADER`]. A client without the header
    /// was released before it and draws what those clients offered: Ankilope, not the cow.
    pub(crate) fn of(headers: &HeaderMap) -> Self {
        let Some(value) = headers.get(ART_HEADER) else {
            return Self::keeping(|unlock| unlock.released() == Released::Offered);
        };

        // Unknown and unreadable names are left out: the client cannot be shown them anyway.
        let named: Vec<&str> = value
            .to_str()
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .collect();
        Self::keeping(|unlock| named.contains(&unlock.id()))
    }

    /// The unlocks `keep` accepts.
    fn keeping(keep: impl Fn(Unlock) -> bool) -> Self {
        let bits = Unlock::ALL
            .into_iter()
            .enumerate()
            .filter(|(_, unlock)| keep(*unlock))
            .fold(0, |bits, (index, _)| bits | 1 << index);
        Drawable(bits)
    }

    fn contains(self, unlock: Unlock) -> bool {
        let index = Unlock::ALL.iter().position(|known| *known == unlock);
        index.is_some_and(|index| self.0 & 1 << index != 0)
    }

    /// SQL that is true for the notifications in `table` such a client may list: everything
    /// but the player's own unlock moments about companions it cannot draw. A batch of
    /// unlocks ("4 new unlocks") has no win naming them, so it shows only to a client that
    /// draws every live unlock.
    pub(crate) fn sql(self, table: &str) -> String {
        let keys: Vec<String> = Unlock::ALL
            .into_iter()
            .filter(|unlock| self.contains(*unlock))
            .map(|unlock| format!("'{MOMENT_PREFIX}{}'", unlock.id()))
            .collect();
        let batch = i32::from(live().all(|unlock| self.contains(unlock)));
        format!(
            "({table}.kind != 'celebration' or {table}.topic is not '{TOPIC}'
              or ({table}.win_id is null and {batch})
              or (select w.key from wins w where w.id = {table}.win_id) in ({}))",
            keys.join(",")
        )
    }
}

/// The unlocks players can earn now, as the fixture says; planned ones have no art yet.
fn live() -> impl Iterator<Item = Unlock> {
    static LIVE: OnceLock<Vec<Unlock>> = OnceLock::new();
    let live = LIVE.get_or_init(|| {
        let rules: Rules = serde_json::from_str(RULES).expect("the unlock fixture parses");
        let is_live = |unlock: &Unlock| {
            let rule = rules
                .companions
                .iter()
                .find(|rule| rule.companion == unlock.id());
            rule.is_some_and(|rule| rule.status == Status::Live)
        };
        Unlock::ALL.into_iter().filter(is_live).collect()
    });
    live.iter().copied()
}

/// The inbox topic of unlock moments: [`MOMENT_PREFIX`] without its colon.
const TOPIC: &str = "companion";

/// The conditions both clients show, defined once with the cases they test against.
const RULES: &str = include_str!("../tests/fixtures/companion_unlock.json");

/// What a milestone counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Level,
    StudyHours,
}

/// Whether a companion shows before it is earned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Visibility {
    Shown,
    Secret,
}

/// Whether the fixture's companion can be earned yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Live,
    /// No art yet: listed so both clients agree on it, but nobody earns it.
    Planned,
}

/// A fixture entry as written. Planned entries may count kinds this server does not measure.
#[derive(Deserialize)]
struct Rule {
    companion: String,
    kind: String,
    goal: u64,
    secret: bool,
    status: Status,
}

#[derive(Deserialize)]
struct Rules {
    companions: Vec<Rule>,
}

/// How one unlock is earned.
#[derive(Clone, Copy, Debug)]
struct Milestone {
    unlock: Unlock,
    kind: Kind,
    goal: u64,
    visibility: Visibility,
}

/// The live milestones of a fixture, in its order.
fn parse(source: &str) -> Vec<Milestone> {
    let rules: Rules = serde_json::from_str(source).expect("the unlock fixture parses");
    rules
        .companions
        .into_iter()
        .filter(|rule| rule.status == Status::Live)
        .map(|rule| Milestone {
            unlock: Unlock::parse(&rule.companion).expect("a live companion is an unlock"),
            kind: serde_json::from_value(serde_json::Value::String(rule.kind))
                .expect("a live companion counts a known kind"),
            goal: rule.goal,
            visibility: if rule.secret {
                Visibility::Secret
            } else {
                Visibility::Shown
            },
        })
        .collect()
}

fn milestones() -> &'static [Milestone] {
    static PARSED: OnceLock<Vec<Milestone>> = OnceLock::new();
    PARSED.get_or_init(|| parse(RULES))
}

fn milestone(unlock: Unlock) -> Milestone {
    *milestones()
        .iter()
        .find(|milestone| milestone.unlock == unlock)
        .expect("every unlock has a milestone")
}

/// What milestones count for one player, derived from their reviews and never stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Measures {
    level: u64,
    study_hours: u64,
}

impl Measures {
    /// A player without reviews is at the first level with nothing studied.
    fn of(profile: Option<&Profile>) -> Self {
        match profile {
            Some(profile) => Measures {
                level: profile.level,
                study_hours: profile.lifetime.hours,
            },
            None => Measures {
                level: crate::game::level_for(0).0,
                study_hours: 0,
            },
        }
    }

    fn value(self, kind: Kind) -> u64 {
        match kind {
            Kind::Level => self.level,
            Kind::StudyHours => self.study_hours,
        }
    }

    fn reached(self, milestone: &Milestone) -> bool {
        self.value(milestone.kind) >= milestone.goal
    }
}

/// How far a player is toward a locked companion.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Progress {
    kind: Kind,
    value: u64,
    goal: u64,
}

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(TABLE)?;
    conn.execute_batch(
        "create table if not exists companion_unlocks (
            user text not null,
            unlock text not null,
            unlocked_at integer not null,
            primary key (user, unlock)
        ) without rowid;",
    )?;
    Ok(())
}

impl Store {
    pub fn companion(&self, user: &str) -> Result<String, Error> {
        Ok(self
            .conn
            .query_row(
                "select companion from companion_preferences where user = ?1",
                [user],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or_else(|| DEFAULT.into()))
    }

    pub fn set_companion(&self, user: &str, companion: &str) -> Result<(), Error> {
        self.conn.execute(
            "insert into companion_preferences (user, companion) values (?1, ?2)
             on conflict (user) do update set companion = excluded.companion",
            params![user, companion],
        )?;
        Ok(())
    }

    /// What `user` has unlocked, in [`Unlock::ALL`]'s order. Ids this server no longer knows
    /// are left out.
    pub(crate) fn unlocks(&self, user: &str) -> Result<Vec<Unlock>, Error> {
        let mut stmt = self
            .conn
            .prepare("select unlock from companion_unlocks where user = ?1")?;
        let ids: Vec<String> = stmt
            .query_map([user], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(Unlock::ALL
            .into_iter()
            .filter(|unlock| ids.iter().any(|id| id == unlock.id()))
            .collect())
    }

    /// Records `unlock` for `user`; unlocking again keeps the first time. True when it is new.
    pub(crate) fn add_unlock(&self, user: &str, unlock: Unlock, at_ms: i64) -> Result<bool, Error> {
        let added = self.conn.execute(
            "insert into companion_unlocks (user, unlock, unlocked_at) values (?1, ?2, ?3)
             on conflict (user, unlock) do nothing",
            params![user, unlock.id(), at_ms],
        )?;
        Ok(added > 0)
    }

    /// Grants every milestone `measures` reach and answers what `user` has unlocked.
    fn grant(&self, user: &str, measures: Measures, at_ms: i64) -> Result<Vec<Unlock>, Error> {
        for milestone in milestones() {
            if measures.reached(milestone) {
                self.add_unlock(user, milestone.unlock, at_ms)?;
            }
        }
        self.unlocks(user)
    }
}

/// Grants what `profile` has earned and answers the unlock moments `user` has, one per unlock
/// kept, whenever it was granted. Each is announced once: the caller queues or marks them seen.
pub(crate) fn moments(store: &Store, user: &str, profile: &Profile) -> Result<Vec<Event>, Error> {
    let unlocks = store.grant(user, Measures::of(Some(profile)), crate::app::now_ms())?;
    Ok(unlocks.into_iter().map(Unlock::event).collect())
}

/// Whether the client sent [`CLIENT_HEADER`]: see there.
fn shows_locks(headers: &HeaderMap) -> bool {
    headers
        .get(CLIENT_HEADER)
        .is_some_and(|value| value.as_bytes() == MILESTONES.as_bytes())
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Preference {
    pub companion: String,
}

pub async fn get(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Preference>, StatusCode> {
    require_owner(&app, &user, &headers)?;
    let companion = app
        .store
        .lock()
        .unwrap()
        .companion(&user)
        .map_err(crate::store_error)?;
    Ok(Json(Preference { companion }))
}

pub async fn set(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
    Json(preference): Json<Preference>,
) -> Result<Json<Preference>, StatusCode> {
    require_owner(&app, &user, &headers)?;
    if !valid(&preference.companion) {
        return Err(StatusCode::BAD_REQUEST);
    }

    // A companion that needs an unlock waits for it; a milestone reached since the last
    // upload counts.
    if let Some(unlock) = needed(&preference.companion)
        && !current(&app, &user)?.unlocks.contains(&unlock.id())
    {
        admit_released(&app, &user, unlock, &headers)?;
    }
    app.store
        .lock()
        .unwrap()
        .set_companion(&user, &preference.companion)
        .map_err(crate::store_error)?;
    Ok(Json(preference))
}

/// Clients released before milestones offered Ankilope to everyone and show any refusal as an
/// error. Their save is still accepted and grants the unlock, without a moment, so the player
/// keeps what they chose. A client that shows locks, or a companion they never offered, is
/// refused.
fn admit_released(
    app: &App,
    user: &str,
    unlock: Unlock,
    headers: &HeaderMap,
) -> Result<(), StatusCode> {
    if shows_locks(headers) || unlock.released() == Released::Hidden {
        return Err(StatusCode::CONFLICT);
    }
    let store = app.store.lock().unwrap();
    store
        .add_unlock(user, unlock, crate::app::now_ms())
        .map_err(crate::store_error)?;
    store
        .mark_seen(user, &unlock.event().key)
        .map_err(crate::store_error)?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Unlocks {
    unlocks: Vec<&'static str>,
    /// Each locked companion that is not secret, by id.
    progress: BTreeMap<&'static str, Progress>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnlockRequest {
    unlock: String,
}

/// `unlocks` with the progress toward every other companion that shows while locked.
fn listed(unlocks: Vec<Unlock>, measures: Measures) -> Unlocks {
    let progress = milestones()
        .iter()
        .filter(|milestone| milestone.visibility == Visibility::Shown)
        .filter(|milestone| !unlocks.contains(&milestone.unlock))
        .map(|milestone| {
            let progress = Progress {
                kind: milestone.kind,
                value: measures.value(milestone.kind),
                goal: milestone.goal,
            };
            (milestone.unlock.id(), progress)
        })
        .collect();
    Unlocks {
        unlocks: unlocks.into_iter().map(Unlock::id).collect(),
        progress,
    }
}

/// Grants what `user` has earned and lists it with the progress toward the rest. The profile
/// is worked out before the store is locked, as everywhere else.
fn current(app: &App, user: &str) -> Result<Unlocks, StatusCode> {
    let measures = Measures::of(app.profile(user).as_ref());
    let unlocks = app
        .store
        .lock()
        .unwrap()
        .grant(user, measures, crate::app::now_ms())
        .map_err(crate::store_error)?;
    Ok(listed(unlocks, measures))
}

pub async fn get_unlocks(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Unlocks>, StatusCode> {
    require_owner(&app, &user, &headers)?;
    current(&app, &user).map(Json)
}

/// Every unlock is a milestone the server grants, so a client cannot add one: asking for one
/// already granted answers the list, as it did when clients reported the cow; any other is
/// `409`. Unknown ids, and companions the fixture lists as planned, are `400`.
pub async fn add_unlock(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
    Json(request): Json<UnlockRequest>,
) -> Result<Json<Unlocks>, StatusCode> {
    require_owner(&app, &user, &headers)?;
    let unlock = Unlock::parse(&request.unlock).ok_or(StatusCode::BAD_REQUEST)?;
    let answer = current(&app, &user)?;
    if !answer.unlocks.contains(&unlock.id()) {
        return Err(StatusCode::CONFLICT);
    }
    Ok(Json(answer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{call, cleanup, fixture};
    use serde_json::{Value, json};

    const HOUR_MS: i64 = 3_600_000;

    fn measures(level: u64, study_hours: u64) -> Measures {
        Measures { level, study_hours }
    }

    #[test]
    fn choice_is_per_player_and_survives_reopening_the_store() {
        let (store, path) = crate::test_support::temporary_store();
        assert_eq!(store.companion("alice").unwrap(), DEFAULT);
        store.set_companion("alice", "ankilope").unwrap();
        store.set_companion("bob", "none").unwrap();
        drop(store);
        let reopened = Store::open(&path).unwrap();
        assert_eq!(reopened.companion("alice").unwrap(), "ankilope");
        assert_eq!(reopened.companion("bob").unwrap(), "none");
        assert_eq!(reopened.companion("carol").unwrap(), DEFAULT);
    }

    #[test]
    fn unlocks_are_per_player_kept_once_and_survive_reopening_the_store() {
        let (store, path) = crate::test_support::temporary_store();
        assert!(store.unlocks("alice").unwrap().is_empty());
        assert!(store.add_unlock("alice", Unlock::Cow, 1).unwrap());
        assert!(!store.add_unlock("alice", Unlock::Cow, 2).unwrap());
        drop(store);
        let reopened = Store::open(&path).unwrap();
        assert_eq!(reopened.unlocks("alice").unwrap(), [Unlock::Cow]);
        assert!(reopened.unlocks("bob").unwrap().is_empty());
        let first: i64 = reopened
            .conn
            .query_row("select unlocked_at from companion_unlocks", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(first, 1, "unlocking again keeps the first time");
    }

    #[test]
    fn every_live_companion_in_the_fixture_is_an_unlock_with_one_milestone() {
        let rules: Value = serde_json::from_str(RULES).unwrap();
        for unlock in Unlock::ALL {
            let found = milestones().iter().filter(|m| m.unlock == unlock).count();
            assert_eq!(found, 1, "{unlock:?}");
            assert_eq!(Unlock::parse(unlock.id()), Some(unlock));
            assert_eq!(needed(unlock.id()), Some(unlock));
        }
        assert_eq!(milestones().len(), Unlock::ALL.len());

        // Planned companions have no art yet, so this server knows nothing of them.
        for rule in rules["companions"].as_array().unwrap() {
            let planned = rule["status"] == "planned";
            let id = rule["companion"].as_str().unwrap();
            assert_eq!(Unlock::parse(id).is_none(), planned, "{id}");
            assert_eq!(valid(id), !planned, "{id}");
        }
        assert_eq!(needed("aki"), None);
        assert_eq!(needed("none"), None);
        assert_eq!(Unlock::parse("Cow"), None);
    }

    #[test]
    fn the_fixture_checks_decide_each_unlock() {
        let rules: Value = serde_json::from_str(RULES).unwrap();
        let checks = rules["checks"].as_array().unwrap();
        assert!(!checks.is_empty());
        for check in checks {
            let unlock = Unlock::parse(check["companion"].as_str().unwrap()).unwrap();
            let value = check["value"].as_u64().unwrap();
            let milestone = milestone(unlock);
            let measured = match milestone.kind {
                Kind::Level => measures(value, 0),
                Kind::StudyHours => measures(1, value),
            };
            assert_eq!(
                measured.reached(&milestone),
                check["unlocks"].as_bool().unwrap(),
                "{}",
                check["name"]
            );
        }
    }

    #[test]
    fn level_30_unlocks_ankilope_for_good() {
        let (store, path) = crate::test_support::temporary_store();
        assert!(store.grant("hill", measures(29, 0), 1).unwrap().is_empty());
        assert_eq!(
            store.grant("hill", measures(30, 0), 2).unwrap(),
            [Unlock::Ankilope]
        );

        // A level reset or less study later takes nothing back.
        assert_eq!(
            store.grant("hill", measures(1, 0), 3).unwrap(),
            [Unlock::Ankilope]
        );
        assert_eq!(
            store.grant("hill", measures(1, 10), 4).unwrap(),
            Unlock::ALL
        );
        cleanup(store, path);
    }

    #[test]
    fn progress_lists_only_locked_companions_that_show() {
        let listed = |unlocks: Vec<Unlock>, measured| {
            serde_json::to_value(super::listed(unlocks, measured)).unwrap()
        };
        assert_eq!(
            listed(vec![], measures(3, 6)),
            json!({"unlocks": [], "progress": {
                "ankilope": {"kind": "level", "value": 3, "goal": 30},
                "cow": {"kind": "study_hours", "value": 6, "goal": 10}
            }})
        );
        assert_eq!(
            listed(vec![Unlock::Cow], measures(3, 12)),
            json!({"unlocks": ["cow"], "progress": {
                "ankilope": {"kind": "level", "value": 3, "goal": 30}
            }})
        );
        assert_eq!(
            listed(Unlock::ALL.to_vec(), measures(40, 12)),
            json!({"unlocks": ["ankilope", "cow"], "progress": {}})
        );
        assert_eq!(Measures::of(None), measures(1, 0));
    }

    #[test]
    fn unlock_moments_name_the_companion_and_its_milestone() {
        let ankilope = Unlock::Ankilope.event();
        assert_eq!(ankilope.key, "companion:ankilope");
        assert_eq!(ankilope.title, "Ankilope joined you!");
        assert_eq!(ankilope.body, "You reached level 30.");
        let cow = Unlock::Cow.event();
        assert_eq!(cow.key, "companion:cow");
        assert_eq!(cow.title, "Bluma joined you!");
        assert_eq!(cow.body, "You have studied for 10 hours in total.");
        assert_eq!(of_moment("companion:cow"), Some(COW));
        assert_eq!(of_moment("companion:horse"), None);
        assert_eq!(of_moment("level:3"), None);
        assert_eq!(
            crate::wins::own_words(&cow.title, &cow.body),
            (
                "Bluma joined me!".into(),
                "I have studied for 10 hours in total.".into()
            )
        );
        assert_eq!(
            crate::wins::own_words(&ankilope.title, &ankilope.body).1,
            "I reached level 30."
        );
        assert_eq!(art_owner(COW), "ankilope");
        assert_eq!(art_owner("aki"), "aki");
    }

    /// An upload of one review that took `time_ms`, answered at `at`.
    fn study(at: i64, time_ms: i64) -> Value {
        json!({
            "reviews": [{"id": at, "cid": at, "last_ivl": 1, "ivl": 3, "time_ms": time_ms, "kind": 1}],
            "clock": {"offset_west_min": 0, "rollover_hour": 4}
        })
    }

    #[tokio::test]
    async fn ten_hours_of_study_bring_bluma_with_a_moment_to_share() {
        let (app, path) = fixture();
        let now = crate::app::now_ms();
        let reviews = "/api/reviews/hill";
        let unlocks = "/api/unlocks/hill";

        // The first upload records history; 9 hours 59 minutes are not enough.
        let almost = 10 * HOUR_MS - 60_000;
        let first = call(
            &app,
            "POST",
            reviews,
            "hill-secret",
            study(now - 3 * HOUR_MS, almost),
        )
        .await;
        assert_eq!(first.0, StatusCode::OK);
        let (status, got) = call(&app, "GET", unlocks, "hill-secret", Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(got["unlocks"], json!([]));
        assert_eq!(
            got["progress"]["cow"],
            json!({"kind": "study_hours", "value": 9, "goal": 10})
        );
        assert_eq!(got["progress"]["ankilope"]["goal"], 30);

        // One more minute makes 10 hours: the upload grants Bluma.
        let more = call(
            &app,
            "POST",
            reviews,
            "hill-secret",
            study(now - HOUR_MS, 60_000),
        )
        .await;
        assert_eq!(more.0, StatusCode::OK);
        assert_eq!(
            app.store.lock().unwrap().unlocks("hill").unwrap(),
            [Unlock::Cow]
        );
        let (_, got) = call(&app, "GET", unlocks, "hill-secret", Value::Null).await;
        assert_eq!(got["unlocks"], json!(["cow"]));
        assert_eq!(got["progress"].get("cow"), None);

        // The worker announces it once, as a moment with a win.
        crate::worker::tick(&app).unwrap();
        crate::worker::tick(&app).unwrap();
        let rows = inbox(&app, "/api/notifications/hill", Some("aki, ankilope, cow")).await;
        let joined: Vec<&Value> = rows
            .iter()
            .filter(|row| row["moment"] == "companion:cow")
            .collect();
        assert_eq!(joined.len(), 1, "{rows:?}");
        assert_eq!(joined[0]["title"], "Bluma joined you!");
        assert_eq!(joined[0]["kind"], "celebration");
        assert_eq!(joined[0]["topic"], "companion");
        assert!(joined[0]["win"].is_i64(), "{}", joined[0]);

        // It can be shared like any other moment.
        let share = json!({"moment": {"key": "companion:cow"}, "to": {"friends": ["cerro"]}});
        let shared = call(&app, "POST", "/api/share/hill", "hill-secret", share).await;
        assert_eq!(shared.0, StatusCode::OK, "{}", shared.1);
        assert_eq!(shared.1["recipients"], 1);
        cleanup(app, path);
    }

    /// The rows of `hill`'s inbox at `path`, for a client that lists the companions it can
    /// draw in `art`, or a released one that lists none.
    async fn inbox(app: &Arc<App>, path: &str, art: Option<&str>) -> Vec<Value> {
        let mut request = axum::http::Request::builder()
            .uri(path)
            .header("authorization", "Bearer hill-secret");
        if let Some(art) = art {
            request = request.header("x-ankiquest-companion-art", art);
        }
        let request = request.body(axum::body::Body::empty()).unwrap();
        let (status, body) =
            crate::test_support::json_response(crate::test_support::route(app, request).await)
                .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let rows = body.get("items").unwrap_or(&body).as_array().unwrap();
        rows.clone()
    }

    /// The companion moments among [`inbox`]'s rows.
    async fn companion_moments(app: &Arc<App>, path: &str, art: Option<&str>) -> Value {
        let rows = inbox(app, path, art).await;
        Value::Array(
            rows.iter()
                .filter_map(|row| row["moment"].as_str())
                .filter(|key| key.starts_with(MOMENT_PREFIX))
                .map(|key| Value::String(key.into()))
                .collect(),
        )
    }

    /// Uploads hill's study so that the worker grants Bluma with her moment.
    async fn bring_bluma(app: &Arc<App>) {
        let now = crate::app::now_ms();

        // A first upload and worker pass record history; the next upload brings Bluma.
        for (at, time_ms) in [
            (now - 3 * HOUR_MS, 10 * HOUR_MS - 60_000),
            (now - HOUR_MS, 60_000),
        ] {
            let upload = call(
                app,
                "POST",
                "/api/reviews/hill",
                "hill-secret",
                study(at, time_ms),
            )
            .await;
            assert_eq!(upload.0, StatusCode::OK);
            crate::worker::tick(app).unwrap();
        }
    }

    #[tokio::test]
    async fn the_inbox_leaves_out_companions_a_client_cannot_draw() {
        let (app, path) = fixture();
        bring_bluma(&app).await;
        let joined = json!(["companion:cow"]);

        // Released clients draw Aki and Ankilope only, and send no list: Bluma stays out of
        // both inbox endpoints, and the activity counts.
        for inbox in ["/api/notifications/hill", "/api/activity/hill"] {
            assert_eq!(
                companion_moments(&app, inbox, None).await,
                json!([]),
                "{inbox}"
            );
            let listed = companion_moments(&app, inbox, Some("aki, ankilope")).await;
            assert_eq!(listed, json!([]), "{inbox}");
            let listed = companion_moments(&app, inbox, Some("aki,ankilope,cow")).await;
            assert_eq!(listed, joined, "{inbox}");
            let listed = companion_moments(&app, inbox, Some("aki, ankilope, cow, horse")).await;
            assert_eq!(listed, joined, "{inbox}");
        }
        let (_, activity) = call(
            &app,
            "GET",
            "/api/activity/hill",
            "hill-secret",
            Value::Null,
        )
        .await;
        let items = activity["items"].as_array().unwrap().len();
        assert_eq!(activity["unread_count"], items, "{activity}");
        cleanup(app, path);
    }

    #[tokio::test]
    async fn read_all_leaves_moments_the_client_cannot_draw_unread() {
        let (app, path) = fixture();
        bring_bluma(&app).await;

        // A message arrives after Bluma's moment, so the released client's latest row is newer.
        {
            let store = app.store.lock().unwrap();
            let message = crate::notifications::Outgoing {
                to: "hill",
                from: "cerro",
                title: "Hello",
                body: "Keep going",
                kind: "message",
            };
            let now = crate::app::now_ms();
            crate::notifications::insert_notification(&store.conn, &message, 1, now, false)
                .unwrap();
        }

        // A released client reads everything it lists, up to its latest row.
        let activity = "/api/activity/hill";
        let (_, released) = call(&app, "GET", activity, "hill-secret", Value::Null).await;
        let through = json!({"through": released["latest_id"]});
        let read = call(
            &app,
            "POST",
            "/api/activity/hill/read",
            "hill-secret",
            through,
        )
        .await;
        assert_eq!(read.0, StatusCode::NO_CONTENT);

        // After an update that draws Bluma, her moment is still unread.
        let rows = inbox(&app, activity, Some("aki, ankilope, cow")).await;
        let bluma = rows
            .iter()
            .find(|row| row["moment"] == "companion:cow")
            .expect("Bluma's moment");
        assert_eq!(bluma["read_at"], Value::Null, "{bluma}");
        cleanup(app, path);
    }

    #[test]
    fn a_batch_of_unlocks_shows_only_where_every_unlock_is_drawn() {
        let (store, path) = crate::test_support::temporary_store();
        let now = crate::app::now_ms();

        // More unlocks at once than get their own push share one row, without a win.
        let batch = crate::notifications::Outgoing {
            to: "hill",
            from: "",
            title: "4 new unlocks",
            body: "Ankilope joined you!, Bluma joined you!",
            kind: "celebration",
        };
        let id =
            crate::notifications::insert_notification(&store.conn, &batch, 1, now, false).unwrap();
        store
            .conn
            .execute(
                "update notifications set topic = ?1 where id = ?2",
                rusqlite::params![TOPIC, id],
            )
            .unwrap();

        // Only a client that draws every live unlock can show the whole batch.
        let every: Vec<&str> = live().map(Unlock::id).collect();
        let some: Vec<&str> = every.iter().copied().filter(|id| *id != COW).collect();
        let listed = |art: Option<&str>| {
            let mut headers = HeaderMap::new();
            if let Some(art) = art {
                headers.insert(ART_HEADER, art.parse().unwrap());
            }
            let peers = crate::notifications::PeerStudy::Shown;
            let rows = store
                .notifications_for("hill", now, Drawable::of(&headers), peers)
                .unwrap();
            rows.len()
        };
        assert_eq!(listed(Some(&every.join(", "))), 1);
        assert_eq!(listed(Some(&some.join(", "))), 0);
        assert_eq!(listed(None), 0);
        let peers = crate::notifications::PeerStudy::Shown;
        let all = store.notifications_for("hill", now, Drawable::all(), peers);
        assert_eq!(all.unwrap().len(), 1);
        cleanup(store, path);
    }

    /// `POST /api/companion/hill` choosing `companion`, from a client that does or does not
    /// show locked companions.
    async fn choose(app: &Arc<App>, companion: &str, shows: bool) -> StatusCode {
        let mut request = axum::http::Request::builder()
            .method("POST")
            .uri("/api/companion/hill")
            .header("authorization", "Bearer hill-secret")
            .header("content-type", "application/json");
        if shows {
            request = request.header(CLIENT_HEADER, MILESTONES);
        }
        let body = json!({"companion": companion}).to_string();
        let request = request.body(axum::body::Body::from(body)).unwrap();
        crate::test_support::route(app, request).await.status()
    }

    #[tokio::test]
    async fn locked_companions_are_refused_except_ankilope_from_released_clients() {
        let (app, path) = fixture();

        // Clients that show locks are refused both locked companions.
        assert_eq!(choose(&app, "ankilope", true).await, StatusCode::CONFLICT);
        assert_eq!(choose(&app, "cow", true).await, StatusCode::CONFLICT);
        assert_eq!(choose(&app, "aki", true).await, StatusCode::OK);

        // Released clients offered Ankilope to everyone: their save is kept, and grants it
        // without a moment. They never offered the cow.
        assert_eq!(choose(&app, "cow", false).await, StatusCode::CONFLICT);
        assert_eq!(choose(&app, "ankilope", false).await, StatusCode::OK);
        {
            let store = app.store.lock().unwrap();
            assert_eq!(store.companion("hill").unwrap(), "ankilope");
            assert_eq!(store.unlocks("hill").unwrap(), [Unlock::Ankilope]);
            assert!(store.is_seen("hill", "companion:ankilope").unwrap());
        }
        assert_eq!(choose(&app, "ankilope", true).await, StatusCode::OK);

        // Clients cannot report unlocks: only one already granted answers.
        let unlocks = "/api/unlocks/hill";
        let ask = |id: &str| json!({"unlock": id});
        let granted = call(&app, "POST", unlocks, "hill-secret", ask("ankilope")).await;
        assert_eq!(granted.0, StatusCode::OK);
        assert_eq!(granted.1["unlocks"], json!(["ankilope"]));
        let cow = call(&app, "POST", unlocks, "hill-secret", ask("cow")).await;
        assert_eq!(cow.0, StatusCode::CONFLICT);
        for unknown in ["horse", "anki_tan", "leech_tan"] {
            let bad = call(&app, "POST", unlocks, "hill-secret", ask(unknown)).await;
            assert_eq!(bad.0, StatusCode::BAD_REQUEST, "{unknown}");
        }

        // Only the player's own token reads or asks.
        let other = call(&app, "GET", unlocks, "cerro-secret", Value::Null).await;
        assert_eq!(other.0, StatusCode::UNAUTHORIZED);
        let other = call(&app, "POST", unlocks, "cerro-secret", ask("cow")).await;
        assert_eq!(other.0, StatusCode::UNAUTHORIZED);
        cleanup(app, path);
    }
}
