//! Prints which companion art exists: every pose in
//! `tests/fixtures/companion_poses.json` and every other per-companion
//! picture the clients show, for each companion, as a Markdown table.
//!
//! A pose's master is `static/<folder>/<pose>.png`, or the older name
//! [`RENAMES`] gives when that file is absent. A missing extension shows
//! the first pose its fallback chain reaches that exists. It counts named
//! masters only, not art the apps map from other files (such as Anki-tan's
//! pack). Read-only.

use std::fs;
use std::path::Path;
use std::process::ExitCode;

use serde_json::Value;

use super::root;

/// The pose list both clients use.
const FIXTURE: &str = "tests/fixtures/companion_poses.json";

/// Every companion and its master folder under `static/`, in picker order.
const COMPANIONS: &[(&str, &str)] = &[
    ("aki", "aki"),
    ("ankilope", "ankilope"),
    ("cow", "cow"),
    ("anki_tan", "anki-tan"),
    ("anki_tan_chibi", "anki-tan-chibi"),
    ("leech_tan", "leech-tan"),
];

/// Poses whose masters may still have the file name they had before the
/// fixture; the pose's own name is checked first.
const RENAMES: &[(&str, &str)] = &[
    ("asleep", "sleeping"),
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

/// Runs the report; it takes no arguments.
pub fn run(args: &[String]) -> ExitCode {
    if !args.is_empty() {
        eprintln!("usage: xtask art-coverage");
        return ExitCode::FAILURE;
    }
    let root = root();
    match poses(&root.join(FIXTURE)) {
        Ok(poses) => {
            print!("{}", report(&root, &poses));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
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

/// The older file name a pose's master may still have.
fn old_name(kind: &str) -> Option<&str> {
    RENAMES
        .iter()
        .find(|(pose, _)| *pose == kind)
        .map(|(_, file)| *file)
}

/// Whether `folder` has a master for `kind`: under its own name first,
/// then under its older name.
fn has(folder: &Path, kind: &str) -> bool {
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
        "Named masters in static/<folder>/ only. yes: master exists; (pose): missing, \
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
}
