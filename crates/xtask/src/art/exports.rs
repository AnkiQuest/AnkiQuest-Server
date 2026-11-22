//! The manifest of companion art outside `static/` (`art-exports.json`).
//!
//! `exports` lists every committed copy of a master: the file, its master
//! and the SHA-256 the exporter wrote, so the tests can regenerate each
//! copy and compare. Copies are generated, so a copy nothing reads any
//! more is deleted, not listed; only the masters keep old art.
//!
//! This repository's manifest lists its own copies. An AnkiQuest-Desktop
//! checkout keeps one in the same format for the add-on's copies, which
//! `export-addon-art` writes.

use std::fs;
use std::path::{Component, Path};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// This repository's manifest, relative to its root. No exporter writes
/// here; the guard tests read it.
#[cfg(test)]
pub(super) const MANIFEST: &str = "crates/xtask/art-exports.json";

/// The add-on's manifest, relative to an AnkiQuest-Desktop checkout.
pub(super) const DESKTOP_MANIFEST: &str = "art-exports.json";

/// What the manifest says about its own format.
const ABOUT: &str = "Companion art outside static/<companion>/. `exports`: \
committed copies of a master, written by the named xtask exporter, with the SHA-256 \
of the file it wrote; the tests regenerate each copy and compare. Export deletes \
a copy whose master is gone. See \
docs/branding/mascot-source.md.";

/// One committed copy of a master.
pub(super) struct Export {
    pub(super) file: String,
    pub(super) master: String,
    pub(super) exporter: Exporter,
    pub(super) sha256: String,
}

/// The exporter that writes a copy.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Exporter {
    /// `export-addon-art`: 256 px palette PNGs.
    Addon,
}

impl Exporter {
    fn name(self) -> &'static str {
        match self {
            Exporter::Addon => "addon",
        }
    }

    fn parse(name: &str) -> Option<Exporter> {
        [Exporter::Addon]
            .into_iter()
            .find(|exporter| exporter.name() == name)
    }
}

/// The whole manifest.
pub(super) struct Manifest {
    pub(super) exports: Vec<Export>,
}

/// Reads this repository's manifest.
#[cfg(test)]
pub(super) fn read() -> Result<Manifest, String> {
    read_at(&super::root().join(MANIFEST))
}

/// Reads the manifest at `path`.
pub(super) fn read_at(path: &Path) -> Result<Manifest, String> {
    let name = path.display();
    let text = fs::read_to_string(path).map_err(|e| format!("{name}: {e}"))?;
    let value: Value = serde_json::from_str(&text).map_err(|e| format!("{name}: {e}"))?;
    let field = |entry: &Value, field: &str| {
        entry[field]
            .as_str()
            .map(String::from)
            .ok_or(format!("{name}: an entry has no {field}"))
    };

    // Committed copies, each with its exporter.
    let mut exports = Vec::new();
    for entry in value["exports"].as_array().ok_or("no exports")? {
        let exporter = field(entry, "exporter")?;
        exports.push(Export {
            file: field(entry, "file")?,
            master: field(entry, "master")?,
            exporter: Exporter::parse(&exporter).ok_or(format!("no exporter {exporter}"))?,
            sha256: field(entry, "sha256")?,
        });
    }

    // Every path stays inside the checkout it is read from.
    let outside = exports
        .iter()
        .flat_map(|e| [&e.file, &e.master])
        .find(|path| !is_relative(path));
    if let Some(path) = outside {
        return Err(format!("{name}: {path} is not a relative path"));
    }

    // Each copy is written by one entry.
    let mut files: Vec<&str> = exports.iter().map(|e| e.file.as_str()).collect();
    files.sort_unstable();
    if let Some([file, _]) = files.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(format!("{name}: {file} is listed twice"));
    }
    Ok(Manifest { exports })
}

/// Whether `path` is relative and never leaves its folder.
fn is_relative(path: &str) -> bool {
    Path::new(path)
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
}

/// Writes `manifest` to `path`, in its committed format.
pub(super) fn write(path: &Path, manifest: &Manifest) -> Result<(), String> {
    let exports: Vec<Value> = manifest
        .exports
        .iter()
        .map(|export| {
            json!({
                "file": export.file,
                "master": export.master,
                "exporter": export.exporter.name(),
                "sha256": export.sha256,
            })
        })
        .collect();
    let value = json!({"_about": ABOUT, "exports": exports});
    let text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())? + "\n";
    fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Lower-case hex SHA-256 of `bytes`.
pub(super) fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_listed_twice_is_refused() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join(DESKTOP_MANIFEST);
        let entry = json!({"file": "a.png", "master": "static/aki/face.png",
            "exporter": "addon", "sha256": ""});
        let value = json!({"exports": [entry, entry]});
        fs::write(&path, value.to_string()).unwrap();
        let Err(error) = read_at(&path) else {
            panic!("a duplicate was accepted");
        };
        assert!(error.contains("a.png is listed twice"), "{error}");
    }

    #[test]
    fn a_path_outside_the_checkout_is_refused() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join(DESKTOP_MANIFEST);
        let export = |file: &str, master: &str| json!({"file": file, "master": master, "exporter": "addon", "sha256": ""});
        let face = "static/aki/face.png";
        let manifests = [
            json!({"exports": [export("../a.png", face)]}),
            json!({"exports": [export("/tmp/a.png", face)]}),
            json!({"exports": [export("a.png", "static/../../a.png")]}),
        ];
        for value in manifests {
            fs::write(&path, value.to_string()).unwrap();
            let Err(error) = read_at(&path) else {
                panic!("accepted {value}");
            };
            assert!(error.contains("not a relative path"), "{error}");
        }
    }
}
