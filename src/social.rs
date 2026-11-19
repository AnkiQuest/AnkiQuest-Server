//! Who sees and reaches whom: groups joined by invite, mutual friends, and public profiles,
//! which every player has until they make theirs private.
//! Everyone who is not a self-service account (configured and sync-folder players) keeps
//! sharing the original community, so existing deployments behave as before.

use crate::web_util::{json_error, normalize_user};
use crate::{Error, app::App, app::now_ms};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing::get};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;

pub const MAX_GROUPS: i64 = 20;
pub const MAX_MEMBERS: i64 = 200;
pub const MAX_PENDING: i64 = 50;
pub const MAX_MUTED: i64 = 500;
const MAX_GROUP_NAME: usize = 40;
/// How long an invitation to a group waits for an answer: as long as the inbox keeps its row.
pub(crate) const INVITATION_AGE_MS: i64 = crate::notifications::ACTIVITY_AGE_MS;
/// Invitations to one group that may wait for an answer at once.
pub const MAX_INVITATIONS: i64 = 50;

/// Where an invitation to a group stands until it expires, `INVITATION_AGE_MS` after it was
/// sent. Joining the group, by the invitation or by link, deletes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InvitationState {
    /// Waiting for `accept_invite` or `decline_invite`.
    Pending,
    /// Declined. The owner still sees the player as invited and inviting again does nothing,
    /// so a decline is as private as a mute.
    Declined,
}

impl InvitationState {
    fn parse(state: &str) -> Option<Self> {
        match state {
            "pending" => Some(Self::Pending),
            "declined" => Some(Self::Declined),
            _ => None,
        }
    }
}

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists groups (
             id integer primary key autoincrement,
             name text not null,
             invite text not null unique,
             created_by text not null,
             created_at integer not null
         );
         create table if not exists group_members (
             group_id integer not null references groups (id) on delete cascade,
             user text not null,
             owner integer not null default 0,
             joined_at integer not null,
             primary key (group_id, user)
         ) without rowid;
         create index if not exists group_members_user on group_members (user);
         create table if not exists friendships (
             a text not null,
             b text not null,
             requested_by text not null,
             accepted integer not null default 0,
             created_at integer not null,
             primary key (a, b),
             check (a < b)
         ) without rowid;
         create index if not exists friendships_b on friendships (b);
         create table if not exists visibility (
             user text primary key,
             public integer not null
         ) without rowid;
         create table if not exists muted_people (
             recipient text not null,
             sender text not null,
             created_at integer not null,
             primary key (recipient, sender)
         ) without rowid;
         create table if not exists group_invitations (
             group_id integer not null references groups (id) on delete cascade,
             user text not null,
             invited_by text not null,
             created_at integer not null,
             state text not null default 'pending' check (state in ('pending', 'declined')),
             -- The inbox row this invitation put there; null when the invitee muted the owner.
             notification integer,
             primary key (group_id, user)
         ) without rowid;
         create index if not exists group_invitations_user on group_invitations (user);
         create table if not exists global_board_hides (
             viewer text not null,
             target text not null,
             created_at integer not null,
             primary key (viewer, target),
             check (viewer != target)
         ) without rowid;",
    )?;
    Ok(())
}

/// Whether `recipient` muted `sender`: no invitations, conquests, nudges, completions, shares
/// or cheers from them. Both are SQL expressions.
pub(crate) fn muted_sql(recipient: &str, sender: &str) -> String {
    format!(
        "exists (select 1 from muted_people where recipient = {recipient} and sender = {sender})"
    )
}

/// Whether `recipient` muted `sender`; see [`muted_sql`].
pub(crate) fn muted(conn: &Connection, recipient: &str, sender: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        &format!("select {}", muted_sql("?1", "?2")),
        params![recipient, sender],
        |r| r.get(0),
    )
}

pub(crate) fn pair<'a>(x: &'a str, y: &'a str) -> (&'a str, &'a str) {
    if x < y { (x, y) } else { (y, x) }
}

pub fn friends(conn: &Connection, user: &str) -> rusqlite::Result<BTreeSet<String>> {
    conn.prepare(
        "select case when a = ?1 then b else a end from friendships
             where accepted = 1 and (a = ?1 or b = ?1)",
    )?
    .query_map([user], |r| r.get(0))?
    .collect::<Result<_, _>>()
}

pub fn group_mates(conn: &Connection, user: &str) -> rusqlite::Result<BTreeSet<String>> {
    conn.prepare(
        "select distinct other.user from group_members mine
             join group_members other on other.group_id = mine.group_id
             where mine.user = ?1",
    )?
    .query_map([user], |r| r.get(0))?
    .collect::<Result<_, _>>()
}

pub fn group_members(conn: &Connection, group: i64) -> rusqlite::Result<BTreeSet<String>> {
    conn.prepare("select user from group_members where group_id = ?1")?
        .query_map([group], |r| r.get(0))?
        .collect::<Result<_, _>>()
}

pub(crate) fn is_member(conn: &Connection, group: i64, user: &str) -> rusqlite::Result<bool> {
    conn.prepare("select 1 from group_members where group_id = ?1 and user = ?2")?
        .exists(params![group, user])
}

fn public_flag(conn: &Connection, user: &str) -> rusqlite::Result<Option<bool>> {
    conn.query_row(
        "select public from visibility where user = ?1",
        [user],
        |r| r.get(0),
    )
    .optional()
}

/// Players who turned the global leaderboard off themselves.
pub(crate) fn global_opt_outs(conn: &Connection) -> rusqlite::Result<BTreeSet<String>> {
    conn.prepare("select user from player_settings where global_leaderboard = 0")?
        .query_map([], |r| r.get(0))?
        .collect()
}

/// Who is asking: a signed-in player, the shared community password, or nobody.
pub struct Viewer {
    pub member: Option<String>,
    pub community: bool,
}

/// Which players a board covers.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Circle,
    Global,
    Group(i64),
}

impl Scope {
    /// A board's `scope`, with `group` naming the group for `scope=group`. `friends` and
    /// `circle` are the same board; `group:<id>` is the website's spelling of a group.
    pub fn parse(value: Option<&str>, group: Option<&str>) -> Option<Option<Scope>> {
        match value {
            None | Some("") => Some(None),
            Some("circle" | "friends") => Some(Some(Scope::Circle)),
            Some("global") => Some(Some(Scope::Global)),
            Some("group") => group
                .and_then(|id| id.parse().ok())
                .map(|id| Some(Scope::Group(id))),
            Some(other) => other
                .strip_prefix("group:")
                .and_then(|id| id.parse().ok())
                .map(|id| Some(Scope::Group(id))),
        }
    }
}

/// A player's friends, group mates and the player, plus `legacy_players()` when the player is
/// legacy too: everyone from before accounts shares the original community.
pub(crate) fn circle_of(
    conn: &Connection,
    user: &str,
    legacy: bool,
    legacy_players: impl FnOnce() -> BTreeSet<String>,
) -> rusqlite::Result<BTreeSet<String>> {
    let mut circle = friends(conn, user)?;
    circle.extend(group_mates(conn, user)?);
    if legacy {
        circle.extend(legacy_players());
    }
    circle.insert(user.to_string());
    Ok(circle)
}

impl App {
    /// Players from before accounts existed: configured and sync-folder players.
    pub fn legacy(&self, user: &str) -> bool {
        !self.accounts.contains(user)
    }

    pub fn known_users(&self) -> BTreeSet<String> {
        self.config
            .users
            .keys()
            .cloned()
            .chain(self.players.read().unwrap().keys().cloned())
            .chain(self.accounts.users())
            .collect()
    }

    /// Profiles are public until their player makes them private.
    pub fn is_public(&self, conn: &Connection, user: &str) -> rusqlite::Result<bool> {
        Ok(public_flag(conn, user)?.unwrap_or(true))
    }

    /// Whether the profile was public before profiles became public by default: chosen so, or
    /// a player from before accounts who never chose. What a player wrote about their cards
    /// (conquest labels) reaches strangers only then, as it did when they wrote it.
    pub(crate) fn chose_public(&self, conn: &Connection, user: &str) -> rusqlite::Result<bool> {
        Ok(public_flag(conn, user)?.unwrap_or_else(|| self.legacy(user)))
    }

    /// The people a player shares something with, including the player.
    pub fn circle(&self, conn: &Connection, user: &str) -> rusqlite::Result<BTreeSet<String>> {
        circle_of(conn, user, self.legacy(user), || {
            self.known_users()
                .into_iter()
                .filter(|other| self.legacy(other))
                .collect()
        })
    }

    pub fn connected(&self, conn: &Connection, a: &str, b: &str) -> rusqlite::Result<bool> {
        Ok(a == b || self.circle(conn, a)?.contains(b))
    }

    /// Everyone with a public profile who did not take themselves off the global board.
    pub fn global(&self, conn: &Connection) -> rusqlite::Result<BTreeSet<String>> {
        let private: BTreeSet<String> = conn
            .prepare("select user from visibility where public = 0")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let unlisted = global_opt_outs(conn)?;
        Ok(self
            .known_users()
            .into_iter()
            .filter(|user| !private.contains(user) && !unlisted.contains(user))
            .collect())
    }

    pub fn visible(
        &self,
        conn: &Connection,
        viewer: &Viewer,
        target: &str,
    ) -> rusqlite::Result<bool> {
        let public = self.is_public(conn, target)?;
        self.visible_if(conn, viewer, target, public)
    }

    /// [`App::visible`] under the rule from before profiles became public by default.
    pub(crate) fn visible_by_choice(
        &self,
        conn: &Connection,
        viewer: &Viewer,
        target: &str,
    ) -> rusqlite::Result<bool> {
        let public = self.chose_public(conn, target)?;
        self.visible_if(conn, viewer, target, public)
    }

    fn visible_if(
        &self,
        conn: &Connection,
        viewer: &Viewer,
        target: &str,
        public: bool,
    ) -> rusqlite::Result<bool> {
        if viewer.member.as_deref() == Some(target) {
            return Ok(true);
        }

        // Others' study waits for the viewer's first review.
        if self.lock(viewer).is_some() {
            return Ok(false);
        }
        if public {
            return Ok(true);
        }
        if viewer.community && self.legacy(target) {
            return Ok(true);
        }
        match &viewer.member {
            Some(member) => self.connected(conn, member, target),
            None => Ok(false),
        }
    }

    /// The players on a board for this viewer, or None when the scope is not theirs to see.
    pub fn board(
        &self,
        conn: &Connection,
        viewer: &Viewer,
        scope: Option<Scope>,
    ) -> rusqlite::Result<Option<BTreeSet<String>>> {
        let scope = scope.unwrap_or(if viewer.member.is_some() {
            Scope::Circle
        } else {
            Scope::Global
        });
        Ok(match (scope, &viewer.member) {
            (Scope::Global, _) => {
                let mut users = self.global(conn)?;
                if viewer.community {
                    let unlisted = global_opt_outs(conn)?;
                    users.extend(
                        self.known_users()
                            .into_iter()
                            .filter(|u| self.legacy(u) && !unlisted.contains(u)),
                    );
                }
                Some(users)
            }
            (Scope::Circle, Some(member)) => Some(self.circle(conn, member)?),
            (Scope::Circle, None) => None,
            (Scope::Group(group), Some(member)) => {
                if is_member(conn, group, member)? {
                    Some(group_members(conn, group)?)
                } else {
                    None
                }
            }
            (Scope::Group(_), None) => None,
        })
    }
}

/// 24 hex characters.
fn new_invite() -> Result<String, Error> {
    let random = crate::web_util::random_bytes("invite randomness unavailable")?;
    Ok(crate::web_util::hex(&random[..12]))
}

fn clean_name(name: &str) -> Option<String> {
    let name = name.trim();
    crate::web_util::valid_title(name, MAX_GROUP_NAME).then(|| name.to_string())
}

/// A player as the community page shows them.
pub type Person = crate::decks::Recipient;

#[derive(Debug, Serialize)]
pub struct Group {
    pub id: i64,
    pub name: String,
    pub owner: bool,
    /// Only owners see the invite code.
    pub invite: Option<String>,
    pub members: Vec<Person>,
    /// Players the owner invited who have not joined, declined ones included until their
    /// invitation expires; only owners see them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub invited: Vec<Person>,
}

/// A group someone invited this player to, waiting for `accept_invite` or `decline_invite`.
#[derive(Debug, Serialize)]
pub struct Invitation {
    pub group: i64,
    pub name: String,
    /// How many members the group has.
    pub members: i64,
    pub from: Person,
}

#[derive(Debug, Serialize)]
pub struct Overview {
    pub groups: Vec<Group>,
    pub friends: Vec<Person>,
    pub incoming: Vec<Person>,
    pub outgoing: Vec<Person>,
    pub public: bool,
    /// People this player muted, by display name.
    pub muted: Vec<Person>,
    /// Groups this player is invited to, newest first, except by people they muted.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub invitations: Vec<Invitation>,
}

fn overview(app: &App, conn: &Connection, user: &str) -> rusqlite::Result<Overview> {
    let rows: Vec<(i64, String, String, bool)> = conn
        .prepare(
            "select g.id, g.name, g.invite, m.owner from groups g
             join group_members m on m.group_id = g.id where m.user = ?1 order by g.name, g.id",
        )?
        .query_map([user], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<Result<_, _>>()?;
    let cutoff = now_ms() - INVITATION_AGE_MS;
    let mut groups = Vec::new();
    for (id, name, invite, owner) in rows {
        let members = app.sorted_recipients(group_members(conn, id)?);
        let invited = if owner {
            app.sorted_recipients(invited(conn, id, cutoff)?)
        } else {
            Vec::new()
        };
        groups.push(Group {
            id,
            name,
            owner,
            invite: owner.then_some(invite),
            members,
            invited,
        });
    }
    let requests = |incoming: bool| -> rusqlite::Result<Vec<Person>> {
        let people = conn
            .prepare(
                "select case when a = ?1 then b else a end from friendships
                 where accepted = 0 and (a = ?1 or b = ?1) and (requested_by = ?1) = ?2",
            )?
            .query_map(params![user, !incoming], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(app.sorted_recipients(people))
    };
    let friend_list = app.sorted_recipients(friends(conn, user)?);
    let muted = app.sorted_recipients(
        conn.prepare("select sender from muted_people where recipient = ?1")?
            .query_map([user], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?,
    );
    Ok(Overview {
        groups,
        friends: friend_list,
        incoming: requests(true)?,
        outgoing: requests(false)?,
        public: app.is_public(conn, user)?,
        muted,
        invitations: invitations(app, conn, user, cutoff)?,
    })
}

/// Who was invited to `group` since `cutoff` and has not joined. Declined invitations count,
/// so the owner cannot tell a decline from no answer.
fn invited(conn: &Connection, group: i64, cutoff: i64) -> rusqlite::Result<Vec<String>> {
    conn.prepare("select user from group_invitations where group_id = ?1 and created_at >= ?2")?
        .query_map(params![group, cutoff], |r| r.get(0))?
        .collect()
}

/// The groups `user` is invited to since `cutoff`, leaving out invitations from people they muted.
fn invitations(
    app: &App,
    conn: &Connection,
    user: &str,
    cutoff: i64,
) -> rusqlite::Result<Vec<Invitation>> {
    let rows: Vec<(i64, String, i64, String)> = conn
        .prepare(&format!(
            "select g.id, g.name, (select count(*) from group_members m where m.group_id = g.id),
                    i.invited_by
             from group_invitations i join groups g on g.id = i.group_id
             where i.user = ?1 and i.created_at >= ?2 and i.state = 'pending' and not {}
             order by i.created_at desc, g.id",
            muted_sql("?1", "i.invited_by")
        ))?
        .query_map(params![user, cutoff], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows
        .into_iter()
        .map(|(group, name, members, from)| Invitation {
            group,
            name,
            members,
            from: app.recipient(from),
        })
        .collect())
}

fn failed(error: impl std::fmt::Display) -> Response {
    log_error!("social storage failed: {error}");
    json_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "That could not be saved. Please try again.",
    )
}

fn respond(app: &App, conn: &Connection, user: &str) -> Response {
    match overview(app, conn, user) {
        Ok(overview) => Json(overview).into_response(),
        Err(error) => failed(error),
    }
}

async fn get_overview(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !crate::access::authorized(&app, &user, &headers) {
        return json_error(StatusCode::UNAUTHORIZED, "Sign in first.");
    }
    respond(&app, &app.store.lock().unwrap().conn, &user)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(tag = "action", rename_all = "snake_case")]
enum Change {
    CreateGroup { name: String },
    RenameGroup { group: i64, name: String },
    NewInvite { group: i64 },
    Join { invite: String },
    Invite { group: i64, user: String },
    AcceptInvite { group: i64 },
    DeclineInvite { group: i64 },
    Leave { group: i64 },
    Remove { group: i64, member: String },
    Befriend { user: String },
    Unfriend { user: String },
    Mute { user: String },
    Unmute { user: String },
    Visibility { public: bool },
}

async fn change(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
    Json(change): Json<Change>,
) -> Response {
    if !crate::access::authorized(&app, &user, &headers) {
        return json_error(StatusCode::UNAUTHORIZED, "Sign in first.");
    }
    let store = app.store.lock().unwrap();
    let conn = &store.conn;
    let now = now_ms();
    let tx = match conn.unchecked_transaction() {
        Ok(tx) => tx,
        Err(error) => return failed(error),
    };
    let outcome: Result<Result<(), (StatusCode, &str)>, Error> = (|| {
        Ok(match change {
            Change::CreateGroup { name } => {
                let Some(name) = clean_name(&name) else {
                    return Ok(Err((
                        StatusCode::BAD_REQUEST,
                        "Group names are 1 to 40 characters.",
                    )));
                };
                let count: i64 = conn.query_row(
                    "select count(*) from group_members where user = ?1",
                    [&user],
                    |r| r.get(0),
                )?;
                if count >= MAX_GROUPS {
                    return Ok(Err((
                        StatusCode::CONFLICT,
                        "You are in as many groups as you can be.",
                    )));
                }
                conn.execute(
                    "insert into groups (name, invite, created_by, created_at) values (?1, ?2, ?3, ?4)",
                    params![name, new_invite()?, user, now],
                )?;
                conn.execute(
                    "insert into group_members (group_id, user, owner, joined_at) values (?1, ?2, 1, ?3)",
                    params![conn.last_insert_rowid(), user, now],
                )?;
                Ok(())
            }
            Change::RenameGroup { group, name } => {
                let Some(name) = clean_name(&name) else {
                    return Ok(Err((
                        StatusCode::BAD_REQUEST,
                        "Group names are 1 to 40 characters.",
                    )));
                };
                if !owns(conn, group, &user)? {
                    return Ok(Err((
                        StatusCode::FORBIDDEN,
                        "Only the group's owner can do that.",
                    )));
                }
                conn.execute(
                    "update groups set name = ?2 where id = ?1",
                    params![group, name],
                )?;
                Ok(())
            }
            Change::NewInvite { group } => {
                if !owns(conn, group, &user)? {
                    return Ok(Err((
                        StatusCode::FORBIDDEN,
                        "Only the group's owner can do that.",
                    )));
                }
                conn.execute(
                    "update groups set invite = ?2 where id = ?1",
                    params![group, new_invite()?],
                )?;
                Ok(())
            }
            Change::Join { invite } => {
                let group: Option<i64> = conn
                    .query_row(
                        "select id from groups where invite = ?1",
                        [invite.trim()],
                        |r| r.get(0),
                    )
                    .optional()?;
                let Some(group) = group else {
                    return Ok(Err((
                        StatusCode::NOT_FOUND,
                        "That invite link is not valid any more.",
                    )));
                };
                join(conn, group, &user, now)?
            }
            Change::Invite {
                group,
                user: invitee,
            } => invite_player(&app, conn, group, &user, &normalize_user(&invitee), now)?,
            Change::AcceptInvite { group } => {
                if invitation(conn, group, &user, now)? != Some(InvitationState::Pending) {
                    return Ok(Err((
                        StatusCode::NOT_FOUND,
                        "That invitation is not valid any more.",
                    )));
                }
                join(conn, group, &user, now)?
            }
            Change::DeclineInvite { group } => {
                decline(conn, group, &user, now)?;
                Ok(())
            }
            Change::Leave { group } => {
                leave(conn, group, &user)?;
                prune(&app, conn, &user)?;
                Ok(())
            }
            Change::Remove { group, member } => {
                if !owns(conn, group, &user)? || member == user {
                    return Ok(Err((
                        StatusCode::FORBIDDEN,
                        "Only the group's owner can remove others.",
                    )));
                }
                conn.execute(
                    "delete from group_members where group_id = ?1 and user = ?2",
                    params![group, member],
                )?;
                prune(&app, conn, &member)?;
                Ok(())
            }
            Change::Befriend { user: other } => {
                let other = normalize_user(&other);
                if other == user || !app.is_user(&other) {
                    return Ok(Err((
                        StatusCode::NOT_FOUND,
                        "There is nobody with that username.",
                    )));
                }
                let (a, b) = pair(&user, &other);
                let existing: Option<(String, bool)> = conn
                    .query_row(
                        "select requested_by, accepted from friendships where a = ?1 and b = ?2",
                        params![a, b],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                match existing {
                    Some((_, true)) => {}
                    Some((by, false)) if by != user => {
                        conn.execute(
                            "update friendships set accepted = 1 where a = ?1 and b = ?2",
                            params![a, b],
                        )?;
                    }
                    Some(_) => {}
                    None => {
                        let pending: i64 = conn.query_row(
                            "select count(*) from friendships where requested_by = ?1 and accepted = 0",
                            [&user],
                            |r| r.get(0),
                        )?;
                        if pending >= MAX_PENDING {
                            return Ok(Err((
                                StatusCode::CONFLICT,
                                "Wait for some friend requests to be answered first.",
                            )));
                        }
                        conn.execute(
                            "insert into friendships (a, b, requested_by, accepted, created_at) values (?1, ?2, ?3, 0, ?4)",
                            params![a, b, user, now],
                        )?;
                    }
                }
                Ok(())
            }
            Change::Unfriend { user: other } => {
                let other = normalize_user(&other);
                let (a, b) = pair(&user, &other);
                conn.execute(
                    "delete from friendships where a = ?1 and b = ?2",
                    params![a, b],
                )?;
                prune(&app, conn, &user)?;
                Ok(())
            }
            Change::Mute { user: other } => {
                let other = normalize_user(&other);
                if other == user || !app.is_user(&other) {
                    return Ok(Err((
                        StatusCode::NOT_FOUND,
                        "There is nobody with that username.",
                    )));
                }
                let count: i64 = conn.query_row(
                    "select count(*) from muted_people where recipient = ?1",
                    [&user],
                    |r| r.get(0),
                )?;
                if count >= MAX_MUTED && !muted(conn, &user, &other)? {
                    return Ok(Err((
                        StatusCode::CONFLICT,
                        "You muted as many people as you can. Unmute someone first.",
                    )));
                }
                conn.execute(
                    "insert or ignore into muted_people (recipient, sender, created_at) values (?1, ?2, ?3)",
                    params![user, other, now],
                )?;
                // What is already queued for the phone stays in the inbox but is not pushed.
                conn.execute(
                    "update notifications set push_cancelled = 1
                     where recipient = ?1 and sender = ?2 and pushed = 0",
                    params![user, other],
                )?;
                Ok(())
            }
            Change::Unmute { user: other } => {
                conn.execute(
                    "delete from muted_people where recipient = ?1 and sender = ?2",
                    params![user, normalize_user(&other)],
                )?;
                Ok(())
            }
            Change::Visibility { public } => {
                conn.execute(
                    "insert into visibility (user, public) values (?1, ?2)
                     on conflict (user) do update set public = excluded.public",
                    params![user, public],
                )?;
                Ok(())
            }
        })
    })();
    if matches!(outcome, Ok(Ok(()))) {
        if let Err(error) = tx.commit() {
            return failed(error);
        }
    } else {
        drop(tx);
    }
    match outcome {
        Ok(Ok(())) => respond(&app, conn, &user),
        Ok(Err((status, message))) => json_error(status, message),
        Err(error) => failed(error),
    }
}

type Refusal = (StatusCode, &'static str);

/// Puts `user` in `group`, by invite link or invitation, within the group limits.
fn join(conn: &Connection, group: i64, user: &str, now: i64) -> Result<Result<(), Refusal>, Error> {
    if is_member(conn, group, user)? {
        end_invitation(conn, group, user, now)?;
        return Ok(Ok(()));
    }
    let mine: i64 = conn.query_row(
        "select count(*) from group_members where user = ?1",
        [user],
        |r| r.get(0),
    )?;
    let size: i64 = conn.query_row(
        "select count(*) from group_members where group_id = ?1",
        [group],
        |r| r.get(0),
    )?;
    if mine >= MAX_GROUPS || size >= MAX_MEMBERS {
        return Ok(Err((
            StatusCode::CONFLICT,
            "That group, or your list of groups, is full.",
        )));
    }
    conn.execute(
        "insert into group_members (group_id, user, owner, joined_at) values (?1, ?2, 0, ?3)",
        params![group, user, now],
    )?;
    end_invitation(conn, group, user, now)?;
    Ok(Ok(()))
}

/// The state of `user`'s invitation to `group`, or `None` when there is none that has not expired.
fn invitation(
    conn: &Connection,
    group: i64,
    user: &str,
    now: i64,
) -> rusqlite::Result<Option<InvitationState>> {
    let state: Option<String> = conn
        .query_row(
            "select state from group_invitations
             where group_id = ?1 and user = ?2 and created_at >= ?3",
            params![group, user, now - INVITATION_AGE_MS],
            |r| r.get(0),
        )
        .optional()?;
    Ok(state.as_deref().and_then(InvitationState::parse))
}

/// Marks the inbox row of `user`'s invitation to `group` read, as it needs no answer any more.
fn read_invitation_row(
    conn: &Connection,
    group: i64,
    user: &str,
    now: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "update notifications set read_at = coalesce(read_at, ?3)
         where id = (select notification from group_invitations where group_id = ?1 and user = ?2)",
        params![group, user, now],
    )?;
    Ok(())
}

/// Declines a pending invitation. It stays until it expires, so inviting again does nothing.
fn decline(conn: &Connection, group: i64, user: &str, now: i64) -> rusqlite::Result<()> {
    if invitation(conn, group, user, now)? != Some(InvitationState::Pending) {
        return Ok(());
    }
    read_invitation_row(conn, group, user, now)?;
    conn.execute(
        "update group_invitations set state = 'declined' where group_id = ?1 and user = ?2",
        params![group, user],
    )?;
    Ok(())
}

/// Ends `user`'s invitation to `group` once they joined it.
fn end_invitation(conn: &Connection, group: i64, user: &str, now: i64) -> rusqlite::Result<()> {
    read_invitation_row(conn, group, user, now)?;
    conn.execute(
        "delete from group_invitations where group_id = ?1 and user = ?2",
        params![group, user],
    )?;
    Ok(())
}

/// The owner of `group` invites `invitee`, who gets an inbox row unless they muted the owner.
/// Inviting again while an invitation waits, or before a declined one expires, changes nothing.
fn invite_player(
    app: &App,
    conn: &Connection,
    group: i64,
    owner: &str,
    invitee: &str,
    now: i64,
) -> Result<Result<(), Refusal>, Error> {
    // Only owners invite, as only they hold the group's invite link.
    if !owns(conn, group, owner)? {
        return Ok(Err((
            StatusCode::FORBIDDEN,
            "Only the group's owner can do that.",
        )));
    }
    let nobody = (StatusCode::NOT_FOUND, "There is nobody with that username.");
    if !app.is_user(invitee) {
        return Ok(Err(nobody));
    }
    if is_member(conn, group, invitee)? {
        return Ok(Err((
            StatusCode::CONFLICT,
            "They are already in that group.",
        )));
    }

    // The owner can invite only someone whose profile they can see.
    let viewer = Viewer {
        member: Some(owner.to_string()),
        community: false,
    };
    if !app.visible(conn, &viewer, invitee)? {
        return Ok(Err(nobody));
    }
    if invitation(conn, group, invitee, now)?.is_some() {
        return Ok(Ok(()));
    }
    let waiting: i64 = conn.query_row(
        "select count(*) from group_invitations where group_id = ?1 and created_at >= ?2",
        params![group, now - INVITATION_AGE_MS],
        |r| r.get(0),
    )?;
    if waiting >= MAX_INVITATIONS {
        return Ok(Err((
            StatusCode::CONFLICT,
            "Wait for some invitations to be answered first.",
        )));
    }

    // An expired invitation is replaced by the new one.
    conn.execute(
        "insert into group_invitations (group_id, user, invited_by, created_at) values (?1, ?2, ?3, ?4)
         on conflict (group_id, user) do update
         set invited_by = excluded.invited_by, created_at = excluded.created_at,
             state = 'pending', notification = null",
        params![group, invitee, owner, now],
    )?;

    // A muted owner's invitation waits like any other, but nothing tells the invitee.
    if muted(conn, invitee, owner)? {
        return Ok(Ok(()));
    }
    let name: String = conn.query_row("select name from groups where id = ?1", [group], |r| {
        r.get(0)
    })?;
    let id = crate::notifications::insert_notification(
        conn,
        &crate::notifications::Outgoing {
            to: invitee,
            from: owner,
            title: crate::i18n::phrase("Group invitation"),
            body: &crate::i18n::english("{0} invited you to {1}.", &[&app.display(owner), &name]),
            kind: "group_invite",
        },
        crate::store::clock_in(conn, invitee)?.day(now),
        now,
        false,
    )?;
    conn.execute(
        "update notifications set group_id = ?1 where id = ?2",
        params![group, id],
    )?;
    conn.execute(
        "update group_invitations set notification = ?3 where group_id = ?1 and user = ?2",
        params![group, invitee, id],
    )?;
    Ok(Ok(()))
}

/// Takes `user` out of a group. The longest-standing member inherits it, and an empty group is deleted.
pub fn leave(conn: &Connection, group: i64, user: &str) -> rusqlite::Result<()> {
    let owner = owns(conn, group, user)?;
    conn.execute(
        "delete from group_members where group_id = ?1 and user = ?2",
        params![group, user],
    )?;
    let next: Option<String> = conn
        .query_row(
            "select user from group_members where group_id = ?1 order by joined_at, user limit 1",
            [group],
            |r| r.get(0),
        )
        .optional()?;
    match next {
        None => {
            conn.execute("delete from groups where id = ?1", [group])?;
        }
        Some(next)
            if owner
                && !conn
                    .prepare("select 1 from group_members where group_id = ?1 and owner = 1")?
                    .exists([group])? =>
        {
            conn.execute(
                "update group_members set owner = 1 where group_id = ?1 and user = ?2",
                params![group, next],
            )?;
        }
        Some(_) => {}
    }
    Ok(())
}

/// Stops deck notifications between people who no longer share a group or friendship.
fn prune(app: &App, conn: &Connection, user: &str) -> rusqlite::Result<()> {
    prune_outside(conn, user, &app.circle(conn, user)?).map(|_| ())
}

/// Deletes deck recipients between `user` and anyone outside their `circle`, and returns how many.
pub(crate) fn prune_outside(
    conn: &Connection,
    user: &str,
    circle: &BTreeSet<String>,
) -> rusqlite::Result<usize> {
    let mut removed = 0;
    let pairs: Vec<(String, String)> = conn
        .prepare("select distinct owner, recipient from deck_recipients where owner = ?1 or recipient = ?1")?
        .query_map([user], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    for (owner, recipient) in pairs {
        let other = if owner == user { &recipient } else { &owner };
        if !circle.contains(other) {
            removed += conn.execute(
                "delete from deck_recipients where owner = ?1 and recipient = ?2",
                params![owner, recipient],
            )?;
        }
    }
    Ok(removed)
}

pub(crate) fn owns(conn: &Connection, group: i64, user: &str) -> rusqlite::Result<bool> {
    conn.prepare("select 1 from group_members where group_id = ?1 and user = ?2 and owner = 1")?
        .exists(params![group, user])
}

/// What an invite link shows before joining: the group's name and size.
async fn invite(State(app): State<Arc<App>>, Path(code): Path<String>) -> Response {
    let store = app.store.lock().unwrap();
    let found: Result<Option<(String, i64)>, rusqlite::Error> = store
        .conn
        .query_row(
            "select g.name, count(m.user) from groups g left join group_members m on m.group_id = g.id
             where g.invite = ?1 group by g.id",
            [code.trim()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional();
    match found {
        Ok(Some((name, members))) => {
            Json(serde_json::json!({"name": name, "members": members})).into_response()
        }
        Ok(None) => json_error(
            StatusCode::NOT_FOUND,
            "That invite link is not valid any more.",
        ),
        Err(error) => failed(error),
    }
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/social/{user}", get(get_overview).post(change))
        .route("/api/invites/{code}", get(invite))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{call, cleanup, sign_up, with_reviews};
    use std::path::PathBuf;

    fn fixture() -> (Arc<App>, PathBuf) {
        crate::test_support::app_with(serde_json::json!({
            "registration": true,
            "users": {"alice": {"token": "alice-token"}, "bob": {"token": "bob-token"}}
        }))
    }

    fn circle(app: &App, user: &str) -> Vec<String> {
        app.circle(&app.store.lock().unwrap().conn, user)
            .unwrap()
            .into_iter()
            .collect()
    }

    #[tokio::test]
    async fn configured_players_keep_their_community_and_accounts_start_alone() {
        let (app, path) = fixture();
        sign_up(&app, "ana").await;
        // A stranger to everyone who has studied, so profiles' own visibility decides.
        sign_up(&app, "carl").await;
        with_reviews(&app, "carl", 1);
        let stranger = Viewer {
            member: Some("carl".into()),
            community: false,
        };
        assert_eq!(circle(&app, "alice"), ["alice", "bob"]);
        assert_eq!(circle(&app, "ana"), ["ana"]);
        let store = app.store.lock().unwrap();
        let conn = &store.conn;
        assert!(
            app.is_public(conn, "alice").unwrap(),
            "legacy profiles stay as public as before"
        );
        assert!(
            app.is_public(conn, "ana").unwrap(),
            "accounts start public too"
        );
        assert!(app.chose_public(conn, "alice").unwrap());
        assert!(
            !app.chose_public(conn, "ana").unwrap(),
            "but never chose it, which keeps conquest labels for friends"
        );
        let anonymous = Viewer {
            member: None,
            community: false,
        };
        assert!(app.visible(conn, &stranger, "ana").unwrap());
        assert!(!app.visible_by_choice(conn, &stranger, "ana").unwrap());
        assert!(
            !app.visible(conn, &anonymous, "ana").unwrap(),
            "nobody's study reaches visitors who are not signed in"
        );
        assert_eq!(
            app.board(conn, &anonymous, None)
                .unwrap()
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>(),
            ["alice", "ana", "bob", "carl"]
        );
        conn.execute(
            "insert into visibility (user, public) values ('ana', 0)",
            [],
        )
        .unwrap();
        assert!(
            !app.is_public(conn, "ana").unwrap(),
            "a private profile stays private"
        );
        assert!(!app.visible(conn, &stranger, "ana").unwrap());
        assert!(
            app.visible(
                conn,
                &Viewer {
                    member: Some("ana".into()),
                    community: false
                },
                "ana"
            )
            .unwrap()
        );
        assert_eq!(
            app.board(conn, &anonymous, None)
                .unwrap()
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>(),
            ["alice", "bob", "carl"]
        );
        conn.execute(
            "insert into player_settings (user, global_leaderboard) values ('bob', 0)",
            [],
        )
        .unwrap();
        assert_eq!(
            app.board(conn, &anonymous, None)
                .unwrap()
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>(),
            ["alice", "carl"],
            "a public profile that left the global board is not listed there"
        );
        assert!(app.visible(conn, &stranger, "bob").unwrap());
        drop(store);
        cleanup(app, path);
    }

    #[tokio::test]
    async fn an_invite_link_joins_a_group_whose_members_see_each_other() {
        let (app, path) = fixture();
        let ana = sign_up(&app, "ana").await;
        let bo = sign_up(&app, "bobby").await;
        let (status, overview) = call(
            &app,
            "POST",
            "/api/social/ana",
            &ana,
            serde_json::json!({"action": "create_group", "name": "Spanish club"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{overview}");
        let group = overview["groups"][0]["id"].as_i64().unwrap();
        let invite = overview["groups"][0]["invite"]
            .as_str()
            .unwrap()
            .to_string();

        let (status, preview) = call(
            &app,
            "GET",
            &format!("/api/invites/{invite}"),
            &bo,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(
            (status, preview["name"].as_str()),
            (StatusCode::OK, Some("Spanish club"))
        );
        let (status, joined) = call(
            &app,
            "POST",
            "/api/social/bobby",
            &bo,
            serde_json::json!({"action": "join", "invite": invite}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            joined["groups"][0]["invite"].is_null(),
            "only the owner sees the invite"
        );
        assert_eq!(circle(&app, "bobby"), ["ana", "bobby"]);
        with_reviews(&app, "bobby", 1);
        with_reviews(&app, "alice", 1);
        {
            let conn = &app.store.lock().unwrap().conn;
            let viewer = Viewer {
                member: Some("bobby".into()),
                community: false,
            };
            assert!(app.visible(conn, &viewer, "ana").unwrap());
            assert_eq!(
                app.board(conn, &viewer, Some(Scope::Group(group)))
                    .unwrap()
                    .unwrap()
                    .len(),
                2
            );
            let outsider = Viewer {
                member: Some("alice".into()),
                community: false,
            };
            assert!(
                app.board(conn, &outsider, Some(Scope::Group(group)))
                    .unwrap()
                    .is_none(),
                "other groups' boards stay private"
            );
            assert!(
                app.visible(conn, &outsider, "ana").unwrap(),
                "a profile nobody made private is public"
            );
            assert!(!app.visible_by_choice(conn, &outsider, "ana").unwrap());
            assert!(app.visible_by_choice(conn, &viewer, "ana").unwrap());
        }

        let (status, _) = call(
            &app,
            "POST",
            "/api/social/ana",
            &ana,
            serde_json::json!({"action": "new_invite", "group": group}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = call(
            &app,
            "GET",
            &format!("/api/invites/{invite}"),
            &bo,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "a new link retires the old one"
        );
        let (status, _) = call(
            &app,
            "POST",
            "/api/social/bobby",
            &bo,
            serde_json::json!({"action": "new_invite", "group": group}),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let (status, _) = call(
            &app,
            "POST",
            "/api/social/ana",
            &ana,
            serde_json::json!({"action": "leave", "group": group}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (_, overview) = call(
            &app,
            "GET",
            "/api/social/bobby",
            &bo,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(
            overview["groups"][0]["owner"], true,
            "the next member inherits the group"
        );
        let (_, overview) = call(
            &app,
            "POST",
            "/api/social/bobby",
            &bo,
            serde_json::json!({"action": "leave", "group": group}),
        )
        .await;
        assert_eq!(overview["groups"].as_array().unwrap().len(), 0);
        let count: i64 = app
            .store
            .lock()
            .unwrap()
            .conn
            .query_row("select count(*) from groups", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "an empty group is deleted");
        cleanup(app, path);
    }

    #[tokio::test]
    async fn a_muted_person_reaches_nobody_until_unmuted() {
        let (app, path) = fixture();
        let ana = sign_up(&app, "ana").await;
        sign_up(&app, "bobby").await;
        let mute = |action: &'static str, who: &'static str| {
            let (app, ana) = (app.clone(), ana.clone());
            async move {
                call(
                    &app,
                    "POST",
                    "/api/social/ana",
                    &ana,
                    serde_json::json!({"action": action, "user": who}),
                )
                .await
            }
        };
        assert_eq!(mute("mute", "ana").await.0, StatusCode::NOT_FOUND);
        assert_eq!(mute("mute", "nobody").await.0, StatusCode::NOT_FOUND);
        {
            let mut store = app.store.lock().unwrap();
            store.set_friend_nudges("ana", true).unwrap();
            store
                .conn
                .execute(
                    "insert into notifications (recipient, sender, title, body, day, created_at, kind)
                     values ('ana', 'bobby', 't', 'b', 0, 0, 'conquest')",
                    [],
                )
                .unwrap();
            assert!(store.friend_nudges_enabled("ana", "bobby").unwrap());
        }
        let (status, overview) = mute("mute", "Bobby").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(overview["muted"][0]["user"], "bobby");
        assert_eq!(
            mute("mute", "bobby").await.0,
            StatusCode::OK,
            "twice is fine"
        );
        {
            let store = app.store.lock().unwrap();
            assert!(muted(&store.conn, "ana", "bobby").unwrap());
            assert!(!muted(&store.conn, "bobby", "ana").unwrap(), "one way");
            assert!(!store.friend_nudges_enabled("ana", "bobby").unwrap());
            let cancelled: bool = store
                .conn
                .query_row(
                    "select push_cancelled from notifications where sender = 'bobby'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(cancelled, "what was waiting is not pushed");
        }
        let (status, overview) = mute("unmute", "bobby").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(overview["muted"], serde_json::json!([]));
        assert!(
            app.store
                .lock()
                .unwrap()
                .friend_nudges_enabled("ana", "bobby")
                .unwrap()
        );
        cleanup(app, path);
    }

    #[tokio::test]
    async fn friends_need_both_sides_and_profiles_can_go_public() {
        let (app, path) = fixture();
        let ana = sign_up(&app, "ana").await;
        let bo = sign_up(&app, "bobby").await;
        let (_, overview) = call(
            &app,
            "POST",
            "/api/social/ana",
            &ana,
            serde_json::json!({"action": "befriend", "user": "bobby"}),
        )
        .await;
        assert_eq!(overview["outgoing"][0]["user"], "bobby");
        assert_eq!(
            circle(&app, "ana"),
            ["ana"],
            "a request alone connects nobody"
        );
        let (_, overview) = call(
            &app,
            "GET",
            "/api/social/bobby",
            &bo,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(overview["incoming"][0]["user"], "ana");
        let (_, overview) = call(
            &app,
            "POST",
            "/api/social/bobby",
            &bo,
            serde_json::json!({"action": "befriend", "user": "ana"}),
        )
        .await;
        assert_eq!(overview["friends"][0]["user"], "ana");
        assert_eq!(circle(&app, "ana"), ["ana", "bobby"]);
        let (status, _) = call(
            &app,
            "POST",
            "/api/social/ana",
            &ana,
            serde_json::json!({"action": "befriend", "user": "nobody-here"}),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        app.store
            .lock()
            .unwrap()
            .conn
            .execute("insert into deck_recipients (owner, deck_id, recipient) values ('ana', 'spanish', 'bobby')", [])
            .unwrap();
        call(
            &app,
            "POST",
            "/api/social/bobby",
            &bo,
            serde_json::json!({"action": "unfriend", "user": "ana"}),
        )
        .await;
        assert_eq!(circle(&app, "ana"), ["ana"]);
        let left: i64 = app
            .store
            .lock()
            .unwrap()
            .conn
            .query_row("select count(*) from deck_recipients", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0, "unfriending stops deck notifications");

        let (_, overview) = call(
            &app,
            "POST",
            "/api/social/ana",
            &ana,
            serde_json::json!({"action": "visibility", "public": true}),
        )
        .await;
        assert_eq!(overview["public"], true);
        with_reviews(&app, "alice", 1);
        let store = app.store.lock().unwrap();
        let conn = &store.conn;
        assert!(
            app.visible(
                conn,
                &Viewer {
                    member: Some("alice".into()),
                    community: false
                },
                "ana"
            )
            .unwrap()
        );
        assert!(app.global(conn).unwrap().contains("ana"));
        drop(store);
        cleanup(app, path);
    }

    #[tokio::test]
    async fn only_the_owner_changes_their_social_life() {
        let (app, path) = fixture();
        let ana = sign_up(&app, "ana").await;
        let (status, _) = call(
            &app,
            "POST",
            "/api/social/bobby",
            &ana,
            serde_json::json!({"action": "visibility", "public": true}),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = call(
            &app,
            "GET",
            "/api/social/alice",
            &ana,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        cleanup(app, path);
    }

    async fn study(app: &Arc<App>, user: &str, token: &str) {
        let now = now_ms();
        let reviews: Vec<_> = (0..5)
            .map(|i| serde_json::json!({"id": now - 60_000 + i, "cid": 1_000 + i, "last_ivl": 1, "time_ms": 8_000, "kind": 1}))
            .collect();
        let body = serde_json::json!({"reviews": reviews, "clock": {"offset_west_min": 0, "rollover_hour": 4}, "decks": null});
        let (status, _) = call(app, "POST", &format!("/api/reviews/{user}"), token, body).await;
        assert_eq!(status, StatusCode::OK);
    }

    fn users(board: &serde_json::Value) -> Vec<&str> {
        let mut users: Vec<&str> = board
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["user"].as_str().unwrap())
            .collect();
        users.sort();
        users
    }

    #[tokio::test]
    async fn strangers_stay_off_each_others_boards_profiles_and_lists() {
        let (app, path) = fixture();
        let ana = sign_up(&app, "ana").await;
        let bo = sign_up(&app, "bobby").await;
        study(&app, "ana", &ana).await;
        study(&app, "bobby", &bo).await;
        study(&app, "alice", "alice-token").await;
        let global = || {
            call(
                &app,
                "GET",
                "/api/leaderboard?scope=global",
                &ana,
                serde_json::Value::Null,
            )
        };
        let (_, board) = global().await;
        assert_eq!(
            users(&board),
            ["alice", "ana", "bobby"],
            "profiles start public"
        );
        let (status, _) = call(
            &app,
            "GET",
            "/api/profile/bobby",
            &ana,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (_, overview) = call(
            &app,
            "POST",
            "/api/social/bobby",
            &bo,
            serde_json::json!({"action": "visibility", "public": false}),
        )
        .await;
        assert_eq!(overview["public"], false);

        let (_, board) = call(
            &app,
            "GET",
            "/api/leaderboard",
            &ana,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(users(&board), ["ana"]);
        let (_, board) = global().await;
        assert_eq!(
            users(&board),
            ["alice", "ana"],
            "the global board lists public profiles and the viewer, not Bobby who went private"
        );
        let (_, board) = call(
            &app,
            "GET",
            "/api/leaderboard",
            "alice-token",
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(
            users(&board),
            ["alice"],
            "the original community does not see new accounts"
        );
        let (status, _) = call(
            &app,
            "GET",
            "/api/leaderboard?scope=group:1",
            &ana,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = call(
            &app,
            "GET",
            "/api/profile/bobby",
            &ana,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (_, inbox) = call(
            &app,
            "GET",
            "/api/deck-copies/ana",
            &ana,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(inbox["friends"], serde_json::json!([]));
        let (_, nudges) = call(
            &app,
            "GET",
            "/api/friend-nudges/ana",
            &ana,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(nudges["friends"], serde_json::json!([]));
        let (status, _) = call(
            &app,
            "POST",
            "/api/friend-nudges/ana",
            &ana,
            serde_json::json!({"recipient": "bobby"}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        call(
            &app,
            "POST",
            "/api/social/ana",
            &ana,
            serde_json::json!({"action": "befriend", "user": "bobby"}),
        )
        .await;
        call(
            &app,
            "POST",
            "/api/social/bobby",
            &bo,
            serde_json::json!({"action": "befriend", "user": "ana"}),
        )
        .await;
        let (_, board) = call(
            &app,
            "GET",
            "/api/leaderboard",
            &ana,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(users(&board), ["ana", "bobby"]);
        let (status, _) = call(
            &app,
            "GET",
            "/api/profile/bobby",
            &ana,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (_, inbox) = call(
            &app,
            "GET",
            "/api/deck-copies/ana",
            &ana,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(inbox["friends"][0]["user"], "bobby");
        let (_, records) = call(&app, "GET", "/api/records", &ana, serde_json::Value::Null).await;
        assert!(!records.to_string().contains("alice"));
        cleanup(app, path);
    }

    /// Ana owns "Spanish club"; Bobby and Cyril have accounts and have studied.
    async fn club() -> (Arc<App>, PathBuf, String, String, String, i64) {
        let (app, path) = fixture();
        let ana = sign_up(&app, "ana").await;
        let bo = sign_up(&app, "bobby").await;
        let cy = sign_up(&app, "cyril").await;
        for user in ["ana", "bobby", "cyril"] {
            with_reviews(&app, user, 1);
        }
        let (_, created) = call(
            &app,
            "POST",
            "/api/social/ana",
            &ana,
            serde_json::json!({"action": "create_group", "name": "Spanish club"}),
        )
        .await;
        let group = created["groups"][0]["id"].as_i64().unwrap();
        (app, path, ana, bo, cy, group)
    }

    async fn act(
        app: &Arc<App>,
        user: &str,
        token: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        call(app, "POST", &format!("/api/social/{user}"), token, body).await
    }

    async fn invite_rows(app: &Arc<App>, user: &str, token: &str) -> Vec<serde_json::Value> {
        let (_, activity) = call(
            app,
            "GET",
            &format!("/api/activity/{user}"),
            token,
            serde_json::Value::Null,
        )
        .await;
        activity["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["kind"] == "group_invite")
            .cloned()
            .collect()
    }

    #[tokio::test]
    async fn an_owner_invites_a_player_who_accepts_in_the_app() {
        let (app, path, ana, bo, cy, group) = club().await;

        // The invitation shows for the owner, for the invitee and in the invitee's inbox.
        let (status, overview) = act(
            &app,
            "ana",
            &ana,
            serde_json::json!({"action": "invite", "group": group, "user": "bobby"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{overview}");
        assert_eq!(overview["groups"][0]["invited"][0]["user"], "bobby");
        let (_, mine) = call(
            &app,
            "GET",
            "/api/social/bobby",
            &bo,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(
            mine["invitations"],
            serde_json::json!([{
                "group": group,
                "name": "Spanish club",
                "members": 1,
                "from": {"user": "ana", "display": "ana"}
            }])
        );
        assert!(mine["groups"].as_array().unwrap().is_empty());
        let rows = invite_rows(&app, "bobby", &bo).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (
                &rows[0]["sender"],
                &rows[0]["group"],
                &rows[0]["action_required"]
            ),
            (
                &serde_json::json!("ana"),
                &serde_json::json!(group),
                &serde_json::json!(true)
            )
        );

        // Inviting again changes nothing and notifies nobody twice.
        let (status, _) = act(
            &app,
            "ana",
            &ana,
            serde_json::json!({"action": "invite", "group": group, "user": "bobby"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(invite_rows(&app, "bobby", &bo).await.len(), 1);

        // Only the owner invites, as only the owner holds the invite link.
        let (status, _) = act(
            &app,
            "bobby",
            &bo,
            serde_json::json!({"action": "invite", "group": group, "user": "cyril"}),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        for (who, expected) in [
            ("nobody", StatusCode::NOT_FOUND),
            ("ana", StatusCode::CONFLICT),
        ] {
            let (status, _) = act(
                &app,
                "ana",
                &ana,
                serde_json::json!({"action": "invite", "group": group, "user": who}),
            )
            .await;
            assert_eq!(status, expected, "{who}");
        }

        // Accepting joins the group, answers the inbox row and cannot happen twice.
        let (status, joined) = act(
            &app,
            "bobby",
            &bo,
            serde_json::json!({"action": "accept_invite", "group": group}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{joined}");
        assert_eq!(joined["groups"][0]["id"], group);
        assert!(joined.get("invitations").is_none());
        let rows = invite_rows(&app, "bobby", &bo).await;
        assert_eq!(rows[0]["action_required"], false);
        assert!(rows[0]["read_at"].is_number());
        assert_eq!(circle(&app, "bobby"), ["ana", "bobby"]);
        let (status, _) = act(
            &app,
            "bobby",
            &bo,
            serde_json::json!({"action": "accept_invite", "group": group}),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = act(
            &app,
            "ana",
            &ana,
            serde_json::json!({"action": "invite", "group": group, "user": "bobby"}),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "Bobby is a member now");
        let _ = cy;
        cleanup(app, path);
    }

    #[tokio::test]
    async fn a_declined_expired_or_deleted_invitation_joins_nothing() {
        let (app, path, ana, bo, cy, group) = club().await;
        let invite = |who: &'static str| {
            let (app, ana) = (app.clone(), ana.clone());
            async move {
                act(
                    &app,
                    "ana",
                    &ana,
                    serde_json::json!({"action": "invite", "group": group, "user": who}),
                )
                .await
                .0
            }
        };

        // Declining removes it for Bobby only; declining again is fine.
        assert_eq!(invite("bobby").await, StatusCode::OK);
        for _ in 0..2 {
            let (status, overview) = act(
                &app,
                "bobby",
                &bo,
                serde_json::json!({"action": "decline_invite", "group": group}),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert!(overview.get("invitations").is_none());
        }
        assert_eq!(
            invite_rows(&app, "bobby", &bo).await[0]["action_required"],
            false
        );
        let (_, owner) = call(
            &app,
            "GET",
            "/api/social/ana",
            &ana,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(
            owner["groups"][0]["invited"][0]["user"], "bobby",
            "Ana cannot tell Bobby declined"
        );

        // An invitation lasts as long as the inbox keeps its row.
        assert_eq!(invite("cyril").await, StatusCode::OK);
        app.store
            .lock()
            .unwrap()
            .conn
            .execute(
                "update group_invitations set created_at = created_at - ?1",
                [INVITATION_AGE_MS + 1],
            )
            .unwrap();
        let (_, mine) = call(
            &app,
            "GET",
            "/api/social/cyril",
            &cy,
            serde_json::Value::Null,
        )
        .await;
        assert!(mine.get("invitations").is_none());
        let (status, _) = act(
            &app,
            "cyril",
            &cy,
            serde_json::json!({"action": "accept_invite", "group": group}),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // A deleted group takes its invitations with it.
        assert_eq!(invite("cyril").await, StatusCode::OK);
        act(
            &app,
            "ana",
            &ana,
            serde_json::json!({"action": "leave", "group": group}),
        )
        .await;
        let (_, mine) = call(
            &app,
            "GET",
            "/api/social/cyril",
            &cy,
            serde_json::Value::Null,
        )
        .await;
        assert!(mine.get("invitations").is_none());
        assert_eq!(
            invite_rows(&app, "cyril", &cy).await[0]["action_required"],
            false
        );
        cleanup(app, path);
    }

    /// Bobby's group invitation rows, newest first, with whether each needs an answer, and
    /// his `action_count`.
    async fn invite_actions(app: &Arc<App>, token: &str) -> (Vec<bool>, u64) {
        let (_, activity) = call(
            app,
            "GET",
            "/api/activity/bobby",
            token,
            serde_json::Value::Null,
        )
        .await;
        let actions = activity["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["kind"] == "group_invite")
            .map(|row| row["action_required"].as_bool().unwrap())
            .collect();
        (actions, activity["action_count"].as_u64().unwrap())
    }

    #[tokio::test]
    async fn answered_invitations_stay_answered_after_reinviting() {
        let (app, path, ana, bo, _, group) = club().await;
        let invite = || {
            let (app, ana) = (app.clone(), ana.clone());
            async move {
                act(
                    &app,
                    "ana",
                    &ana,
                    serde_json::json!({"action": "invite", "group": group, "user": "bobby"}),
                )
                .await
            }
        };
        let answer = |action: &'static str| {
            let (app, bo) = (app.clone(), bo.clone());
            async move {
                act(
                    &app,
                    "bobby",
                    &bo,
                    serde_json::json!({"action": action, "group": group}),
                )
                .await
                .0
            }
        };

        // Declined: re-inviting during the invitation's lifetime changes nothing for anyone,
        // and Ana still sees Bobby as invited, so she cannot tell he declined.
        assert_eq!(invite().await.0, StatusCode::OK);
        assert_eq!(answer("decline_invite").await, StatusCode::OK);
        let (status, overview) = invite().await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(overview["groups"][0]["invited"][0]["user"], "bobby");
        assert_eq!(invite_actions(&app, &bo).await, (vec![false], 0));
        let (_, mine) = call(
            &app,
            "GET",
            "/api/social/bobby",
            &bo,
            serde_json::Value::Null,
        )
        .await;
        assert!(mine.get("invitations").is_none());
        assert_eq!(answer("accept_invite").await, StatusCode::NOT_FOUND);

        // Once the declined invitation has expired, a new one needs an answer; the old row does not.
        app.store
            .lock()
            .unwrap()
            .conn
            .execute(
                "update group_invitations set created_at = created_at - ?1",
                [INVITATION_AGE_MS + 1],
            )
            .unwrap();
        assert_eq!(invite().await.0, StatusCode::OK);
        assert_eq!(invite_actions(&app, &bo).await, (vec![true, false], 1));

        // Accepted, left, invited again: only the newest row needs an answer.
        assert_eq!(answer("accept_invite").await, StatusCode::OK);
        assert_eq!(invite_actions(&app, &bo).await, (vec![false, false], 0));
        let (status, _) = act(
            &app,
            "bobby",
            &bo,
            serde_json::json!({"action": "leave", "group": group}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(invite().await.0, StatusCode::OK);
        assert_eq!(
            invite_actions(&app, &bo).await,
            (vec![true, false, false], 1)
        );
        let (status, needs) = call(
            &app,
            "GET",
            "/api/activity/bobby?category=needs_action",
            &bo,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(needs["items"].as_array().unwrap().len(), 1);
        cleanup(app, path);
    }

    #[tokio::test]
    async fn a_muted_owner_s_invitation_is_not_delivered() {
        let (app, path, ana, bo, _, group) = club().await;
        act(
            &app,
            "bobby",
            &bo,
            serde_json::json!({"action": "mute", "user": "ana"}),
        )
        .await;
        let (status, overview) = act(
            &app,
            "ana",
            &ana,
            serde_json::json!({"action": "invite", "group": group, "user": "bobby"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            overview["groups"][0]["invited"][0]["user"], "bobby",
            "Ana cannot tell she was muted"
        );
        let (_, mine) = call(
            &app,
            "GET",
            "/api/social/bobby",
            &bo,
            serde_json::Value::Null,
        )
        .await;
        assert!(mine.get("invitations").is_none());
        assert!(invite_rows(&app, "bobby", &bo).await.is_empty());
        cleanup(app, path);
    }

    #[tokio::test]
    async fn an_invite_link_still_joins_and_answers_an_invitation() {
        let (app, path, ana, bo, _, group) = club().await;
        let (_, overview) = act(
            &app,
            "ana",
            &ana,
            serde_json::json!({"action": "invite", "group": group, "user": "bobby"}),
        )
        .await;
        let code = overview["groups"][0]["invite"].clone();
        let (status, joined) = act(
            &app,
            "bobby",
            &bo,
            serde_json::json!({"action": "join", "invite": code}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(joined["groups"][0]["id"], group);
        assert!(joined.get("invitations").is_none());
        assert_eq!(
            invite_rows(&app, "bobby", &bo).await[0]["action_required"],
            false
        );
        cleanup(app, path);
    }

    #[test]
    fn scopes_parse() {
        assert_eq!(Scope::parse(None, None), Some(None));
        assert_eq!(
            Scope::parse(Some("circle"), None),
            Some(Some(Scope::Circle))
        );
        assert_eq!(
            Scope::parse(Some("group:7"), None),
            Some(Some(Scope::Group(7)))
        );
        assert_eq!(Scope::parse(Some("group:x"), None), None);
        assert_eq!(Scope::parse(Some("everyone"), None), None);
        assert_eq!(
            Scope::parse(Some("friends"), None),
            Some(Some(Scope::Circle))
        );
        assert_eq!(
            Scope::parse(Some("group"), Some("7")),
            Some(Some(Scope::Group(7)))
        );
        assert_eq!(Scope::parse(Some("group"), None), None);
        assert_eq!(Scope::parse(Some("group"), Some("x")), None);
    }
}
