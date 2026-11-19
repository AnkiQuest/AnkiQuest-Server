//! A companion folder's pose list, `poses.json`: the file that shows each
//! pose, for folders whose masters are not named after their poses (Anki-tan's
//! licensed pack). Paths are relative to the folder.
//!
//! ```json
//! {"_about": "...", "poses": {"face": "sprite/idle/anki-tan_idle_0001.png"}}
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// The pose list's file name inside a companion folder.
pub const LISTED: &str = "poses.json";

/// Each pose `folder/poses.json` lists, with its file resolved against
/// `folder`; `None` when the folder has no list.
pub fn listed(folder: &Path) -> Result<Option<BTreeMap<String, PathBuf>>, String> {
    let path = folder.join(LISTED);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let poses = json["poses"]
        .as_object()
        .ok_or_else(|| format!("{}: poses is not an object", path.display()))?;

    // Each pose and its file, relative to the folder.
    let mut files = BTreeMap::new();
    for (pose, file) in poses {
        let file = file
            .as_str()
            .ok_or_else(|| format!("{}: {pose} is not a path", path.display()))?;
        files.insert(pose.clone(), folder.join(file));
    }
    Ok(Some(files))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_folder_list() {
        let folder = std::env::temp_dir().join(format!("listed-{}", std::process::id()));
        fs::create_dir_all(&folder).unwrap();
        assert_eq!(listed(&folder), Ok(None));

        // Paths resolve against the folder, `..` included.
        let json = r#"{"poses": {"face": "emoji/smile.png", "sad": "../cry.png"}}"#;
        fs::write(folder.join(LISTED), json).unwrap();
        let files = listed(&folder).unwrap().unwrap();
        assert_eq!(files["face"], folder.join("emoji/smile.png"));
        assert_eq!(files["sad"], folder.join("../cry.png"));

        // A list that is not one fails.
        fs::write(folder.join(LISTED), r#"{"poses": ["face"]}"#).unwrap();
        assert!(listed(&folder).is_err());
        fs::remove_dir_all(&folder).unwrap();
    }
}
