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

/// The companions every player has. The others are unlocks, choosable once live and unlocked.
const FREE: [&str; 2] = ["aki", "none"];

/// Every companion the table stores, planned ones included, so that making one live needs no
/// migration. Changing it needs a migration step.
pub(crate) const TABLE: &str = "create table if not exists companion_preferences (
    user text primary key,
    companion text not null check (companion in
        ('aki', 'ankilope', 'cow', 'none', 'anki_tan', 'anki_tan_chibi', 'quest_chan',
         'quest_chan_chibi', 'leech_kun'))
) without rowid;";

/// Whether a player may save `value`: a free companion or a live unlock.
pub fn valid(value: &str) -> bool {
    FREE.contains(&value) || Unlock::parse(value).is_some_and(Unlock::live)
}

/// Something a player unlocks once and keeps. Unlocks are not settings: settings sync does not
/// touch them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unlock {
    /// Ankilope, earned at a level.
    Ankilope,
    /// The cow, shown as Bluma, earned by hours of study.
    Cow,
    /// Anki-tan, earned by a streak.
    AnkiTan,
    /// Anki-tan's chibi version, found by tapping Anki-tan.
    AnkiTanChibi,
    /// Quest-chan, earned at a level after Ankilope's.
    QuestChan,
    /// Quest-chan's chibi version, found by tapping Quest-chan.
    QuestChanChibi,
    /// Leech-kun, earned by taming leeches.
    LeechKun,
}

/// Whether the clients released before milestones offered a companion to everyone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Released {
    /// Offered to every player, so a save from those clients is still accepted.
    Offered,
    /// Never offered by them.
    Hidden,
}

impl Unlock {
    /// In the fixture's order.
    const ALL: [Unlock; 7] = [
        Unlock::Ankilope,
        Unlock::Cow,
        Unlock::AnkiTan,
        Unlock::AnkiTanChibi,
        Unlock::QuestChan,
        Unlock::QuestChanChibi,
        Unlock::LeechKun,
    ];

    pub(crate) fn id(self) -> &'static str {
        match self {
            Unlock::Ankilope => "ankilope",
            Unlock::Cow => COW,
            Unlock::AnkiTan => "anki_tan",
            Unlock::AnkiTanChibi => "anki_tan_chibi",
            Unlock::QuestChan => "quest_chan",
            Unlock::QuestChanChibi => "quest_chan_chibi",
            Unlock::LeechKun => "leech_kun",
        }
    }

    /// The companion's name, as both clients show it.
    fn name(self) -> &'static str {
        match self {
            Unlock::Ankilope => "Ankilope",
            Unlock::Cow => "Bluma",
            Unlock::AnkiTan => "Anki-tan",
            Unlock::AnkiTanChibi => "Anki-tan chibi",
            Unlock::QuestChan => "Quest-chan",
            Unlock::QuestChanChibi => "Quest-chan chibi",
            Unlock::LeechKun => "Leech-kun",
        }
    }

    fn released(self) -> Released {
        match self {
            Unlock::Ankilope => Released::Offered,
            Unlock::Cow
            | Unlock::AnkiTan
            | Unlock::AnkiTanChibi
            | Unlock::QuestChan
            | Unlock::QuestChanChibi
            | Unlock::LeechKun => Released::Hidden,
        }
    }

    fn parse(id: &str) -> Option<Unlock> {
        Self::ALL.into_iter().find(|unlock| unlock.id() == id)
    }

    /// Whether players can earn it: planned ones have no art yet and are ignored.
    fn live(self) -> bool {
        milestone(self).status == Status::Live
    }

    /// The moment a player gets when this unlock is granted.
    fn event(self) -> Event {
        let milestone = milestone(self);
        let goal = milestone.goal.to_string();
        let body = match milestone.counts {
            Counts::Measured(Measure::Level) => english("You reached level {0}.", &[&goal]),
            Counts::Measured(Measure::StudyHours) => {
                english("You have studied for {0} hours in total.", &[&goal])
            }
            Counts::Measured(Measure::Streak) => english("You kept a {0}-day streak.", &[&goal]),
            Counts::Measured(Measure::LeechesTamed) => english("You tamed {0} leeches.", &[&goal]),
            Counts::Taps { .. } => english("You found {0}.", &[self.name()]),
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
/// `cards::turn_purple` tints.
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

/// The conditions both clients show, defined once with the cases they test against.
const RULES: &str = include_str!("../tests/fixtures/companion_unlock.json");

/// What the server measures toward a milestone, as the fixture's `kind` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Measure {
    Level,
    StudyHours,
    /// Days in a row, as the player's streak stands now.
    Streak,
    LeechesTamed,
}

/// What counts toward a milestone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Counts {
    /// A value the server derives from the player's reviews.
    Measured(Measure),
    /// A chibi: the client counts taps on its base `on` by the fixture's tap rule and asks for
    /// the chibi on the last one. The server only checks that `on` is unlocked.
    Taps { on: Unlock },
}

/// The fixture's `kind` for [`Counts::Taps`].
const TAPS_KIND: &str = "taps";

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

/// A fixture entry as written.
#[derive(Deserialize)]
struct Rule {
    companion: String,
    kind: String,
    goal: u64,
    requires: Option<String>,
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
    counts: Counts,
    goal: u64,
    visibility: Visibility,
    status: Status,
}

/// Every milestone of a fixture, planned ones included, in its order; an error names what the
/// fixture gets wrong.
fn parse(source: &str) -> Result<Vec<Milestone>, String> {
    let rules: Rules = serde_json::from_str(source).map_err(|error| error.to_string())?;
    rules.companions.into_iter().map(read).collect()
}

/// One fixture entry as a milestone.
fn read(rule: Rule) -> Result<Milestone, String> {
    let unlock = Unlock::parse(&rule.companion)
        .ok_or_else(|| format!("{} is not an unlock", rule.companion))?;

    // A chibi names its base, another unlock; every other kind is measured.
    let counts = if rule.kind == TAPS_KIND {
        let on = rule
            .requires
            .as_deref()
            .and_then(Unlock::parse)
            .filter(|on| *on != unlock)
            .ok_or_else(|| format!("{} needs its base in `requires`", rule.companion))?;
        Counts::Taps { on }
    } else {
        let kind = serde_json::Value::String(rule.kind);
        Counts::Measured(serde_json::from_value(kind).map_err(|error| error.to_string())?)
    };
    let visibility = if rule.secret {
        Visibility::Secret
    } else {
        Visibility::Shown
    };
    Ok(Milestone {
        unlock,
        counts,
        goal: rule.goal,
        visibility,
        status: rule.status,
    })
}

fn milestones() -> &'static [Milestone] {
    static PARSED: OnceLock<Vec<Milestone>> = OnceLock::new();
    PARSED.get_or_init(|| parse(RULES).expect("the unlock fixture is valid"))
}

fn milestone(unlock: Unlock) -> Milestone {
    *milestones()
        .iter()
        .find(|milestone| milestone.unlock == unlock)
        .expect("every unlock has a milestone")
}

/// What milestones measure for one player, derived from their reviews and never stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Measures {
    level: u64,
    study_hours: u64,
    streak: u64,
    leeches_tamed: u64,
}

impl Measures {
    /// A player without reviews is at the first level with nothing studied.
    fn of(profile: Option<&Profile>) -> Self {
        match profile {
            Some(profile) => Measures {
                level: profile.level,
                study_hours: profile.lifetime.hours,
                streak: profile.streak,
                leeches_tamed: profile.lifetime.leeches_tamed,
            },
            None => Measures {
                level: crate::game::level_for(0).0,
                study_hours: 0,
                streak: 0,
                leeches_tamed: 0,
            },
        }
    }

    fn value(self, measure: Measure) -> u64 {
        match measure {
            Measure::Level => self.level,
            Measure::StudyHours => self.study_hours,
            Measure::Streak => self.streak,
            Measure::LeechesTamed => self.leeches_tamed,
        }
    }

    /// Whether these reach `milestone`. A chibi is found by the client, never measured.
    fn reached(self, milestone: &Milestone) -> bool {
        match milestone.counts {
            Counts::Measured(measure) => self.value(measure) >= milestone.goal,
            Counts::Taps { .. } => false,
        }
    }
}

/// What a client asking for an unlock comes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    /// Already unlocked: answer the list.
    Kept,
    /// A chibi whose base is unlocked: grant it now.
    Grant,
    /// Not earned: `409`.
    Locked,
}

impl Verdict {
    /// The answer's status.
    fn status(self) -> StatusCode {
        match self {
            Verdict::Kept | Verdict::Grant => StatusCode::OK,
            Verdict::Locked => StatusCode::CONFLICT,
        }
    }
}

/// Judges a request for `milestone` from a player who has `unlocked`. The server grants
/// measured unlocks itself, so a client only asks after them; a chibi is granted once its base
/// is unlocked.
fn judge(milestone: &Milestone, unlocked: &[Unlock]) -> Verdict {
    if unlocked.contains(&milestone.unlock) {
        return Verdict::Kept;
    }
    match milestone.counts {
        Counts::Taps { on } if unlocked.contains(&on) => Verdict::Grant,
        Counts::Taps { .. } | Counts::Measured(_) => Verdict::Locked,
    }
}

/// How far a player is toward a locked companion.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Progress {
    kind: Measure,
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

    /// Grants each of `milestones` that `measures` reach and answers what `user` has unlocked.
    fn earn(
        &self,
        user: &str,
        milestones: &[Milestone],
        measures: Measures,
        at_ms: i64,
    ) -> Result<Vec<Unlock>, Error> {
        for milestone in milestones {
            if measures.reached(milestone) {
                self.add_unlock(user, milestone.unlock, at_ms)?;
            }
        }
        self.unlocks(user)
    }

    /// Grants every live milestone `measures` reach and answers what `user` has unlocked that
    /// is live. Planned companions are ignored.
    fn grant(&self, user: &str, measures: Measures, at_ms: i64) -> Result<Vec<Unlock>, Error> {
        let live: Vec<Milestone> = milestones()
            .iter()
            .copied()
            .filter(|milestone| milestone.status == Status::Live)
            .collect();
        let unlocks = self.earn(user, &live, measures, at_ms)?;
        Ok(unlocks.into_iter().filter(|unlock| unlock.live()).collect())
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
    /// The tap run apps released before the chibi became an Easter egg send with her. The
    /// server no longer checks it, but still accepts it so those apps keep working.
    #[serde(default, rename = "taps")]
    _taps: Option<serde::de::IgnoredAny>,
}

/// `unlocks` with the progress toward every other live, measured companion of `milestones`
/// that shows while locked.
fn listed(milestones: &[Milestone], unlocks: Vec<Unlock>, measures: Measures) -> Unlocks {
    let progress = milestones
        .iter()
        .filter(|milestone| milestone.status == Status::Live)
        .filter(|milestone| milestone.visibility == Visibility::Shown)
        .filter(|milestone| !unlocks.contains(&milestone.unlock))
        .filter_map(|milestone| {
            let Counts::Measured(measure) = milestone.counts else {
                return None;
            };
            let progress = Progress {
                kind: measure,
                value: measures.value(measure),
                goal: milestone.goal,
            };
            Some((milestone.unlock.id(), progress))
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
    Ok(listed(milestones(), unlocks, measures))
}

pub async fn get_unlocks(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Unlocks>, StatusCode> {
    require_owner(&app, &user, &headers)?;
    current(&app, &user).map(Json)
}

/// The server grants measured unlocks itself, so asking for one already granted answers the
/// list, as it did when clients reported the cow, and any other is `409`. A chibi, found by
/// tapping its base in the client, is granted once its base is unlocked, and is `409` before.
/// Unknown ids, and companions the fixture lists as planned, are `400`.
pub async fn add_unlock(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
    Json(request): Json<UnlockRequest>,
) -> Result<Json<Unlocks>, StatusCode> {
    require_owner(&app, &user, &headers)?;
    let unlock = Unlock::parse(&request.unlock)
        .filter(|unlock| unlock.live())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let answer = current(&app, &user)?;
    let unlocked: Vec<Unlock> = answer
        .unlocks
        .iter()
        .filter_map(|id| Unlock::parse(id))
        .collect();
    match judge(&milestone(unlock), &unlocked) {
        Verdict::Kept => Ok(Json(answer)),
        verdict @ Verdict::Locked => Err(verdict.status()),
        Verdict::Grant => {
            // The worker announces it with its moment, as it does every unlock.
            app.store
                .lock()
                .unwrap()
                .add_unlock(&user, unlock, crate::app::now_ms())
                .map_err(crate::store_error)?;
            current(&app, &user).map(Json)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{call, cleanup, fixture};
    use serde_json::{Value, json};

    const HOUR_MS: i64 = 3_600_000;

    fn measures(level: u64, study_hours: u64) -> Measures {
        Measures {
            level,
            study_hours,
            streak: 0,
            leeches_tamed: 0,
        }
    }

    /// A player at the first level whose `measure` is `value`, everything else at nothing.
    fn only(measure: Measure, value: u64) -> Measures {
        let mut measured = measures(1, 0);
        match measure {
            Measure::Level => measured.level = value,
            Measure::StudyHours => measured.study_hours = value,
            Measure::Streak => measured.streak = value,
            Measure::LeechesTamed => measured.leeches_tamed = value,
        }
        measured
    }

    /// The fixture with every planned companion live, as it will be once their art ships.
    fn all_live() -> Vec<Milestone> {
        parse(&RULES.replace("\"status\": \"planned\"", "\"status\": \"live\"")).unwrap()
    }

    fn find(milestones: &[Milestone], unlock: Unlock) -> Milestone {
        *milestones.iter().find(|m| m.unlock == unlock).unwrap()
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
    fn every_companion_in_the_fixture_is_an_unlock_with_one_milestone() {
        let rules: Value = serde_json::from_str(RULES).unwrap();
        for unlock in Unlock::ALL {
            let found = milestones().iter().filter(|m| m.unlock == unlock).count();
            assert_eq!(found, 1, "{unlock:?}");
            assert_eq!(Unlock::parse(unlock.id()), Some(unlock));
            assert_eq!(needed(unlock.id()), Some(unlock));
        }
        assert_eq!(milestones().len(), Unlock::ALL.len());

        // Planned companions have no art yet: nobody may choose them.
        for rule in rules["companions"].as_array().unwrap() {
            let live = rule["status"] == "live";
            let id = rule["companion"].as_str().unwrap();
            let unlock = Unlock::parse(id).unwrap();
            assert_eq!(milestone(unlock).status == Status::Live, live, "{id}");
            assert_eq!(valid(id), live, "{id}");
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
            let Counts::Measured(measure) = milestone.counts else {
                panic!("{} checks a measured kind", check["name"]);
            };
            assert_eq!(
                only(measure, value).reached(&milestone),
                check["unlocks"].as_bool().unwrap(),
                "{}",
                check["name"]
            );
        }
    }

    #[test]
    fn every_chibi_names_its_unlocked_base() {
        let rules: Value = serde_json::from_str(RULES).unwrap();
        let chibis = [
            (Unlock::AnkiTanChibi, Unlock::AnkiTan),
            (Unlock::QuestChanChibi, Unlock::QuestChan),
        ];
        for (unlock, base) in chibis {
            let chibi = milestone(unlock);
            assert_eq!(chibi.counts, Counts::Taps { on: base }, "{unlock:?}");
            assert_eq!(chibi.goal, rules["taps"].as_u64().unwrap(), "{unlock:?}");
            assert_eq!(chibi.visibility, Visibility::Secret, "{unlock:?}");
        }
    }

    #[test]
    fn a_chibi_without_its_base_fails_the_fixture() {
        let base = "\"requires\": \"anki_tan\", ";
        assert!(RULES.contains(base));
        assert!(parse(&RULES.replace(base, "")).is_err());

        // A chibi of itself or of an unknown companion has no base either.
        let own = RULES.replace(base, "\"requires\": \"anki_tan_chibi\", ");
        assert!(parse(&own).is_err());
        let unknown = RULES.replace(base, "\"requires\": \"horse\", ");
        assert!(parse(&unknown).is_err());
    }

    #[test]
    fn the_fixture_requests_decide_each_chibi_unlock() {
        let live = all_live();
        let rules: Value = serde_json::from_str(RULES).unwrap();
        let requests = rules["requests"].as_array().unwrap();
        assert!(!requests.is_empty());
        for request in requests {
            let unlock = Unlock::parse(request["unlock"].as_str().unwrap()).unwrap();
            let unlocked: Vec<Unlock> = request["unlocked"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| Unlock::parse(id.as_str().unwrap()).unwrap())
                .collect();
            let status = judge(&find(&live, unlock), &unlocked).status();
            assert_eq!(
                u64::from(status.as_u16()),
                request["status"].as_u64().unwrap(),
                "{}",
                request["name"]
            );
        }
    }

    #[test]
    fn planned_companions_are_never_granted_or_listed() {
        let (store, path) = crate::test_support::temporary_store();
        let everything = Measures {
            level: 99,
            study_hours: 99,
            streak: 999,
            leeches_tamed: 99,
        };
        assert_eq!(
            store.grant("hill", everything, 1).unwrap(),
            [Unlock::Ankilope, Unlock::Cow]
        );
        let shown = serde_json::to_value(listed(milestones(), vec![], everything)).unwrap();
        assert_eq!(
            shown["progress"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            ["ankilope", "cow"]
        );

        // A planned unlock stored anyway stays out of the answer and the moments.
        store.add_unlock("hill", Unlock::LeechKun, 2).unwrap();
        assert_eq!(
            store.grant("hill", everything, 3).unwrap(),
            [Unlock::Ankilope, Unlock::Cow]
        );
        cleanup(store, path);
    }

    #[test]
    fn once_live_streaks_and_tamed_leeches_are_granted() {
        let (store, path) = crate::test_support::temporary_store();
        let live = all_live();
        let earn = |measured, at| store.earn("hill", &live, measured, at).unwrap();
        assert!(earn(only(Measure::Streak, 65), 1).is_empty());
        assert_eq!(earn(only(Measure::Streak, 66), 2), [Unlock::AnkiTan]);
        assert_eq!(earn(only(Measure::LeechesTamed, 2), 3), [Unlock::AnkiTan]);
        assert_eq!(
            earn(only(Measure::LeechesTamed, 3), 4),
            [Unlock::AnkiTan, Unlock::LeechKun]
        );

        // A lost streak takes nothing back, and no measure brings the chibi: she is tapped.
        let everything = Measures {
            level: 99,
            study_hours: 99,
            streak: 0,
            leeches_tamed: 99,
        };
        assert!(!earn(everything, 5).contains(&Unlock::AnkiTanChibi));
        assert!(earn(everything, 5).contains(&Unlock::AnkiTan));
        cleanup(store, path);
    }

    #[test]
    fn once_live_level_50_brings_quest_chan_but_not_her_chibi() {
        let (store, path) = crate::test_support::temporary_store();
        let live = all_live();
        let earn = |measured, at| store.earn("hill", &live, measured, at).unwrap();

        // Level 30 brings Ankilope; Quest-chan waits for level 50.
        assert_eq!(earn(only(Measure::Level, 49), 1), [Unlock::Ankilope]);
        assert_eq!(
            earn(only(Measure::Level, 50), 2),
            [Unlock::Ankilope, Unlock::QuestChan]
        );

        // Her chibi is tapped, never measured.
        assert!(!earn(only(Measure::Level, 99), 3).contains(&Unlock::QuestChanChibi));
        cleanup(store, path);
    }

    #[test]
    fn once_live_anki_tan_and_quest_chan_show_progress() {
        let live = all_live();
        let measured = Measures {
            level: 3,
            study_hours: 6,
            streak: 12,
            leeches_tamed: 2,
        };
        let shown = serde_json::to_value(listed(&live, vec![], measured)).unwrap();
        assert_eq!(
            shown["progress"],
            json!({
                "ankilope": {"kind": "level", "value": 3, "goal": 30},
                "cow": {"kind": "study_hours", "value": 6, "goal": 10},
                "anki_tan": {"kind": "streak", "value": 12, "goal": 66},
                "quest_chan": {"kind": "level", "value": 3, "goal": 50}
            })
        );
    }

    #[test]
    fn the_chibi_joins_only_after_her_base() {
        let live = all_live();
        let chibi = find(&live, Unlock::AnkiTanChibi);

        // Her base first; then asking brings her, and asking again answers the list.
        assert_eq!(judge(&chibi, &[]), Verdict::Locked);
        assert_eq!(judge(&chibi, &[Unlock::Cow]), Verdict::Locked);
        assert_eq!(judge(&chibi, &[Unlock::AnkiTan]), Verdict::Grant);
        let both = [Unlock::AnkiTan, Unlock::AnkiTanChibi];
        assert_eq!(judge(&chibi, &both), Verdict::Kept);

        // Quest-chan's chibi needs Quest-chan, not Anki-tan.
        let quest = find(&live, Unlock::QuestChanChibi);
        assert_eq!(judge(&quest, &[Unlock::AnkiTan]), Verdict::Locked);
        assert_eq!(judge(&quest, &[Unlock::QuestChan]), Verdict::Grant);
        assert_eq!(judge(&chibi, &[Unlock::QuestChan]), Verdict::Locked);

        // Measured unlocks are granted by the server; asking only answers.
        let cow = find(&live, Unlock::Cow);
        assert_eq!(judge(&cow, &[Unlock::Cow]), Verdict::Kept);
        assert_eq!(judge(&cow, &[]), Verdict::Locked);
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
            [Unlock::Ankilope, Unlock::Cow]
        );
        cleanup(store, path);
    }

    #[test]
    fn progress_lists_only_locked_companions_that_show() {
        let listed = |unlocks: Vec<Unlock>, measured| {
            serde_json::to_value(super::listed(milestones(), unlocks, measured)).unwrap()
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
            listed(vec![Unlock::Ankilope, Unlock::Cow], measures(40, 12)),
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

        // The planned companions, ready for when they go live.
        let expected = [
            (
                Unlock::AnkiTan,
                "Anki-tan joined you!",
                "You kept a 66-day streak.",
                "I kept a 66-day streak.",
            ),
            (
                Unlock::AnkiTanChibi,
                "Anki-tan chibi joined you!",
                "You found Anki-tan chibi.",
                "I found Anki-tan chibi.",
            ),
            (
                Unlock::QuestChan,
                "Quest-chan joined you!",
                "You reached level 50.",
                "I reached level 50.",
            ),
            (
                Unlock::QuestChanChibi,
                "Quest-chan chibi joined you!",
                "You found Quest-chan chibi.",
                "I found Quest-chan chibi.",
            ),
            (
                Unlock::LeechKun,
                "Leech-kun joined you!",
                "You tamed 3 leeches.",
                "I tamed 3 leeches.",
            ),
        ];
        for (unlock, title, body, own) in expected {
            let event = unlock.event();
            assert_eq!(event.key, format!("companion:{}", unlock.id()));
            assert_eq!((event.title.as_str(), event.body.as_str()), (title, body));
            assert_eq!(crate::wins::own_words(&event.title, &event.body).1, own);
            assert_eq!(of_moment(&event.key), Some(unlock.id()));
        }
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
        let (_, inbox) = call(
            &app,
            "GET",
            "/api/notifications/hill",
            "hill-secret",
            Value::Null,
        )
        .await;
        let joined: Vec<&Value> = inbox
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["moment"] == "companion:cow")
            .collect();
        assert_eq!(joined.len(), 1, "{inbox}");
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
        // Planned companions are refused like unknown ids, as is Leech-kun's old id.
        let planned = [
            "anki_tan",
            "anki_tan_chibi",
            "quest_chan",
            "quest_chan_chibi",
        ];
        for unknown in ["horse", "leech_tan", "leech_kun"]
            .into_iter()
            .chain(planned)
        {
            let bad = call(&app, "POST", unlocks, "hill-secret", ask(unknown)).await;
            assert_eq!(bad.0, StatusCode::BAD_REQUEST, "{unknown}");
        }

        // Released apps may still send a tap run: it is ignored, whatever the unlock.
        let run: Vec<Value> = (0..9).map(|i| json!(["anki_tan", i * 100])).collect();
        let tapped = json!({"unlock": "anki_tan_chibi", "taps": run});
        let bad = call(&app, "POST", unlocks, "hill-secret", tapped).await;
        assert_eq!(bad.0, StatusCode::BAD_REQUEST, "she is planned");
        let tapped = json!({"unlock": "ankilope", "taps": run});
        let kept = call(&app, "POST", unlocks, "hill-secret", tapped).await;
        assert_eq!(kept.0, StatusCode::OK);
        assert_eq!(kept.1["unlocks"], json!(["ankilope"]));
        let tapped = json!({"unlock": "cow", "taps": "anything"});
        let locked = call(&app, "POST", unlocks, "hill-secret", tapped).await;
        assert_eq!(locked.0, StatusCode::CONFLICT);
        let odd = json!({"unlock": "cow", "proof": true});
        let bad = call(&app, "POST", unlocks, "hill-secret", odd).await;
        assert_eq!(
            bad.0,
            StatusCode::UNPROCESSABLE_ENTITY,
            "other fields are still refused"
        );

        // Only the player's own token reads or asks.
        let other = call(&app, "GET", unlocks, "cerro-secret", Value::Null).await;
        assert_eq!(other.0, StatusCode::UNAUTHORIZED);
        let other = call(&app, "POST", unlocks, "cerro-secret", ask("cow")).await;
        assert_eq!(other.0, StatusCode::UNAUTHORIZED);
        cleanup(app, path);
    }
}
