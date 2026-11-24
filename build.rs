//! Builds the web pages into `OUT_DIR/web`, where the server embeds them:
//! TypeScript from `web/` through esbuild, the translation module from
//! `crates/i18n-wasm` as WebAssembly, and each page in `static/` with its
//! `<script data-entry="…"></script>` placeholders replaced by the compiled code.
//!
//! Also exports the companion pictures the server embeds into `OUT_DIR/art`
//! from the masters in the pinned AnkiQuest-Art release, staged into `art/`, found through
//! its `manifest.json`: share-card art and the pictures the pages show.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Scripts served as files, and the page scripts inlined into their HTML.
const ENTRIES: [&str; 8] = [
    "site",
    "avatars",
    "personal",
    "pages/index",
    "pages/community",
    "pages/login",
    "pages/conquered",
    "pages/dev",
];
const PAGES: [&str; 8] = [
    "index",
    "community",
    "login",
    "personal",
    "privacy",
    "conquered",
    "dev",
    "oauth",
];

/// Poses a share card shows, as the fixture names them. A companion without
/// one shows the pose its fallbacks reach (`src/cards.rs`).
const CARD_POSES: [&str; 5] = ["happy", "streak", "frozen", "freeze", "winner"];

/// Companions whose cards show their own art. Bluma's cards show Ankilope's
/// art turned purple (`src/cards.rs`), so hers is not exported yet.
const CARD_COMPANIONS: [&str; 3] = ["aki", "anki_tan", "ankilope"];

/// Share-card art edge in pixels: cards draw it at up to 470 px.
const CARD_SIZE: u32 = 512;

/// How the server serves a picture of a master.
#[derive(Clone, Copy)]
enum Size {
    /// The master's own bytes.
    Master,
    /// Fitted into this many pixels square.
    Fit(u32),
}

/// Face icons: small copies of `face`, which pages draw at 25 to 72 pixels.
const FACE_192: Size = Size::Fit(192);
const FACE_32: Size = Size::Fit(32);

/// Pictures of masters larger than the pages need: pages draw them at up
/// to 132 CSS pixels.
const WEB: Size = Size::Fit(320);

/// Every companion picture the server serves at `/<folder>/<name>.png`:
/// companion id, URL folder, name, the pose it shows and its size.
/// `web/aki.ts` asks for these. `celebrate` keeps the name wins store (the
/// `art` field of the wins API) and shows the happy pose. A companion
/// without the pose's master shows the first pose its fallbacks reach; a
/// picture whose chain reaches no master is not served.
const PICTURES: [(&str, &str, &str, &str, Size); 20] = [
    ("aki", "aki", "welcome", "welcome", Size::Master),
    ("aki", "aki", "review", "review", Size::Master),
    ("aki", "aki", "celebrate", "happy", Size::Master),
    ("aki", "aki", "streak", "streak", Size::Master),
    ("aki", "aki", "freeze", "freeze", Size::Master),
    ("aki", "aki", "winner", "winner", Size::Master),
    ("aki", "aki", "face", "face", Size::Master),
    ("aki", "aki", "face-192", "face", FACE_192),
    ("aki", "aki", "face-32", "face", FACE_32),
    ("ankilope", "ankilope", "welcome", "welcome", Size::Master),
    ("ankilope", "ankilope", "study", "study", Size::Master),
    ("ankilope", "ankilope", "celebrate", "happy", Size::Master),
    ("ankilope", "ankilope", "face", "face", Size::Master),
    ("ankilope", "ankilope", "face-192", "face", FACE_192),
    ("anki_tan", "anki-tan", "welcome", "welcome", WEB),
    ("anki_tan", "anki-tan", "review", "review", WEB),
    ("anki_tan", "anki-tan", "happy", "happy", WEB),
    ("anki_tan", "anki-tan", "streak", "streak", WEB),
    ("anki_tan", "anki-tan", "frozen", "frozen", WEB),
    ("anki_tan", "anki-tan", "face-192", "face", FACE_192),
];

type Error = Box<dyn std::error::Error>;

fn main() -> Result<(), Error> {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let out = PathBuf::from(std::env::var("OUT_DIR")?).join("web");
    std::fs::create_dir_all(&out)?;
    for watched in [
        "web",
        "static",
        ankiquest_art::MASTERS,
        "crates/art",
        "crates/i18n",
        "crates/i18n-wasm",
        "tests/fixtures/companion_poses.json",
        "Cargo.lock",
    ] {
        println!("cargo::rerun-if-changed={watched}");
    }
    println!("cargo::rerun-if-env-changed=ESBUILD");

    art(&root, &out.with_file_name("art"))?;
    typescript(&root, &out)?;
    wasm(&root, &out)?;
    for page in PAGES {
        let source = std::fs::read_to_string(root.join(format!("static/{page}.html")))?;
        std::fs::write(out.join(format!("{page}.html")), inline(&source, &out)?)?;
    }
    Ok(())
}

/// The masters, found by companion and pose through the staged release's
/// manifest, and the fixture's fallbacks.
struct Masters {
    folder: PathBuf,
    manifest: ankiquest_art::Manifest,
    poses: ankiquest_art::Poses,
}

impl Masters {
    fn read(root: &Path) -> Result<Masters, Error> {
        let folder = root.join(ankiquest_art::MASTERS);
        let manifest = ankiquest_art::Manifest::read(&folder).map_err(|error| {
            format!("{error}; is the pinned release staged? (node tests/tools/stage-art.mjs)")
        })?;
        let fixture = std::fs::read_to_string(root.join(ankiquest_art::POSES))?;
        let poses = ankiquest_art::Poses::parse(&fixture)?;
        Ok(Masters {
            folder,
            manifest,
            poses,
        })
    }

    /// The master of `companion`'s `pose`, if it has one.
    fn file(&self, companion: &str, pose: &str) -> Option<PathBuf> {
        let master = self.manifest.master(companion, pose)?;
        Some(self.folder.join(&master.file))
    }

    /// The first pose in `pose`'s fallback chain that `companion` has a
    /// master for.
    fn shown<'a>(&'a self, companion: &str, pose: &'a str) -> Option<&'a str> {
        self.poses
            .resolve(pose, |pose| self.manifest.has(companion, pose))
    }
}

/// Writes the card art and the served pictures, with `cards.rs`, which
/// lists the card art by companion and pose for `src/cards.rs`, and
/// `pictures.rs`, which lists the served pictures for `src/aki.rs`.
fn art(root: &Path, out: &Path) -> Result<(), Error> {
    std::fs::create_dir_all(out)?;
    let masters = Masters::read(root)?;

    // Card art for each companion that has its own: each card pose's master,
    // or the one its fallbacks reach.
    let mut cards = Vec::new();
    for companion in CARD_COMPANIONS {
        let mut shown: Vec<&str> = CARD_POSES
            .iter()
            .filter_map(|pose| masters.shown(companion, pose))
            .collect();
        shown.sort_unstable();
        shown.dedup();
        for pose in shown {
            let Some(master) = masters.file(companion, pose) else {
                continue;
            };
            let path = out
                .join("cards")
                .join(companion)
                .join(format!("{pose}.png"));
            export(&master, &path, Size::Fit(CARD_SIZE))?;
            cards.push(format!(
                "({companion:?}, {pose:?}, include_bytes!({path:?}))"
            ));
        }
    }
    let code = format!(
        "/// Card art by companion and pose, made by build.rs.\n\
         const CARD_ART: [(&str, &str, &[u8]); {}] = [{}];\n",
        cards.len(),
        cards.join(", ")
    );
    std::fs::write(out.join("cards.rs"), code)?;

    // The pictures pages ask for, each from its pose or its fallback.
    let mut served = Vec::new();
    for (companion, folder, name, pose, size) in PICTURES {
        let Some(master) = masters
            .shown(companion, pose)
            .and_then(|pose| masters.file(companion, pose))
        else {
            continue;
        };
        let path = out.join(folder).join(format!("{name}.png"));
        export(&master, &path, size)?;
        served.push(format!(
            "(\"/{folder}/{name}.png\", include_bytes!({path:?}))"
        ));
    }
    let code = format!(
        "/// Companion pictures by the path they are served at, made by build.rs.\n\
         const PICTURES: [(&str, &[u8]); {}] = [{}];\n",
        served.len(),
        served.join(", ")
    );
    std::fs::write(out.join("pictures.rs"), code)?;
    Ok(())
}

/// Writes `master` to `path` at `size`, as PNG.
fn export(master: &Path, path: &Path, size: Size) -> Result<(), Error> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let Size::Fit(size) = size else {
        std::fs::copy(master, path)?;
        return Ok(());
    };
    let image = ankiquest_art::fit(ankiquest_art::load(master)?, size);
    std::fs::write(path, ankiquest_art::encode(&image)?)?;
    Ok(())
}

fn esbuild(root: &Path) -> Result<Command, Error> {
    if let Some(path) = std::env::var_os("ESBUILD") {
        return Ok(Command::new(path));
    }
    let local = root.join("node_modules/.bin").join(if cfg!(windows) {
        "esbuild.cmd"
    } else {
        "esbuild"
    });
    if local.exists() {
        return Ok(Command::new(local));
    }
    match Command::new("esbuild").arg("--version").output() {
        Ok(_) => Ok(Command::new("esbuild")),
        Err(_) => {
            Err("esbuild not found: run `npm ci`, put esbuild on PATH, or set ESBUILD".into())
        }
    }
}

fn typescript(root: &Path, out: &Path) -> Result<(), Error> {
    let status = esbuild(root)?
        .current_dir(root)
        .args(ENTRIES.map(|entry| format!("web/{entry}.ts")))
        .args([
            "--bundle",
            "--format=iife",
            "--target=es2022",
            "--charset=utf8",
            "--log-level=warning",
            "--outbase=web",
        ])
        .arg(format!("--outdir={}", out.display()))
        .status()?;
    if !status.success() {
        return Err("esbuild failed".into());
    }
    Ok(())
}

fn wasm(root: &Path, out: &Path) -> Result<(), Error> {
    let target_dir = out.parent().unwrap().join("wasm-target");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .current_dir(root)
        .args([
            "build",
            "--package=ankiquest-i18n-wasm",
            "--target=wasm32-unknown-unknown",
            "--profile=wasm",
        ])
        .arg("--target-dir")
        .arg(&target_dir);
    // Flags meant for the server's own target must not reach the wasm build.
    for variable in [
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTFLAGS",
        "CARGO_BUILD_TARGET",
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_RUSTFLAGS",
        "RUSTC_WORKSPACE_WRAPPER",
    ] {
        command.env_remove(variable);
    }
    let status = command.status()?;
    if !status.success() {
        return Err(
            "building the translation module failed; is the wasm32-unknown-unknown target installed? \
             (rustup target add wasm32-unknown-unknown)"
                .into(),
        );
    }
    std::fs::copy(
        target_dir.join("wasm32-unknown-unknown/wasm/ankiquest_i18n_wasm.wasm"),
        out.join("i18n.wasm"),
    )?;
    Ok(())
}

/// Replaces each `<script data-entry="name"></script>` with the compiled entry.
fn inline(page: &str, out: &Path) -> Result<String, Error> {
    const OPEN: &str = "<script data-entry=\"";
    const CLOSE: &str = "\"></script>";
    let mut result = String::with_capacity(page.len());
    let mut rest = page;
    while let Some(start) = rest.find(OPEN) {
        result += &rest[..start];
        let after = &rest[start + OPEN.len()..];
        let end = after.find(CLOSE).ok_or("unterminated script placeholder")?;
        let entry = &after[..end];
        if !ENTRIES.contains(&entry) {
            return Err(format!("no web entry {entry}").into());
        }
        let code = std::fs::read_to_string(out.join(format!("{entry}.js")))?;
        if code.contains("</script") {
            return Err(format!("{entry} cannot be inlined: it contains </script").into());
        }
        result += "<script>\n";
        result += &code;
        result += "</script>";
        rest = &after[end + CLOSE.len()..];
    }
    result += rest;
    Ok(result)
}
