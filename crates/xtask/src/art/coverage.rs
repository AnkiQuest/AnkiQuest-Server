//! Reports which companion art exists: every item of
//! `tests/fixtures/companion_poses.json` (core moods, pictures, icons and
//! extensions) for each companion. Read-only.
//!
//! An item's master is the file `<folder>/poses.json` lists for it, else
//! `<folder>/<item>.png` or the older file name [`ankiquest_art::stem`]
//! gives, below [`ankiquest_art::MASTERS`]. A missing item shows the first
//! item its fallback chain reaches that exists. An archive folder is never
//! read for masters; `--json` only lists it.
//!
//! - no flag: a Markdown table, one row per item, one column per companion.
//! - `--missing`: per companion, the core items and extensions without a
//!   master: the art backlog (AGENTS.md, Companion art).
//! - `--since <fixture>`: fails when an item the given older copy of the
//!   fixture lacks has no master for some live companion, so a PR that adds
//!   an item draws it for all of them. Gaps the list already had do not fail
//!   (hill, 2026-11-21: companions stay live while their core is drawn).
//! - `--json`: the matrix for the gallery (AnkiQuest.github.io), described
//!   in `docs/branding/art-coverage.md`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ankiquest_art::{ARCHIVE, Kind, MASTERS, Poses};
use serde_json::{Value, json};

use super::root;

/// The pose list both clients use.
const FIXTURE: &str = ankiquest_art::POSES;

/// How players earn each companion but the starter, and which are live.
const UNLOCKS: &str = "tests/fixtures/companion_unlock.json";

/// The companion every player starts with; it is not in [`UNLOCKS`].
const STARTER: &str = "aki";

/// The `status` [`UNLOCKS`] gives a companion players can have.
const LIVE: &str = "live";

/// Version of the `--json` output; bumped when a field changes meaning or
/// goes away.
const SCHEMA: u32 = 1;

/// Image formats [`archive`] lists.
const IMAGES: [&str; 3] = ["png", "webp", "gif"];

/// Every companion and its master folder under `static/`, in picker order.
const COMPANIONS: &[(&str, &str)] = &[
    ("aki", "aki"),
    ("ankilope", "ankilope"),
    ("cow", "cow"),
    ("anki_tan", "anki-tan"),
    ("anki_tan_chibi", "anki-tan/chibi"),
    ("quest_chan", "quest-chan"),
    ("quest_chan_chibi", "quest-chan/chibi"),
    ("leech_kun", "leech-kun"),
];

/// The names players and hill use for each companion id.
const NAMES: &[(&str, &str)] = &[
    ("aki", "Aki"),
    ("ankilope", "Ankilope"),
    ("cow", "Bluma"),
    ("anki_tan", "Anki-tan"),
    ("anki_tan_chibi", "Anki-tan chibi"),
    ("quest_chan", "Quest-chan"),
    ("quest_chan_chibi", "Quest-chan chibi"),
    ("leech_kun", "Leech-kun"),
];

/// The licence of the pictures in a folder under `static/`.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Licence {
    /// The repository's licence.
    Repository,
    /// Shigeyuki's Anki-tan and every derivative of her.
    AnkiTan,
}

impl Licence {
    /// The key `--json` uses for this licence.
    fn key(self) -> &'static str {
        match self {
            Licence::Repository => "repository",
            Licence::AnkiTan => "anki-tan",
        }
    }

    /// The licence's SPDX id, licence file and required credit.
    fn describe(self) -> Value {
        match self {
            Licence::Repository => json!({
                "spdx": "AGPL-3.0-or-later",
                "file": "LICENSE",
                "credit": null,
            }),
            Licence::AnkiTan => json!({
                "spdx": "CC-BY-SA-4.0",
                "file": "static/anki-tan/LICENSE",
                "credit": "Anki-tan by Shigeyuki (https://www.patreon.com/Shigeyuki)",
            }),
        }
    }
}

/// Folders below [`MASTERS`] whose pictures are not under the repository's
/// licence, with all their subfolders.
const LICENCES: &[(&str, Licence)] = &[("anki-tan", Licence::AnkiTan)];

/// The licence of the pictures in `folder`, relative to `static/`.
fn licence(folder: &str) -> Licence {
    LICENCES
        .iter()
        .find(|(prefix, _)| folder == *prefix || folder.starts_with(&format!("{prefix}/")))
        .map_or(Licence::Repository, |(_, licence)| *licence)
}

/// One item of one companion.
#[derive(Debug, PartialEq)]
enum Status {
    /// The master, relative to the repository root.
    Present(String),
    /// Missing; the clients show this item's master instead.
    Fallback {
        shows: String,
        file: String,
    },
    Missing,
}

/// What the command prints or checks.
enum Mode {
    /// The full table.
    Table,
    /// The items each companion has no master for.
    Missing,
    /// Fails when an item this older fixture lacks is undrawn.
    Since(PathBuf),
    /// The matrix as JSON.
    Json,
}

/// Whether players can have a companion yet.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage {
    Live,
    Planned,
}

impl Stage {
    fn name(self) -> &'static str {
        match self {
            Stage::Live => "live",
            Stage::Planned => "planned",
        }
    }
}

/// A companion, its master folder under `static/` and its stage.
struct Companion {
    id: &'static str,
    folder: &'static str,
    stage: Stage,
}

/// The command's arguments.
const USAGE: &str =
    "usage: xtask art-coverage [--missing | --json | --since <companion_poses.json>]";

/// Runs the report or the check.
pub fn run(args: &[String]) -> ExitCode {
    let Some(mode) = mode(args) else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    let root = root();
    match execute(&root, mode) {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// The mode the arguments ask for.
fn mode(args: &[String]) -> Option<Mode> {
    match args {
        [] => Some(Mode::Table),
        [flag] if flag == "--missing" => Some(Mode::Missing),
        [flag] if flag == "--json" => Some(Mode::Json),
        [flag, base] if flag == "--since" => Some(Mode::Since(PathBuf::from(base))),
        _ => None,
    }
}

/// The output for `mode`, or why the check failed.
fn execute(root: &Path, mode: Mode) -> Result<String, String> {
    let list = poses(&root.join(FIXTURE))?;
    let companions = companions(root)?;
    match mode {
        Mode::Table => Ok(report(root, &companions, &list)),
        Mode::Missing => Ok(backlog(root, &companions, &list)),
        Mode::Json => {
            let value = matrix(root, &companions, &list)?;
            Ok(serde_json::to_string_pretty(&value).map_err(|e| e.to_string())? + "\n")
        }
        Mode::Since(base) => {
            let base = poses(&base)?;
            let undrawn = undrawn(root, &companions, &base, &list);
            if undrawn.is_empty() {
                return Ok("Every added item has a master for every live companion.\n".into());
            }
            Err(format!(
                "Added items need a master for every live companion \
                 (AGENTS.md, Companion art):\n{}",
                undrawn.join("\n")
            ))
        }
    }
}

/// Reads a pose fixture.
fn poses(path: &Path) -> Result<Poses, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Poses::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The companions [`COMPANIONS`] lists, with their stage: the starter
/// is live, the others as [`UNLOCKS`] says. One it does not list is left
/// out.
fn companions(root: &Path) -> Result<Vec<Companion>, String> {
    let path = root.join(UNLOCKS);
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let json: Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let unlocks = json["companions"]
        .as_array()
        .ok_or("companions is not a list")?;

    // The starter, then each companion as the unlock fixture lists it.
    let stage = |id: &str| {
        if id == STARTER {
            return Some(Stage::Live);
        }
        let entry = unlocks.iter().find(|entry| entry["companion"] == id)?;
        Some(if entry["status"] == LIVE {
            Stage::Live
        } else {
            Stage::Planned
        })
    };
    Ok(COMPANIONS
        .iter()
        .filter_map(|&(id, folder)| {
            let stage = stage(id)?;
            Some(Companion { id, folder, stage })
        })
        .collect())
}

/// The display name for a companion id.
fn name(id: &str) -> &str {
    NAMES
        .iter()
        .find(|(known, _)| *known == id)
        .map_or(id, |(_, name)| name)
}

/// A path relative to `root`, with `/` separators.
fn relative(root: &Path, path: &Path) -> String {
    let path = path.strip_prefix(root).unwrap_or(path);
    path.to_string_lossy().replace('\\', "/")
}

/// The companion's master folder.
fn folder(root: &Path, companion: &Companion) -> PathBuf {
    root.join(MASTERS).join(companion.folder)
}

/// The master for `item` in `folder`. A broken `poses.json` lists nothing
/// here; `pose_lists_name_masters` fails on it.
fn master(folder: &Path, item: &str) -> Option<PathBuf> {
    ankiquest_art::master(folder, item).ok().flatten()
}

/// The status of `item` in `folder`, following its fallbacks.
fn status(root: &Path, folder: &Path, poses: &Poses, item: &str) -> Status {
    for (step, shown) in poses.chain(item).into_iter().enumerate() {
        let Some(file) = master(folder, shown) else {
            continue;
        };
        let file = relative(root, &file);
        if step == 0 {
            return Status::Present(file);
        }
        return Status::Fallback {
            shows: shown.to_string(),
            file,
        };
    }
    Status::Missing
}

/// The items of the given kinds `folder` has no master for.
fn missing<'a>(folder: &Path, poses: &'a Poses, kinds: &[Kind]) -> Vec<&'a str> {
    kinds
        .iter()
        .flat_map(|kind| poses.of(*kind))
        .filter(|item| master(folder, item).is_none())
        .collect()
}

/// The kinds a companion needs to go live.
fn core_kinds() -> Vec<Kind> {
    Kind::ALL
        .into_iter()
        .filter(|kind| kind.is_core())
        .collect()
}

/// The art backlog: per companion, the core items and extensions without
/// a master.
fn backlog(root: &Path, companions: &[Companion], poses: &Poses) -> String {
    let mut out = String::from("Items without a master (AGENTS.md, Companion art)\n\n");
    let list = |items: &[&str]| {
        if items.is_empty() {
            "none".to_string()
        } else {
            items.join(", ")
        }
    };
    for companion in companions {
        let folder = folder(root, companion);
        let core = missing(&folder, poses, &core_kinds());
        let extensions = missing(&folder, poses, &[Kind::Extension]);
        let stage = match companion.stage {
            Stage::Live => "",
            Stage::Planned => " (planned)",
        };
        out += &format!(
            "- {} (`{}`){stage}: core {} missing: {}; extensions {} missing: {}\n",
            name(companion.id),
            companion.id,
            core.len(),
            list(&core),
            extensions.len(),
            list(&extensions),
        );
    }
    out
}

/// Each item in `poses` but not in `base` that a live companion has no
/// master for, as `item: companion`. A `base` from before the matrix has no
/// pictures or icons; only the lists both have are compared.
fn undrawn(root: &Path, companions: &[Companion], base: &Poses, poses: &Poses) -> Vec<String> {
    let compared = |item: &str| match poses.kind(item) {
        Some(Kind::Picture | Kind::Icon) => base.has_core_set(),
        _ => true,
    };
    let added = poses
        .names()
        .filter(|item| compared(item) && !base.names().any(|old| old == *item));
    let live: Vec<_> = companions
        .iter()
        .filter(|companion| companion.stage == Stage::Live)
        .collect();
    let mut undrawn = Vec::new();
    for item in added {
        for companion in &live {
            if master(&folder(root, companion), item).is_none() {
                undrawn.push(format!("{item}: {}", companion.id));
            }
        }
    }
    undrawn
}

/// The Markdown table: one row per item, one column per companion, then a
/// count per companion and kind.
fn report(root: &Path, companions: &[Companion], poses: &Poses) -> String {
    let mut out = String::from(
        "Masters. yes: master exists; (item): missing, the clients show that \
         item; **no**: missing. The core set is core_mood, picture and icon.\n\n",
    );
    out += "| item | kind |";
    for companion in companions {
        out += &format!(" {} |", companion.id);
    }
    out += "\n|---|---|";
    out += &"---|".repeat(companions.len());
    out += "\n";

    // Each item's row, counting present masters per companion and kind.
    let mut counts = vec![[0; Kind::ALL.len()]; companions.len()];
    for (index, kind) in Kind::ALL.into_iter().enumerate() {
        for item in poses.of(kind) {
            out += &format!("| {item} | {} |", kind.name());
            for (column, companion) in companions.iter().enumerate() {
                let status = status(root, &folder(root, companion), poses, item);
                if matches!(status, Status::Present(_)) {
                    counts[column][index] += 1;
                }
                out += &format!(" {} |", cell(&status));
            }
            out += "\n";
        }
    }

    // Totals: masters present per kind.
    out += "\n";
    for (companion, counts) in companions.iter().zip(counts) {
        let totals: Vec<String> = Kind::ALL
            .into_iter()
            .zip(counts)
            .map(|(kind, count)| format!("{} {count}/{}", kind.name(), poses.of(kind).len()))
            .collect();
        out += &format!("- {}: {}\n", companion.id, totals.join(", "));
    }
    out
}

/// A table cell for a status.
fn cell(status: &Status) -> String {
    match status {
        Status::Present(_) => "yes".into(),
        Status::Fallback { shows, .. } => format!("({shows})"),
        Status::Missing => "**no**".into(),
    }
}

/// One item of one companion, as JSON.
fn item_json(item: &str, kind: Kind, status: &Status) -> Value {
    match status {
        Status::Present(file) => json!({
            "name": item, "kind": kind.name(), "status": "present", "file": file,
        }),
        Status::Fallback { shows, file } => json!({
            "name": item, "kind": kind.name(), "status": "fallback", "shows": shows, "file": file,
        }),
        Status::Missing => json!({
            "name": item, "kind": kind.name(), "status": "missing",
        }),
    }
}

/// The PNGs directly in `folder` that are no item's master, such as a
/// picture only one companion has.
fn unlisted(root: &Path, folder: &Path, poses: &Poses) -> Vec<String> {
    let masters: Vec<PathBuf> = poses
        .names()
        .filter_map(|item| master(folder, item))
        .collect();
    let Ok(entries) = fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut files: Vec<String> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "png"))
        .filter(|path| !masters.contains(path))
        .map(|path| relative(root, &path))
        .collect();
    files.sort();
    files
}

/// One companion, as JSON.
fn companion_json(root: &Path, companion: &Companion, poses: &Poses) -> Value {
    let folder = folder(root, companion);
    let items: Vec<Value> = Kind::ALL
        .into_iter()
        .flat_map(|kind| poses.of(kind).into_iter().map(move |item| (kind, item)))
        .map(|(kind, item)| item_json(item, kind, &status(root, &folder, poses, item)))
        .collect();
    json!({
        "id": companion.id,
        "name": name(companion.id),
        "folder": relative(root, &folder),
        "stage": companion.stage.name(),
        "licence": licence(companion.folder).key(),
        "items": items,
        "missing": {
            "core": missing(&folder, poses, &core_kinds()),
            "extensions": missing(&folder, poses, &[Kind::Extension]),
        },
        "unlisted": unlisted(root, &folder, poses),
    })
}

/// Every image under `folder`, recursively, relative to `root`.
fn images(root: &Path, folder: &Path, found: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            images(root, &path, found);
            continue;
        }
        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if IMAGES.contains(&extension.to_ascii_lowercase().as_str()) {
            found.push(relative(root, &path));
        }
    }
}

/// Every archive folder below `folder`, recursively.
fn archives(folder: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if entry.file_name() == ARCHIVE {
            found.push(path);
            continue;
        }
        archives(&path, found);
    }
}

/// Each archive folder below [`MASTERS`], with its licence, README and
/// images: a companion's `archive/`, or each folder of a top-level
/// `archive/`. Listed for the gallery only; nothing here is a master.
fn archive(root: &Path) -> Vec<Value> {
    let masters = root.join(MASTERS);
    let mut found = Vec::new();
    archives(&masters, &mut found);

    // A top-level archive is split into its folders, one per companion or pack.
    let mut folders = Vec::new();
    for folder in found {
        if folder != masters.join(ARCHIVE) {
            folders.push(folder);
            continue;
        }
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        folders.extend(entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
    }
    folders.sort();
    folders
        .iter()
        .map(|folder| {
            let mut files = Vec::new();
            images(root, folder, &mut files);
            files.sort();
            let readme = folder.join("README.md");
            json!({
                "folder": relative(root, folder),
                "licence": licence(&relative(&masters, folder)).key(),
                "readme": readme.is_file().then(|| relative(root, &readme)),
                "files": files,
            })
        })
        .collect()
}

/// The whole matrix for the gallery.
fn matrix(root: &Path, companions: &[Companion], poses: &Poses) -> Result<Value, String> {
    let kinds: Vec<Value> = Kind::ALL
        .into_iter()
        .map(|kind| json!({"kind": kind.name(), "core": kind.is_core()}))
        .collect();
    let items: Vec<Value> = Kind::ALL
        .into_iter()
        .flat_map(|kind| poses.of(kind).into_iter().map(move |item| (kind, item)))
        .map(|(kind, item)| json!({"name": item, "kind": kind.name(), "fallback": poses.fallback(item)}))
        .collect();
    let licences: serde_json::Map<String, Value> = [Licence::Repository, Licence::AnkiTan]
        .into_iter()
        .map(|licence| (licence.key().to_string(), licence.describe()))
        .collect();
    Ok(json!({
        "schema": SCHEMA,
        "fixture": FIXTURE,
        "kinds": kinds,
        "items": items,
        "licences": licences,
        "companions": companions
            .iter()
            .map(|companion| companion_json(root, companion, poses))
            .collect::<Vec<_>>(),
        "archive": archive(root),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Poses {
        poses(&root().join(FIXTURE)).unwrap()
    }

    /// A list with the given core moods and extensions, and no pictures or
    /// icons unless `extra` adds them as fixture JSON keys.
    fn list(core: &[&str], extensions: &[(&str, &str)], extra: &str) -> Poses {
        let extensions: serde_json::Map<String, Value> = extensions
            .iter()
            .map(|(item, to)| (item.to_string(), json!(to)))
            .collect();
        let mut value = json!({"core": core, "extensions": extensions});
        if !extra.is_empty() {
            let extra: Value = serde_json::from_str(extra).unwrap();
            for (key, entries) in extra.as_object().unwrap() {
                value[key] = entries.clone();
            }
        }
        Poses::parse(&value.to_string()).unwrap()
    }

    /// A checkout in a temporary folder with the given masters, an unlock
    /// fixture making `cow` live and `anki_tan` planned.
    fn checkout(masters: &[&str]) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        for master in masters {
            let path = root.path().join("static").join(master);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"").unwrap();
        }
        let unlocks = r#"{"companions": [
            {"companion": "cow", "status": "live"},
            {"companion": "anki_tan", "status": "planned"}
        ]}"#;
        fs::create_dir_all(root.path().join("tests/fixtures")).unwrap();
        fs::write(root.path().join(UNLOCKS), unlocks).unwrap();
        root
    }

    #[test]
    fn statuses_follow_fallbacks() {
        let poses = list(
            &["streak", "frozen"],
            &[
                ("streak_hot", "streak"),
                ("streak_blazing", "streak_hot"),
                ("frozen_2", "frozen"),
            ],
            r#"{"pictures": {"freeze_used": "frozen"}, "icons": {"icon": "streak"}}"#,
        );
        let root = checkout(&["aki/streak.png"]);
        let folder = root.path().join("static/aki");

        // Present, fallback through two steps, and a chain with no master.
        let check = |item| status(root.path(), &folder, &poses, item);
        assert_eq!(
            Status::Present("static/aki/streak.png".into()),
            check("streak")
        );
        let streak = Status::Fallback {
            shows: "streak".into(),
            file: "static/aki/streak.png".into(),
        };
        assert_eq!(streak, check("streak_blazing"));
        assert_eq!(streak, check("icon"));
        assert_eq!(Status::Missing, check("frozen_2"));
        assert_eq!(Status::Missing, check("freeze_used"));
        assert_eq!(Status::Missing, check("frozen"));
    }

    #[test]
    fn legacy_file_names_count() {
        let poses = fixture();
        let root = checkout(&["aki/celebrate.png", "aki/leech.png"]);
        let folder = root.path().join("static/aki");
        let check = |item| status(root.path(), &folder, &poses, item);
        assert_eq!(
            Status::Present("static/aki/celebrate.png".into()),
            check("happy")
        );
        assert_eq!(
            Status::Present("static/aki/leech.png".into()),
            check("leech_defeated")
        );
    }

    #[test]
    fn lists_every_companion() {
        let path = root().join(UNLOCKS);
        let json: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let unlockable = json["companions"].as_array().unwrap();
        for companion in unlockable {
            let id = companion["companion"].as_str().unwrap();
            assert!(COMPANIONS.iter().any(|(known, _)| *known == id), "{id}");
        }
    }

    #[test]
    fn no_companion_reads_the_archive() {
        // An archive holds pictures that are not masters (AGENTS.md, Companion art).
        for (id, folder) in COMPANIONS {
            assert!(!ankiquest_art::archived(folder), "{id}");
        }

        // An archived picture named after a pose is no master.
        let poses = fixture();
        let root = checkout(&["archive/aki/face.png", "aki/archive/face.png"]);
        let cast = companions(root.path()).unwrap();
        let aki = cast.iter().find(|companion| companion.id == "aki").unwrap();
        let folder = folder(root.path(), aki);
        assert_eq!(
            Status::Missing,
            status(root.path(), &folder, &poses, "face")
        );
    }

    #[test]
    fn bluma_has_the_complete_core_moods() {
        let poses = fixture();
        let folder = root().join("static/cow");
        for pose in poses.of(Kind::CoreMood) {
            assert!(master(&folder, pose).is_some(), "{pose}");
        }
    }

    #[test]
    fn asleep_masters_use_the_pose_name() {
        // #32: no master keeps the older `sleeping` file name.
        for (companion, folder) in COMPANIONS {
            let folder = root().join("static").join(folder);
            assert!(!folder.join("sleeping.png").exists(), "{companion}");
        }
        assert_eq!(ankiquest_art::stem("asleep"), "asleep");
    }

    #[test]
    fn anki_tan_masters_cover_the_core_moods() {
        // The pack art and generated masters her folders' poses.json lists
        // cover both versions' core moods, and the full version's icons.
        let poses = fixture();
        for folder in ["static/anki-tan", "static/anki-tan/chibi"] {
            let folder = root().join(folder);
            for pose in poses.of(Kind::CoreMood) {
                assert!(
                    master(&folder, pose).is_some(),
                    "{}: {pose}",
                    folder.display()
                );
            }
        }
        let full = root().join("static/anki-tan");
        for icon in ["icon", "icon_happy", "icon_worried", "icon_sleepy"] {
            assert!(master(&full, icon).is_some(), "{icon}");
        }
    }

    #[test]
    fn listed_files_are_masters() {
        let poses = list(&["face", "happy"], &[("welcome", "happy")], "");
        let root = checkout(&["aki/emoji/smile.png", "aki/celebrate.png"]);
        let folder = root.path().join("static/aki");
        let json = r#"{"poses": {"face": "emoji/smile.png", "happy": "celebrate.png",
            "welcome": "emoji/wave.png"}}"#;
        fs::write(folder.join(ankiquest_art::LISTED), json).unwrap();

        // Listed and present, listed but absent.
        let check = |item| status(root.path(), &folder, &poses, item);
        assert_eq!(
            Status::Present("static/aki/emoji/smile.png".into()),
            check("face")
        );
        assert_eq!(
            Status::Fallback {
                shows: "happy".into(),
                file: "static/aki/celebrate.png".into()
            },
            check("welcome")
        );
    }

    #[test]
    fn pose_lists_name_masters() {
        let poses = fixture();
        for (companion, folder) in COMPANIONS {
            let folder = root().join(MASTERS).join(folder);
            let Some(files) = ankiquest_art::listed(&folder).unwrap() else {
                continue;
            };
            for (pose, file) in files {
                assert!(poses.kind(&pose).is_some(), "{companion}: {pose}");
                assert!(file.is_file(), "{companion}: {}", file.display());
                let path = relative(&root(), &file);
                assert!(!ankiquest_art::archived(&path), "{companion}: {path}");
            }
        }
    }

    #[test]
    fn generated_sizes_are_not_committed() {
        // build.rs makes face-192 and face-32 from face.png (AGENTS.md, Companion art).
        for (companion, folder) in COMPANIONS {
            for size in ["face-192.png", "face-32.png"] {
                let file = root().join("static").join(folder).join(size);
                assert!(!file.exists(), "{companion}: {size}");
            }
        }
    }

    #[test]
    fn reads_the_stage_of_each_companion() {
        let root = checkout(&[]);
        let companions = companions(root.path()).unwrap();
        let stages: Vec<_> = companions.iter().map(|c| (c.id, c.stage)).collect();
        assert_eq!(
            stages,
            [
                ("aki", Stage::Live),
                ("cow", Stage::Live),
                ("anki_tan", Stage::Planned)
            ]
        );
    }

    #[test]
    fn added_item_needs_every_live_master() {
        let base = list(&["happy"], &[], r#"{"icons": {"icon": "happy"}}"#);
        let poses = list(
            &["happy"],
            &[("winner", "happy")],
            r#"{"icons": {"icon": "happy"}, "pictures": {"goal": "happy"}}"#,
        );

        // Only Aki has them: Bluma's are missing, Anki-tan is planned.
        let root = checkout(&["aki/winner.png", "aki/goal.png", "anki-tan/happy.png"]);
        let cast = companions(root.path()).unwrap();
        let missing = undrawn(root.path(), &cast, &base, &poses);
        assert_eq!(missing, ["goal: cow", "winner: cow"]);

        // Every live companion has them.
        let root = checkout(&[
            "aki/winner.png",
            "aki/goal.png",
            "cow/winner.png",
            "cow/goal.png",
        ]);
        let cast = companions(root.path()).unwrap();
        assert!(undrawn(root.path(), &cast, &base, &poses).is_empty());
    }

    #[test]
    fn existing_gaps_do_not_fail() {
        // Neither companion has `happy` or `goal`, but master's list already had them.
        let poses = list(
            &["happy"],
            &[("winner", "happy")],
            r#"{"pictures": {"goal": "happy"}}"#,
        );
        let root = checkout(&[]);
        let companions = companions(root.path()).unwrap();
        assert!(undrawn(root.path(), &companions, &poses, &poses).is_empty());
    }

    #[test]
    fn listing_the_core_set_is_not_an_addition() {
        // hill, 2026-11-21: listing the pictures and icons the apps already
        // show does not ask for them to be drawn at once; companions stay
        // live meanwhile. A new core mood still needs every master.
        let base = list(&["happy"], &[], "");
        let poses = list(
            &["happy", "sad"],
            &[],
            r#"{"pictures": {"goal": "happy"}, "icons": {"icon": "happy"}}"#,
        );
        let root = checkout(&["aki/sad.png"]);
        let cast = companions(root.path()).unwrap();
        assert_eq!(undrawn(root.path(), &cast, &base, &poses), ["sad: cow"]);
    }

    #[test]
    fn backlog_lists_missing_items() {
        let poses = list(
            &["happy", "sad"],
            &[("winner", "happy")],
            r#"{"pictures": {"goal": "happy"}}"#,
        );
        let root = checkout(&[
            "aki/celebrate.png",
            "aki/sad.png",
            "aki/winner.png",
            "aki/goal.png",
            "cow/sad.png",
        ]);
        let companions = companions(root.path()).unwrap();
        assert_eq!(
            backlog(root.path(), &companions, &poses),
            "Items without a master (AGENTS.md, Companion art)\n\n\
             - Aki (`aki`): core 0 missing: none; extensions 0 missing: none\n\
             - Bluma (`cow`): core 2 missing: happy, goal; extensions 1 missing: winner\n\
             - Anki-tan (`anki_tan`) (planned): core 3 missing: happy, sad, goal; \
             extensions 1 missing: winner\n"
        );
    }

    #[test]
    fn parses_the_modes() {
        let args = |list: &[&str]| list.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        assert!(matches!(mode(&args(&[])), Some(Mode::Table)));
        assert!(matches!(mode(&args(&["--missing"])), Some(Mode::Missing)));
        assert!(matches!(mode(&args(&["--json"])), Some(Mode::Json)));
        assert!(matches!(
            mode(&args(&["--since", "base.json"])),
            Some(Mode::Since(path)) if path == Path::new("base.json")
        ));
        assert!(mode(&args(&["--since"])).is_none());
        assert!(mode(&args(&["--other"])).is_none());
    }

    #[test]
    fn names_every_companion() {
        for (id, _) in COMPANIONS {
            assert!(NAMES.iter().any(|(known, _)| known == id), "{id}");
        }
    }

    #[test]
    fn every_licence_file_has_its_licence() {
        // A folder below the masters with a LICENSE file is under that
        // licence; LICENCES names each one.
        let mut found = Vec::new();
        images_and_licences(&root().join(MASTERS), &mut found);
        assert!(!found.is_empty());
        for folder in found {
            let under = relative(&root().join(MASTERS), &folder);
            assert_eq!(licence(&under), Licence::AnkiTan, "{under}");
        }
        assert_eq!(licence("anki-tan/chibi"), Licence::AnkiTan);
        assert_eq!(licence("anki-tan/archive"), Licence::AnkiTan);
        assert_eq!(licence("cow/archive"), Licence::Repository);
        assert_eq!(licence("anki-tanx"), Licence::Repository);
    }

    /// Every folder under `folder` that holds a LICENSE file.
    fn images_and_licences(folder: &Path, found: &mut Vec<PathBuf>) {
        if folder.join("LICENSE").is_file() {
            found.push(folder.to_path_buf());
        }
        for entry in fs::read_dir(folder).unwrap().flatten() {
            if entry.path().is_dir() {
                images_and_licences(&entry.path(), found);
            }
        }
    }

    #[test]
    fn json_describes_the_matrix() {
        let poses = list(
            &["happy", "sad"],
            &[("winner", "happy")],
            r#"{"pictures": {"goal": "happy"}, "icons": {"icon": "sad"}}"#,
        );
        let root = checkout(&[
            "aki/celebrate.png",
            "aki/extra.png",
            "cow/sad.png",
            "cow/archive/old.png",
            "archive/aki/old.png",
            "anki-tan/archive/pack/smile.png",
        ]);
        fs::write(root.path().join("static/cow/archive/README.md"), "").unwrap();
        let cast = companions(root.path()).unwrap();
        let value = matrix(root.path(), &cast, &poses).unwrap();

        // Items in kind order, each with its fallback.
        assert_eq!(value["schema"], SCHEMA);
        let items: Vec<_> = value["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| {
                (
                    item["name"].as_str().unwrap(),
                    item["kind"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            items,
            [
                ("happy", "core_mood"),
                ("sad", "core_mood"),
                ("goal", "picture"),
                ("icon", "icon"),
                ("winner", "extension")
            ]
        );
        assert_eq!(value["items"][0]["fallback"], Value::Null);
        assert_eq!(value["items"][2]["fallback"], "happy");

        // Aki: present, fallback, missing, and a picture no item names.
        let aki = &value["companions"][0];
        assert_eq!(aki["stage"], "live");
        assert_eq!(aki["licence"], "repository");
        assert_eq!(aki["items"][0]["status"], "present");
        assert_eq!(aki["items"][0]["file"], "static/aki/celebrate.png");
        assert_eq!(aki["items"][2]["status"], "fallback");
        assert_eq!(aki["items"][2]["shows"], "happy");
        assert_eq!(aki["items"][1]["status"], "missing");
        assert_eq!(aki["missing"]["core"], json!(["sad", "goal", "icon"]));
        assert_eq!(aki["missing"]["extensions"], json!(["winner"]));
        assert_eq!(aki["unlisted"], json!(["static/aki/extra.png"]));
        assert_eq!(value["companions"][2]["licence"], "anki-tan");

        // The archive, apart, with each folder's licence.
        assert_eq!(
            value["archive"],
            json!([
                {"folder": "static/anki-tan/archive", "licence": "anki-tan",
                 "readme": null, "files": ["static/anki-tan/archive/pack/smile.png"]},
                {"folder": "static/archive/aki", "licence": "repository",
                 "readme": null, "files": ["static/archive/aki/old.png"]},
                {"folder": "static/cow/archive", "licence": "repository",
                 "readme": "static/cow/archive/README.md", "files": ["static/cow/archive/old.png"]}
            ])
        );
        assert_eq!(value["licences"]["anki-tan"]["spdx"], "CC-BY-SA-4.0");
    }

    #[test]
    fn the_repository_matrix_builds() {
        let poses = fixture();
        let cast = companions(&root()).unwrap();
        let value = matrix(&root(), &cast, &poses).unwrap();
        let archive = value["archive"].as_array().unwrap();
        assert!(
            archive
                .iter()
                .any(|folder| folder["folder"] == "static/cow/archive")
        );
        for companion in value["companions"].as_array().unwrap() {
            for item in companion["items"].as_array().unwrap() {
                let file = item["file"].as_str().unwrap_or("");
                assert!(!ankiquest_art::archived(file), "{file}");
            }
        }
    }
}
