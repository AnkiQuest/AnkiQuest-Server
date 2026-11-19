//! Prints which companion art exists: every pose in
//! `tests/fixtures/companion_poses.json` and every other per-companion
//! picture the clients show, for each companion, as a Markdown table.
//!
//! A pose's master is the file `static/<folder>/poses.json` lists for it
//! (Anki-tan's pack art), else `static/<folder>/<pose>.png`, or the older
//! name [`RENAMES`] gives when that file is absent. A missing extension shows
//! the first pose its fallback chain reaches that exists. Anki-tan's chibi
//! masters stay below the same character's licensed folder. Runtime
//! mappings are not inferred. Read-only.
//!
//! `--missing` lists, per companion, the poses without a master: the art
//! backlog (AGENTS.md, Companion art). `--since <fixture>` fails when a
//! pose the given older copy of the fixture lacks has no master for some
//! live companion, so a PR that adds a pose draws it for all of them.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ankiquest_art::listed;
use serde_json::Value;

use super::root;

/// The pose list both clients use.
const FIXTURE: &str = "tests/fixtures/companion_poses.json";

/// How players earn each companion but the starter, and which are live.
const UNLOCKS: &str = "tests/fixtures/companion_unlock.json";

/// The companion every player starts with; it is not in [`UNLOCKS`].
const STARTER: &str = "aki";

/// The `status` [`UNLOCKS`] gives a companion players can have.
const LIVE: &str = "live";

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

/// Poses whose masters may still have the file name they had before the
/// fixture; the pose's own name is checked first.
const RENAMES: &[(&str, &str)] = &[
    ("happy", "celebrate"),
    ("frozen", "freeze"),
    ("leech_defeated", "leech"),
];

/// Per-companion pictures that are not poses: moment, notification,
/// leaderboard and icon art (export lists in `art.rs` and
/// `art-exports.json`, Android `AnkiquestArt`, `AnkiquestMomentCard`) and
/// the web's face sizes.
const PICTURES: &[&str] = &[
    "achievement",
    "at_risk",
    "backlog",
    "challenge",
    "challenge_won",
    "challenge_lost",
    "comeback",
    "deck_conquered",
    "freeze_used",
    "goal",
    "level_up",
    "minimum",
    "nudge",
    "place_1",
    "place_2",
    "place_3",
    "icon_happy",
    "icon_worried",
    "icon_frozen",
    "icon_sleepy",
    "face-192",
    "face-32",
];

/// One image kind of one companion.
#[derive(Debug, PartialEq)]
enum Status {
    Present,
    /// Missing; the clients show this pose instead.
    Fallback(String),
    Missing,
}

/// The poses from the fixture: core in order, then extensions with
/// their fallbacks.
struct Poses {
    core: Vec<String>,
    extensions: Vec<(String, String)>,
}

impl Poses {
    /// Every pose, core first.
    fn names(&self) -> impl Iterator<Item = &str> {
        let core = self.core.iter().map(String::as_str);
        core.chain(self.extensions.iter().map(|(pose, _)| pose.as_str()))
    }
}

/// What the command prints or checks.
enum Mode {
    /// The full table.
    Table,
    /// The poses each companion has no master for.
    Missing,
    /// Fails when a pose this older fixture lacks is undrawn.
    Since(PathBuf),
}

/// Whether players can have a companion yet.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage {
    Live,
    Planned,
}

/// A companion, its master folder under `static/` and its stage.
struct Companion {
    id: &'static str,
    folder: &'static str,
    stage: Stage,
}

/// The command's arguments.
const USAGE: &str = "usage: xtask art-coverage [--missing | --since <companion_poses.json>]";

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
        [flag, base] if flag == "--since" => Some(Mode::Since(PathBuf::from(base))),
        _ => None,
    }
}

/// The output for `mode`, or why the check failed.
fn execute(root: &Path, mode: Mode) -> Result<String, String> {
    let list = poses(&root.join(FIXTURE))?;
    match mode {
        Mode::Table => Ok(report(root, &list)),
        Mode::Missing => Ok(backlog(root, &companions(root)?, &list)),
        Mode::Since(base) => {
            let base = poses(&base)?;
            let undrawn = undrawn(root, &companions(root)?, &base, &list);
            if undrawn.is_empty() {
                return Ok("Every added pose has a master for every live companion.\n".into());
            }
            Err(format!(
                "Added poses need a master for every live companion \
                 (AGENTS.md, Companion art):\n{}",
                undrawn.join("\n")
            ))
        }
    }
}

/// Reads the pose fixture.
fn poses(path: &Path) -> Result<Poses, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let json: Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let core = json["core"]
        .as_array()
        .ok_or("core is not a list")?
        .iter()
        .filter_map(|pose| pose.as_str().map(String::from))
        .collect();
    let extensions = json["extensions"]
        .as_object()
        .ok_or("extensions is not an object")?
        .iter()
        .filter_map(|(pose, to)| Some((pose.clone(), to.as_str()?.to_string())))
        .collect();
    Ok(Poses { core, extensions })
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

/// The poses `folder` has no master for, core first.
fn missing<'a>(folder: &Path, poses: &'a Poses) -> Vec<&'a str> {
    poses.names().filter(|pose| !has(folder, pose)).collect()
}

/// The art backlog: per companion, the poses without a master.
fn backlog(root: &Path, companions: &[Companion], poses: &Poses) -> String {
    let mut out = String::from("Poses without a master (AGENTS.md, Companion art)\n\n");
    for companion in companions {
        let folder = root.join("static").join(companion.folder);
        let missing = missing(&folder, poses);
        let list = if missing.is_empty() {
            "none".to_string()
        } else {
            missing.join(", ")
        };
        let stage = match companion.stage {
            Stage::Live => "",
            Stage::Planned => " (planned)",
        };
        out += &format!(
            "- {} (`{}`){stage}: {} missing: {list}\n",
            name(companion.id),
            companion.id,
            missing.len()
        );
    }
    out
}

/// Each pose in `poses` but not in `base` that a live companion has no
/// master for, as `pose: companion`.
fn undrawn(root: &Path, companions: &[Companion], base: &Poses, poses: &Poses) -> Vec<String> {
    let added = poses
        .names()
        .filter(|pose| !base.names().any(|old| old == *pose));
    let live: Vec<_> = companions
        .iter()
        .filter(|companion| companion.stage == Stage::Live)
        .collect();
    let mut undrawn = Vec::new();
    for pose in added {
        for companion in &live {
            if !has(&root.join("static").join(companion.folder), pose) {
                undrawn.push(format!("{pose}: {}", companion.id));
            }
        }
    }
    undrawn
}

/// The older file name a pose's master may still have.
fn old_name(kind: &str) -> Option<&str> {
    RENAMES
        .iter()
        .find(|(pose, _)| *pose == kind)
        .map(|(_, file)| *file)
}

/// Whether `folder` has a master for `kind`: the file the folder's
/// `poses.json` lists for it, else a PNG named after it or its older name.
fn has(folder: &Path, kind: &str) -> bool {
    // A broken list fails `pose_lists_name_masters`; here it lists nothing.
    if let Ok(Some(files)) = listed(folder)
        && let Some(file) = files.get(kind)
    {
        return file.is_file();
    }
    let png = |name: &str| folder.join(format!("{name}.png")).is_file();
    png(kind) || old_name(kind).is_some_and(png)
}

/// The status of `kind` in `folder`, following extension fallbacks.
fn status(folder: &Path, poses: &Poses, kind: &str) -> Status {
    if has(folder, kind) {
        return Status::Present;
    }

    // Walk the fallback chain; the fixture guarantees it ends at a core pose.
    let mut next = kind;
    while let Some((_, to)) = poses.extensions.iter().find(|(pose, _)| pose == next) {
        if has(folder, to) {
            return Status::Fallback(to.clone());
        }
        next = to;
    }
    Status::Missing
}

/// The Markdown table: one row per image kind, one column per companion,
/// then a count per companion.
fn report(root: &Path, poses: &Poses) -> String {
    let folders: Vec<_> = COMPANIONS
        .iter()
        .map(|(_, folder)| root.join("static").join(folder))
        .collect();
    let mut out = String::from(
        "Named masters. yes: master exists; (pose): missing, \
         the clients show that pose; **no**: missing\n\n",
    );
    out += "| image | kind |";
    for (companion, _) in COMPANIONS {
        out += &format!(" {companion} |");
    }
    out += "\n|---|---|";
    out += &"---|".repeat(COMPANIONS.len());
    out += "\n";

    // Each kind's row, counting present images per companion and group.
    let groups: [(&str, Vec<&str>); 3] = [
        ("core", poses.core.iter().map(String::as_str).collect()),
        (
            "extension",
            poses
                .extensions
                .iter()
                .map(|(pose, _)| pose.as_str())
                .collect(),
        ),
        ("picture", PICTURES.to_vec()),
    ];
    let mut counts = vec![[0; 3]; COMPANIONS.len()];
    for (group, (name, kinds)) in groups.iter().enumerate() {
        for kind in kinds {
            out += &format!("| {kind} | {name} |");
            for (index, folder) in folders.iter().enumerate() {
                let status = status(folder, poses, kind);
                if status == Status::Present {
                    counts[index][group] += 1;
                }
                out += &format!(" {} |", cell(&status));
            }
            out += "\n";
        }
    }

    // Totals: named masters present in each group.
    out += "\n";
    for ((companion, _), [core, extension, picture]) in COMPANIONS.iter().zip(counts) {
        let (all_core, all_ext) = (poses.core.len(), poses.extensions.len());
        let all_pictures = PICTURES.len();
        out += &format!(
            "- {companion}: core {core}/{all_core}, extensions {extension}/{all_ext}, \
             pictures {picture}/{all_pictures}\n"
        );
    }
    out
}

/// A table cell for a status.
fn cell(status: &Status) -> String {
    match status {
        Status::Present => "yes".into(),
        Status::Fallback(pose) => format!("({pose})"),
        Status::Missing => "**no**".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ankiquest_art::LISTED;

    fn fixture() -> Poses {
        poses(&root().join(FIXTURE)).unwrap()
    }

    #[test]
    fn statuses_follow_fallbacks() {
        let poses = Poses {
            core: vec!["streak".into(), "frozen".into()],
            extensions: vec![
                ("streak_hot".into(), "streak".into()),
                ("streak_blazing".into(), "streak_hot".into()),
                ("frozen_2".into(), "frozen".into()),
            ],
        };
        let folder = tempfile::tempdir().unwrap();
        fs::write(folder.path().join("streak.png"), b"").unwrap();

        // Present, fallback through two steps, and a chain with no master.
        let check = |kind| status(folder.path(), &poses, kind);
        assert_eq!(Status::Present, check("streak"));
        assert_eq!(Status::Fallback("streak".into()), check("streak_blazing"));
        assert_eq!(Status::Missing, check("frozen_2"));
        assert_eq!(Status::Missing, check("frozen"));

        // A renamed pose is found under its old file name.
        fs::write(folder.path().join("freeze.png"), b"").unwrap();
        assert_eq!(Status::Fallback("frozen".into()), check("frozen_2"));
    }

    #[test]
    fn pose_name_found_before_old_name() {
        let poses = Poses {
            core: vec!["frozen".into()],
            extensions: vec![("frozen_2".into(), "frozen".into())],
        };

        // Only the standard name, as Aki's frozen.png on master.
        let standard = tempfile::tempdir().unwrap();
        fs::write(standard.path().join("frozen.png"), b"").unwrap();
        let check = |kind| status(standard.path(), &poses, kind);
        assert_eq!(Status::Present, check("frozen"));
        assert_eq!(Status::Fallback("frozen".into()), check("frozen_2"));

        // Only the old name still counts.
        let old = tempfile::tempdir().unwrap();
        fs::write(old.path().join("freeze.png"), b"").unwrap();
        let check = |kind| status(old.path(), &poses, kind);
        assert_eq!(Status::Present, check("frozen"));
        assert_eq!(Status::Fallback("frozen".into()), check("frozen_2"));
    }

    #[test]
    fn renames_and_pictures_fit_the_fixture() {
        let poses = fixture();
        let names: Vec<_> = poses
            .core
            .iter()
            .chain(poses.extensions.iter().map(|(pose, _)| pose))
            .collect();
        for (pose, _) in RENAMES {
            assert!(names.iter().any(|name| name == pose), "{pose}");
        }
        for picture in PICTURES {
            assert!(!names.iter().any(|name| name == picture), "{picture}");
        }
    }

    #[test]
    fn lists_every_companion() {
        let path = root().join("tests/fixtures/companion_unlock.json");
        let json: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let unlockable = json["companions"].as_array().unwrap();
        for companion in unlockable {
            let id = companion["companion"].as_str().unwrap();
            assert!(COMPANIONS.iter().any(|(known, _)| *known == id), "{id}");
        }
    }

    #[test]
    fn bluma_has_the_complete_core() {
        let poses = fixture();
        let (_, folder) = COMPANIONS.iter().find(|(id, _)| *id == "cow").unwrap();
        let folder = root().join("static").join(folder);
        for pose in &poses.core {
            assert_eq!(Status::Present, status(&folder, &poses, pose), "{pose}");
        }
    }

    #[test]
    fn asleep_masters_use_the_pose_name() {
        // #32: no master keeps the older `sleeping` file name.
        for (companion, folder) in COMPANIONS {
            let folder = root().join("static").join(folder);
            assert!(!folder.join("sleeping.png").exists(), "{companion}");
        }
        assert!(RENAMES.iter().all(|(pose, _)| *pose != "asleep"));
    }

    #[test]
    fn counts_named_anki_tan_art() {
        let poses = fixture();
        let full = root().join("static/anki-tan");
        for pose in ["asleep", "sad", "neglected", "frozen"] {
            assert_eq!(Status::Present, status(&full, &poses, pose), "{pose}");
        }

        // Chibi's generated master stays under the same character licence.
        let chibi = root().join("static/anki-tan/chibi");
        assert_eq!(Status::Present, status(&chibi, &poses, "frozen"));
        assert_eq!(
            Status::Fallback("frozen".into()),
            status(&chibi, &poses, "frozen_3")
        );
    }

    #[test]
    fn anki_tan_pack_art_counts() {
        // The pack art static/anki-tan/README.md maps to poses, kept in the
        // folders' poses.json, covers both versions' core.
        let poses = fixture();
        for folder in ["static/anki-tan", "static/anki-tan/chibi"] {
            let folder = root().join(folder);
            for pose in &poses.core {
                let status = status(&folder, &poses, pose);
                assert_eq!(Status::Present, status, "{}: {pose}", folder.display());
            }
        }
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

    fn list(core: &[&str], extensions: &[(&str, &str)]) -> Poses {
        Poses {
            core: core.iter().map(|pose| pose.to_string()).collect(),
            extensions: extensions
                .iter()
                .map(|(pose, to)| (pose.to_string(), to.to_string()))
                .collect(),
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
    fn added_pose_needs_every_live_master() {
        let base = list(&["happy"], &[]);
        let poses = list(&["happy"], &[("winner", "happy")]);

        // Only Aki has it: Bluma's is missing, Anki-tan is planned.
        let root = checkout(&["aki/winner.png", "anki-tan/happy.png"]);
        let cast = companions(root.path()).unwrap();
        let missing = undrawn(root.path(), &cast, &base, &poses);
        assert_eq!(missing, ["winner: cow"]);

        // Every live companion has it.
        let root = checkout(&["aki/winner.png", "cow/winner.png"]);
        let cast = companions(root.path()).unwrap();
        assert!(undrawn(root.path(), &cast, &base, &poses).is_empty());
    }

    #[test]
    fn existing_gaps_do_not_fail() {
        // Neither companion has `happy`, but master's list already had it.
        let poses = list(&["happy"], &[("winner", "happy")]);
        let root = checkout(&[]);
        let companions = companions(root.path()).unwrap();
        assert!(undrawn(root.path(), &companions, &poses, &poses).is_empty());
    }

    #[test]
    fn backlog_lists_missing_poses() {
        let poses = list(&["happy", "sad"], &[("winner", "happy")]);
        let root = checkout(&[
            "aki/celebrate.png",
            "aki/sad.png",
            "aki/winner.png",
            "cow/sad.png",
        ]);
        let companions = companions(root.path()).unwrap();
        assert_eq!(
            backlog(root.path(), &companions, &poses),
            "Poses without a master (AGENTS.md, Companion art)\n\n\
             - Aki (`aki`): 0 missing: none\n\
             - Bluma (`cow`): 2 missing: happy, winner\n\
             - Anki-tan (`anki_tan`) (planned): 3 missing: happy, sad, winner\n"
        );
    }

    #[test]
    fn parses_the_modes() {
        let args = |list: &[&str]| list.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        assert!(matches!(mode(&args(&[])), Some(Mode::Table)));
        assert!(matches!(mode(&args(&["--missing"])), Some(Mode::Missing)));
        assert!(matches!(
            mode(&args(&["--since", "base.json"])),
            Some(Mode::Since(path)) if path == Path::new("base.json")
        ));
        assert!(mode(&args(&["--since"])).is_none());
        assert!(mode(&args(&["--other"])).is_none());
    }

    #[test]
    fn listed_files_are_masters() {
        let poses = list(&["face", "happy"], &[("welcome", "happy")]);
        let root = checkout(&["aki/emoji/smile.png", "aki/happy.png"]);
        let folder = root.path().join("static/aki");
        let json = r#"{"poses": {"face": "emoji/smile.png", "welcome": "emoji/wave.png"}}"#;
        fs::write(folder.join(LISTED), json).unwrap();

        // Listed and present, named after the pose, listed but absent.
        assert_eq!(Status::Present, status(&folder, &poses, "face"));
        assert_eq!(Status::Present, status(&folder, &poses, "happy"));
        assert_eq!(
            Status::Fallback("happy".into()),
            status(&folder, &poses, "welcome")
        );
    }

    #[test]
    fn pose_lists_name_masters() {
        let poses = fixture();
        for (companion, folder) in COMPANIONS {
            let folder = root().join("static").join(folder);
            let Some(files) = listed(&folder).unwrap() else {
                continue;
            };
            for (pose, file) in files {
                assert!(
                    poses.names().any(|known| known == pose),
                    "{companion}: {pose}"
                );
                assert!(file.is_file(), "{companion}: {}", file.display());
            }
        }
    }

    #[test]
    fn names_every_companion() {
        for (id, _) in COMPANIONS {
            assert!(NAMES.iter().any(|(known, _)| known == id), "{id}");
        }
    }

    #[test]
    fn missing_masters_do_not_count() {
        let checkout = tempfile::tempdir().unwrap();
        let folder = checkout.path().join("static/anki-tan");
        assert!(!has(&folder, "asleep"));
        assert!(!has(&folder, "frozen"));
    }
}
