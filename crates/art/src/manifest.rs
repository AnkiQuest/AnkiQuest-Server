//! AnkiQuest-Art's `manifest.json`: every master, with its companion, pose
//! and licence, and the pictures that show more than one companion.
//!
//! ```json
//! {"masters": [{"companion": "aki", "pose": "face", "kind": "core",
//!   "file": "aki/face.png", "licence": "AGPL-3.0-or-later"}],
//!  "shared": [{"name": "friends", "shows": ["aki", "ankilope"],
//!   "file": "duo/friends.png", "licence": "AGPL-3.0-or-later"}]}
//! ```
//!
//! Files are relative to the submodule. The archive is never listed.

use std::fs;
use std::path::Path;

use serde_json::Value;

/// The manifest's file name in the submodule.
const FILE: &str = "manifest.json";

/// One master: a pose of one companion.
#[derive(Clone, Debug, PartialEq)]
pub struct Master {
    /// Companion id, such as `cow` or `anki_tan_chibi`.
    pub companion: String,
    pub pose: String,
    /// The file, relative to the submodule.
    pub file: String,
    /// SPDX id of the picture's licence.
    pub licence: String,
}

/// A picture of more than one companion, such as the duo.
#[derive(Clone, Debug, PartialEq)]
pub struct Shared {
    pub name: String,
    pub shows: Vec<String>,
    pub file: String,
    pub licence: String,
}

/// The whole manifest.
#[derive(Debug, PartialEq)]
pub struct Manifest {
    pub masters: Vec<Master>,
    pub shared: Vec<Shared>,
}

/// The text field `key` of `entry`.
fn text(entry: &Value, key: &str) -> Result<String, String> {
    entry[key]
        .as_str()
        .map(String::from)
        .ok_or(format!("{FILE}: an entry has no {key}"))
}

impl Manifest {
    /// Reads the manifest of the submodule at `art`.
    pub fn read(art: &Path) -> Result<Manifest, String> {
        let path = art.join(FILE);
        let json = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Manifest::parse(&json)
    }

    /// Parses the manifest's JSON.
    pub fn parse(json: &str) -> Result<Manifest, String> {
        let value: Value = serde_json::from_str(json).map_err(|e| format!("{FILE}: {e}"))?;
        let list = |key: &str| {
            value[key]
                .as_array()
                .cloned()
                .ok_or(format!("{FILE}: {key} is not a list"))
        };

        // Masters of one companion each.
        let mut masters = Vec::new();
        for entry in list("masters")? {
            masters.push(Master {
                companion: text(&entry, "companion")?,
                pose: text(&entry, "pose")?,
                file: text(&entry, "file")?,
                licence: text(&entry, "licence")?,
            });
        }

        // Pictures of several companions.
        let mut shared = Vec::new();
        for entry in list("shared")? {
            let shows = entry["shows"]
                .as_array()
                .ok_or(format!("{FILE}: shows is not a list"))?
                .iter()
                .map(|id| id.as_str().map(String::from))
                .collect::<Option<_>>()
                .ok_or(format!("{FILE}: a companion in shows is not text"))?;
            shared.push(Shared {
                name: text(&entry, "name")?,
                shows,
                file: text(&entry, "file")?,
                licence: text(&entry, "licence")?,
            });
        }
        Ok(Manifest { masters, shared })
    }

    /// Every file the manifest lists, relative to the submodule.
    pub fn files(&self) -> impl Iterator<Item = &str> {
        let masters = self.masters.iter().map(|master| master.file.as_str());
        masters.chain(self.shared.iter().map(|shared| shared.file.as_str()))
    }

    /// The licence of `companion`'s masters; `None` when it has none.
    pub fn licence(&self, companion: &str) -> Option<&str> {
        self.masters
            .iter()
            .find(|master| master.companion == companion)
            .map(|master| master.licence.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// The submodule in this checkout.
    fn art() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(crate::MASTERS)
    }

    #[test]
    fn parses_masters_and_shared_pictures() {
        let json = r#"{"masters": [{"companion": "aki", "pose": "face", "kind": "core",
            "file": "aki/face.png", "licence": "AGPL-3.0-or-later"}],
            "shared": [{"name": "friends", "shows": ["aki", "ankilope"],
            "file": "duo/friends.png", "licence": "AGPL-3.0-or-later"}]}"#;
        let manifest = Manifest::parse(json).unwrap();
        assert_eq!(manifest.masters[0].pose, "face");
        assert_eq!(manifest.shared[0].shows, ["aki", "ankilope"]);
        assert_eq!(
            manifest.files().collect::<Vec<_>>(),
            ["aki/face.png", "duo/friends.png"]
        );
        assert_eq!(manifest.licence("aki"), Some("AGPL-3.0-or-later"));
        assert_eq!(manifest.licence("cow"), None);
        assert!(Manifest::parse(r#"{"masters": [{"companion": "aki"}], "shared": []}"#).is_err());
    }

    #[test]
    fn every_master_is_its_pose_file() {
        // A master is `<companion folder>/<pose>.png`, outside the archive.
        let manifest = Manifest::read(&art()).unwrap();
        for master in &manifest.masters {
            let file = Path::new(&master.file);
            assert_eq!(
                file.file_name().unwrap().to_string_lossy(),
                format!("{}.png", master.pose)
            );
            assert!(art().join(file).is_file(), "{}", master.file);
            assert!(!crate::archived(&master.file), "{}", master.file);
        }
        for file in manifest.shared.iter().map(|shared| &shared.file) {
            assert!(art().join(file).is_file(), "{file}");
        }
    }

    #[test]
    fn each_master_is_listed_once() {
        let manifest = Manifest::read(&art()).unwrap();
        let mut files: Vec<_> = manifest.files().collect();
        let count = files.len();
        files.sort_unstable();
        files.dedup();
        assert_eq!(count, files.len());
    }
}
