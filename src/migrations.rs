//! Schema changes that run once, in order. `pragma user_version` counts the steps a database has had.
//!
//! Each module's `initialize()` still creates its tables with `create table if not exists` on every
//! start. Anything else (a new column, a backfill, a rename, a cleanup) is a new step at the end of
//! `STEPS`. Never edit or reorder a step that has shipped.

use crate::notifications::NUDGE_ALLOWED_SQL;
use crate::store::Error;
use rusqlite::Connection;

type Step = fn(&Connection) -> Result<(), Error>;

const STEPS: &[Step] = &[
    baseline,
    notification_fetches,
    global_leaderboard,
    global_leaderboard_choice,
    adopted_device_names,
    share_notes,
    cow_companion,
    companion_milestones,
    earned_companions,
    share_receipts,
    quest_companions,
    group_invitations,
];

/// Brings the database up to the latest step, then turns foreign keys on for the connection.
pub fn run(conn: &Connection) -> Result<(), Error> {
    run_steps(conn, STEPS)
}

fn run_steps(conn: &Connection, steps: &[Step]) -> Result<(), Error> {
    let version: i64 = conn.query_row("pragma user_version", [], |r| r.get(0))?;
    if usize::try_from(version).map_or(true, |version| version > steps.len()) {
        return Err(format!(
            "the database is at schema version {version}, but this server only knows {}; run a newer server",
            steps.len()
        )
        .into());
    }
    // Steps run with foreign keys off, as SQLite asks for when a step rebuilds a table, and
    // each one checks them itself before it commits.
    conn.execute_batch("pragma foreign_keys = off;")?;
    let ran = steps
        .iter()
        .enumerate()
        .skip(version as usize)
        .try_for_each(|(done, step)| {
            let transaction = conn.unchecked_transaction()?;
            step(&transaction)?;
            let broken: i64 = transaction.query_row(
                "select count(*) from pragma_foreign_key_check",
                [],
                |r| r.get(0),
            )?;
            if broken > 0 {
                return Err(format!(
                    "schema step {} leaves {broken} rows pointing at a missing parent",
                    done + 1
                )
                .into());
            }
            transaction.pragma_update(None, "user_version", (done + 1) as i64)?;
            transaction.commit()?;
            Ok::<_, Error>(())
        });
    // The bundled SQLite already defaults to on; say so here rather than rely on how it was built.
    conn.execute_batch("pragma foreign_keys = on;")?;
    ran
}

/// Whether `table` already has `column`.
fn has_column(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "select exists (select 1 from pragma_table_info(?1) where name = ?2)",
        [table, column],
        |r| r.get(0),
    )
}

fn add_column(conn: &Connection, table: &str, column: &str, definition: &str) -> Result<(), Error> {
    if !has_column(conn, table, column)? {
        conn.execute(
            &format!("alter table {table} add column {column} {definition}"),
            [],
        )?;
    }
    Ok(())
}

/// Version 1: what servers before versioning did on every start. Databases from then have no
/// version and any mix of these changes, so every part checks before it acts.
fn baseline(conn: &Connection) -> Result<(), Error> {
    add_column(conn, "reviews", "ivl", "integer")?;
    // Players recorded before clocks had a history keep the only clock we
    // know about, reaching back over all of their reviews. A move they made
    // before this point has to be declared; it is not in the review data.
    conn.execute(
        "insert into clock_history (user, effective_from_ms, offset_west_min, rollover_hour)
         select user, 0, offset_west_min, rollover_hour from clocks
         where not exists (select 1 from clock_history where clock_history.user = clocks.user)",
        [],
    )?;
    for (column, definition) in [
        ("sender", "text not null default ''"),
        ("replied", "integer not null default 0"),
        ("kind", "text not null default 'message'"),
        ("push_only", "integer not null default 0"),
        ("retry_at", "integer not null default 0"),
        ("push_attempts", "integer not null default 0"),
        ("push_cancelled", "integer not null default 0"),
        ("completion_cancelled", "integer not null default 0"),
        ("read_at", "integer"),
        ("challenge_id", "integer"),
        ("reply_to_title", "text"),
        ("reply_to_body", "text"),
        ("topic", "text"),
        ("win_id", "integer"),
        ("quiet", "integer not null default 0"),
    ] {
        add_column(conn, "notifications", column, definition)?;
    }
    add_column(
        conn,
        "player_settings",
        "celebrations",
        "integer not null default 1",
    )?;
    if !has_column(conn, "player_settings", "friend_nudges")? {
        // Friend nudges start with the choice the player made for nudges.
        conn.execute_batch(
            "alter table player_settings add column friend_nudges integer not null default 0;
             update player_settings set friend_nudges = nudges;",
        )?;
    }
    conn.execute_batch(
        "create index if not exists notifications_pending on notifications (pushed, retry_at, id);
         create index if not exists notifications_sender on notifications (sender, id);",
    )?;
    // Older servers left nudges queued after opt-out. Retain their inbox history
    // without resuming those pushes when the player next enables nudges.
    conn.execute(
        &format!(
            "update notifications set push_cancelled = 1
             where pushed = 0 and push_cancelled = 0 and kind = 'nudge'
             and not {NUDGE_ALLOWED_SQL}"
        ),
        [],
    )?;
    add_column(conn, "goals", "since", "integer")?;
    // An old global warning may be waiting on a failed push when the upgraded
    // server starts. It must not bypass the new opt-ins or quiet hours.
    conn.execute(
        "update notifications set pushed = 1, push_only = 1 where kind = 'risk' and pushed = 0",
        [],
    )?;
    Ok(())
}

/// When the player's own inbox request first returned a notification; null until then.
fn notification_fetches(conn: &Connection) -> Result<(), Error> {
    add_column(conn, "notifications", "fetched_at", "integer")
}

/// Whether the player chose to appear on the global leaderboard; off until they turn it on.
/// [`global_leaderboard_choice`] later made it null until they choose.
fn global_leaderboard(conn: &Connection) -> Result<(), Error> {
    add_column(
        conn,
        "player_settings",
        "global_leaderboard",
        "integer not null default 0",
    )
}

/// The global leaderboard became opt-out: `global_leaderboard` is now null until the player
/// chooses. A stored 0 was the old default, not a choice anyone can be told apart by, so it
/// becomes null; a 1 was always turned on by its player and stays.
fn global_leaderboard_choice(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "alter table player_settings add column global_leaderboard_choice integer;
         update player_settings set global_leaderboard_choice = 1 where global_leaderboard = 1;
         alter table player_settings drop column global_leaderboard;
         alter table player_settings rename column global_leaderboard_choice to global_leaderboard;",
    )?;
    Ok(())
}

/// Device tokens moved over from the server configuration were listed as "Moved from the
/// server configuration", which means nothing to the player; they get the plainer name
/// `admin adopt` now gives them.
fn adopted_device_names(conn: &Connection) -> Result<(), Error> {
    conn.execute(
        "update account_tokens set device = 'Earlier sign-in'
         where device = 'Moved from the server configuration'",
        [],
    )?;
    Ok(())
}

/// The sender's own words with a moment shared by hand; null for every other row.
fn share_notes(conn: &Connection) -> Result<(), Error> {
    add_column(conn, "notifications", "note", "text")
}

/// The cow joined the companions.
fn cow_companion(conn: &Connection) -> Result<(), Error> {
    rebuild_companions(conn)
}

/// SQLite cannot change a check constraint in place, so the companion table is rebuilt with
/// [`crate::companions::TABLE`] and its rows copied over.
fn rebuild_companions(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(&format!(
        "alter table companion_preferences rename to companion_preferences_old;
         {}
         insert into companion_preferences (user, companion)
             select user, companion from companion_preferences_old;
         drop table companion_preferences_old;",
        crate::companions::TABLE
    ))?;
    Ok(())
}

/// Ankilope became an unlock earned at a level. Players who already chose it keep it, as do
/// players who unlocked the cow before it was earned by study. Neither gets an unlock moment
/// for it, unless they have never been announced anything: then the first run records history.
fn companion_milestones(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "insert or ignore into companion_unlocks (user, unlock, unlocked_at)
             select user, 'ankilope', cast(unixepoch('subsec') * 1000 as integer)
             from companion_preferences where companion = 'ankilope';
         insert or ignore into seen (user, key)
             select user, 'companion:' || unlock from companion_unlocks
             where exists (select 1 from seen where seen.user = companion_unlocks.user);",
    )?;
    Ok(())
}

/// Anki-tan, her chibi and Leech-tan can be stored, ready for when they go live.
fn earned_companions(conn: &Connection) -> Result<(), Error> {
    rebuild_companions(conn)
}

/// Preserve outstanding Undo values before new sends get durable, distinct receipts.
fn share_receipts(conn: &Connection) -> Result<(), Error> {
    add_column(conn, "win_deliveries", "send_until", "integer")?;
    conn.execute(
        "insert or ignore into win_receipts (win, until, created_at)
         select d.win, n.created_at + ?1, n.created_at from win_deliveries d
         join notifications n on n.id = d.notification join wins w on w.id = d.win
         where d.automatic = 0",
        [crate::conquests::UNDO_MS],
    )?;
    conn.execute(
        "update win_deliveries set send_until = (
             select n.created_at + ?1 from notifications n where n.id = notification
         ) where automatic = 0 and send_until is null",
        [crate::conquests::UNDO_MS],
    )?;
    Ok(())
}

/// Quest-chan and her chibi can be stored, and Leech-tan is now Leech-kun: rows that name him
/// by his old id `leech_tan` follow. He was planned, so no released client offered him.
fn quest_companions(conn: &Connection) -> Result<(), Error> {
    // Unlocks and announced moments first; a row already under the new id wins.
    conn.execute_batch(
        "update or ignore companion_unlocks set unlock = 'leech_kun' where unlock = 'leech_tan';
         delete from companion_unlocks where unlock = 'leech_tan';
         update or ignore seen set key = 'companion:leech_kun' where key = 'companion:leech_tan';
         delete from seen where key = 'companion:leech_tan';",
    )?;

    // The old check refuses `leech_kun`, so choices are renamed while copying into the new table.
    conn.execute_batch(&format!(
        "alter table companion_preferences rename to companion_preferences_old;
         {}
         insert into companion_preferences (user, companion)
             select user, case companion when 'leech_tan' then 'leech_kun' else companion end
             from companion_preferences_old;
         drop table companion_preferences_old;",
        crate::companions::TABLE
    ))?;
    Ok(())
}

/// Invitations to a group reach the invitee's inbox as rows that name the group.
fn group_invitations(conn: &Connection) -> Result<(), Error> {
    add_column(conn, "notifications", "group_id", "integer")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use crate::test_support::cleanup;

    fn version(conn: &Connection) -> usize {
        conn.query_row("pragma user_version", [], |r| r.get::<_, i64>(0))
            .unwrap() as usize
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ankiquest-migrations-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn count(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("select count(*) from {table}"), [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn steps_run_once_in_order_and_record_the_version() {
        fn first(conn: &Connection) -> Result<(), Error> {
            conn.execute_batch("create table log (step integer); insert into log values (1);")?;
            Ok(())
        }
        fn second(conn: &Connection) -> Result<(), Error> {
            conn.execute("insert into log values (2)", [])?;
            Ok(())
        }
        let conn = Connection::open_in_memory().unwrap();
        run_steps(&conn, &[first]).unwrap();
        assert_eq!(version(&conn), 1);
        run_steps(&conn, &[first, second]).unwrap();
        run_steps(&conn, &[first, second]).unwrap();
        assert_eq!(version(&conn), 2);
        let steps: Vec<i64> = conn
            .prepare("select step from log")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(steps, [1, 2]);
    }

    #[test]
    fn a_failed_step_leaves_no_trace_and_a_newer_database_is_refused() {
        fn broken(conn: &Connection) -> Result<(), Error> {
            conn.execute_batch("create table half (x integer); select * from missing;")?;
            Ok(())
        }
        let conn = Connection::open_in_memory().unwrap();
        assert!(run_steps(&conn, &[broken]).is_err());
        assert_eq!(version(&conn), 0);
        assert!(
            !conn
                .prepare("select 1 from sqlite_master where name = 'half'")
                .unwrap()
                .exists([])
                .unwrap()
        );
        conn.pragma_update(None, "user_version", 5).unwrap();
        assert!(run_steps(&conn, &[broken]).is_err());
        assert_eq!(version(&conn), 5);
    }

    #[test]
    fn an_unversioned_database_takes_the_baseline_and_keeps_its_rows() {
        let dir = temp_dir("baseline");
        drop(Store::open(&dir).unwrap());
        {
            // A database from before versioning, which already had every column.
            let conn = Connection::open(dir.join("ankiquest.db")).unwrap();
            conn.execute_batch(
                "pragma user_version = 0;
                 insert into goals (user, mode, value, since) values ('hill', 'reviews', 50, 7);
                 insert into player_settings (user, nudges, friend_nudges) values ('hill', 1, 0);",
            )
            .unwrap();
        }
        let store = Store::open(&dir).unwrap();
        assert_eq!(version(&store.conn), STEPS.len());
        let (since, friend_nudges): (i64, i64) = store
            .conn
            .query_row(
                "select since, friend_nudges from goals, player_settings",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((since, friend_nudges), (7, 0));
        cleanup(store, dir);
    }

    #[test]
    fn the_global_leaderboard_keeps_only_choices_anyone_could_have_made() {
        let dir = temp_dir("global-choice");
        drop(Store::open(&dir).unwrap());
        {
            // A version 3 database: every row got 0 unless its player turned the board on.
            let conn = Connection::open(dir.join("ankiquest.db")).unwrap();
            conn.execute_batch(
                "alter table player_settings drop column global_leaderboard;
                 alter table player_settings add column global_leaderboard integer not null default 0;
                 insert into player_settings (user, nudges) values ('hill', 1);
                 insert into player_settings (user, global_leaderboard) values ('cerro', 1), ('ana', 0), ('bo', 0);
                 insert into visibility (user, public) values ('bo', 0), ('cerro', 0);
                 pragma user_version = 3;",
            )
            .unwrap();
        }
        let mut store = Store::open(&dir).unwrap();
        assert_eq!(version(&store.conn), STEPS.len());
        let stored: Vec<(String, Option<i64>, i64)> = store
            .conn
            .prepare("select user, global_leaderboard, nudges from player_settings order by user")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            stored,
            [
                ("ana".into(), None, 0),
                ("bo".into(), None, 0),
                ("cerro".into(), Some(1), 0),
                ("hill".into(), None, 1),
            ]
        );
        let listed = |store: &Store, user: &str| store.global_leaderboard(user).unwrap();
        assert!(listed(&store, "ana") && listed(&store, "hill") && listed(&store, "nobody"));
        assert!(
            listed(&store, "cerro"),
            "turned on, though the profile is private"
        );
        assert!(
            !listed(&store, "bo"),
            "a private profile is not listed by default"
        );
        store.set_global_leaderboard("ana", false).unwrap();
        store.set_global_leaderboard("bo", true).unwrap();
        assert!(!listed(&store, "ana"), "an explicit opt-out is kept");
        assert!(listed(&store, "bo"));
        cleanup(store, dir);
    }

    #[test]
    fn a_step_that_leaves_a_dangling_row_is_refused() {
        fn dangling(conn: &Connection) -> Result<(), Error> {
            conn.execute_batch(
                "create table parent (id integer primary key);
                 create table child (parent integer references parent (id));
                 insert into child values (1);",
            )?;
            Ok(())
        }
        let conn = Connection::open_in_memory().unwrap();
        assert!(run_steps(&conn, &[dangling]).is_err());
        assert_eq!(version(&conn), 0);
    }

    #[test]
    fn foreign_keys_are_on_so_deleting_a_parent_cascades() {
        let dir = temp_dir("cascades");
        let store = Store::open(&dir).unwrap();
        let conn = &store.conn;
        let on: bool = conn
            .query_row("pragma foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert!(on);
        conn.execute_batch(
            "insert into accounts values ('hill', 'hill', '', 0);
             insert into account_tokens values ('token', 'hill', 'Phone', 0);
             insert into groups values (1, 'Study', 'invite', 'hill', 0);
             insert into group_members values (1, 'hill', 1, 0);",
        )
        .unwrap();
        assert!(
            conn.execute(
                "insert into account_tokens values ('other', 'gone', 'Phone', 0)",
                []
            )
            .is_err()
        );
        assert!(
            conn.execute("insert into group_members values (2, 'hill', 1, 0)", [])
                .is_err()
        );
        assert!(
            conn.execute(
                "insert into community_members (challenge, user, status) values (9, 'hill', 'invited')",
                []
            )
            .is_err()
        );
        conn.execute("delete from groups where id = 1", []).unwrap();
        assert_eq!(count(conn, "group_members"), 0);
        conn.execute("delete from accounts where user = 'hill'", [])
            .unwrap();
        assert_eq!(count(conn, "account_tokens"), 0);
        cleanup(store, dir);
    }

    #[test]
    fn old_sends_keep_their_receipts() {
        let dir = temp_dir("share-receipts");
        let store = Store::open(&dir).unwrap();
        let conn = &store.conn;
        let win = crate::wins::keep(
            conn,
            "hill",
            "level:7",
            1,
            3_000,
            "Level",
            "Body",
            &serde_json::json!({}),
        )
        .unwrap()
        .unwrap();
        for (recipient, automatic) in [("cerro", false), ("friend", true)] {
            let notice = crate::notifications::insert_notification(
                conn,
                &crate::notifications::Outgoing {
                    to: recipient,
                    from: "hill",
                    title: "Level",
                    body: "Body",
                    kind: "share",
                },
                1,
                3_000,
                false,
            )
            .unwrap();
            conn.execute("insert into win_deliveries (win, recipient, notification, day, automatic) values (?1, ?2, ?3, 1, ?4)", rusqlite::params![win, recipient, notice, automatic]).unwrap();
        }
        // Reproduce the released schema, then reopen through the real upgrade path.
        conn.execute_batch("alter table win_deliveries drop column send_until; drop table win_receipts; pragma user_version = 8;").unwrap();
        drop(store);
        let store = Store::open(&dir).unwrap();
        let expected = 3_000 + crate::conquests::UNDO_MS;
        let until: i64 = store
            .conn
            .query_row(
                "select send_until from win_deliveries where recipient = 'cerro'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(until, expected);
        let automatic: Option<i64> = store
            .conn
            .query_row(
                "select send_until from win_deliveries where recipient = 'friend'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(automatic, None);
        assert_eq!(count(&store.conn, "win_receipts"), 1);
        assert_eq!(version(&store.conn), STEPS.len());

        // Undo may remove every delivery, but reopening must retain its reservation.
        store
            .conn
            .execute("delete from win_deliveries", [])
            .unwrap();
        drop(store);
        let store = Store::open(&dir).unwrap();
        let receipt: (i64, i64, i64) = store
            .conn
            .query_row("select win, until, created_at from win_receipts", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(receipt, (win, expected, 3_000));
        store
            .conn
            .execute("delete from wins where id = ?1", [win])
            .unwrap();
        assert_eq!(count(&store.conn, "win_receipts"), 0);
        cleanup(store, dir);
    }

    #[test]
    fn a_desktop_built_database_still_gets_share_receipts() {
        let dir = temp_dir("desktop-version-9");
        drop(Store::open(&dir).unwrap());
        {
            // A version 9 database from the server built from AnkiQuest-Desktop: its step 9 was
            // `earned_companions`, so the companion table is current but Undo receipts are not.
            let conn = Connection::open(dir.join("ankiquest.db")).unwrap();
            conn.execute_batch(
                "alter table win_deliveries drop column send_until;
                 drop table win_receipts;
                 pragma user_version = 9;",
            )
            .unwrap();
        }
        let store = Store::open(&dir).unwrap();
        assert_eq!(version(&store.conn), STEPS.len());
        assert!(has_column(&store.conn, "win_deliveries", "send_until").unwrap());
        cleanup(store, dir);
    }

    #[test]
    fn the_cow_can_be_stored_after_the_companion_table_is_rebuilt() {
        let dir = temp_dir("cow-companion");
        drop(Store::open(&dir).unwrap());
        {
            // A version 6 database, whose check knows only the first three companions.
            let conn = Connection::open(dir.join("ankiquest.db")).unwrap();
            conn.execute_batch(
                "drop table companion_preferences;
                 create table companion_preferences (
                     user text primary key,
                     companion text not null check (companion in ('aki', 'ankilope', 'none'))
                 ) without rowid;
                 insert into companion_preferences values ('hill', 'ankilope'), ('ana', 'none');
                 pragma user_version = 6;",
            )
            .unwrap();
            assert!(
                conn.execute("insert into companion_preferences values ('bo', 'cow')", [])
                    .is_err()
            );
        }
        let store = Store::open(&dir).unwrap();
        assert_eq!(version(&store.conn), STEPS.len());
        assert_eq!(store.companion("hill").unwrap(), "ankilope");
        assert_eq!(store.companion("ana").unwrap(), "none");
        store.set_companion("bo", "cow").unwrap();
        assert_eq!(store.companion("bo").unwrap(), "cow");
        assert!(store.set_companion("bo", "horse").is_err());
        cleanup(store, dir);
    }

    #[test]
    fn the_earned_companions_can_be_stored_after_the_table_is_rebuilt() {
        let dir = temp_dir("earned-companions");
        drop(Store::open(&dir).unwrap());
        {
            // A version 8 database, whose check knows only the first four companions.
            let conn = Connection::open(dir.join("ankiquest.db")).unwrap();
            conn.execute_batch(
                "drop table companion_preferences;
                 create table companion_preferences (
                     user text primary key,
                     companion text not null check (companion in ('aki', 'ankilope', 'cow', 'none'))
                 ) without rowid;
                 insert into companion_preferences values ('hill', 'cow'), ('ana', 'none');
                 pragma user_version = 8;",
            )
            .unwrap();
        }
        let store = Store::open(&dir).unwrap();
        assert_eq!(version(&store.conn), STEPS.len());
        assert_eq!(store.companion("hill").unwrap(), "cow");
        assert_eq!(store.companion("ana").unwrap(), "none");
        for companion in ["anki_tan", "anki_tan_chibi", "leech_kun"] {
            store.set_companion("bo", companion).unwrap();
            assert_eq!(store.companion("bo").unwrap(), companion);
        }
        assert!(store.set_companion("bo", "horse").is_err());
        cleanup(store, dir);
    }

    #[test]
    fn leech_tan_becomes_leech_kun_and_quest_chan_can_be_stored() {
        let dir = temp_dir("quest-companions");
        drop(Store::open(&dir).unwrap());
        {
            // A version 10 database that knows Leech-kun by his old id.
            let conn = Connection::open(dir.join("ankiquest.db")).unwrap();
            conn.execute_batch(
                "drop table companion_preferences;
                 create table companion_preferences (
                     user text primary key,
                     companion text not null check (companion in
                         ('aki', 'ankilope', 'cow', 'none', 'anki_tan', 'anki_tan_chibi', 'leech_tan'))
                 ) without rowid;
                 insert into companion_preferences values ('hill', 'leech_tan'), ('ana', 'cow');
                 insert into companion_unlocks values ('hill', 'leech_tan', 5), ('ana', 'cow', 6);
                 insert into seen (user, key) values ('hill', 'companion:leech_tan');
                 pragma user_version = 10;",
            )
            .unwrap();
        }
        let store = Store::open(&dir).unwrap();
        assert_eq!(version(&store.conn), STEPS.len());

        // Stored ids, choices and announced moments follow the rename.
        use crate::companions::Unlock;
        assert_eq!(store.companion("hill").unwrap(), "leech_kun");
        assert_eq!(store.companion("ana").unwrap(), "cow");
        assert_eq!(store.unlocks("hill").unwrap(), [Unlock::LeechKun]);
        assert_eq!(store.unlocks("ana").unwrap(), [Unlock::Cow]);
        assert!(store.is_seen("hill", "companion:leech_kun").unwrap());
        assert!(!store.is_seen("hill", "companion:leech_tan").unwrap());

        // Quest-chan and her chibi can be stored; the old id cannot.
        for companion in ["quest_chan", "quest_chan_chibi"] {
            store.set_companion("bo", companion).unwrap();
            assert_eq!(store.companion("bo").unwrap(), companion);
        }
        assert!(store.set_companion("bo", "leech_tan").is_err());
        cleanup(store, dir);
    }

    #[test]
    fn players_keep_ankilope_and_the_cow_without_a_moment() {
        let dir = temp_dir("companion-milestones");
        drop(Store::open(&dir).unwrap());
        {
            // A version 7 database: hill chose Ankilope, bo unlocked the cow by tapping, ana
            // chose Ankilope but has never been announced anything.
            let conn = Connection::open(dir.join("ankiquest.db")).unwrap();
            conn.execute_batch(
                "insert into companion_preferences values
                     ('hill', 'ankilope'), ('ana', 'ankilope'), ('cy', 'aki'), ('bo', 'cow');
                 insert into companion_unlocks values ('bo', 'cow', 5);
                 insert into seen (user, key) values ('hill', 'level:2'), ('bo', 'level:2');
                 pragma user_version = 7;",
            )
            .unwrap();
        }
        let store = Store::open(&dir).unwrap();
        assert_eq!(version(&store.conn), STEPS.len());
        use crate::companions::Unlock;
        assert_eq!(store.unlocks("hill").unwrap(), [Unlock::Ankilope]);
        assert_eq!(store.unlocks("ana").unwrap(), [Unlock::Ankilope]);
        assert_eq!(store.unlocks("bo").unwrap(), [Unlock::Cow]);
        assert!(store.unlocks("cy").unwrap().is_empty());
        assert!(store.is_seen("hill", "companion:ankilope").unwrap());
        assert!(store.is_seen("bo", "companion:cow").unwrap());
        assert!(
            !store.is_seen("ana", "companion:ankilope").unwrap(),
            "a first run records it with the rest of the history"
        );
        cleanup(store, dir);
    }

    #[test]
    fn adopted_devices_get_a_plain_name() {
        let dir = temp_dir("adopted-names");
        drop(Store::open(&dir).unwrap());
        {
            let conn = Connection::open(dir.join("ankiquest.db")).unwrap();
            conn.execute_batch(
                "insert into accounts (user, display, password_hash, created_at) values ('ana', 'Ana', '', 1);
                 insert into account_tokens (token_hash, user, device, created_at) values
                     ('a', 'ana', 'Moved from the server configuration', 1),
                     ('b', 'ana', 'Pixel', 2);
                 pragma user_version = 4;",
            )
            .unwrap();
        }
        let store = Store::open(&dir).unwrap();
        assert_eq!(version(&store.conn), STEPS.len());
        let names: Vec<String> = store
            .conn
            .prepare("select device from account_tokens order by token_hash")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(names, [crate::accounts::ADOPTED_DEVICE, "Pixel"]);
        cleanup(store, dir);
    }
}
