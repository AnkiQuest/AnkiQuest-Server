//! Builds the web pages into `OUT_DIR/web`, where the server embeds them:
//! TypeScript from `web/` through esbuild, the translation module from
//! `crates/i18n-wasm` as WebAssembly, and each page in `static/` with its
//! `<script data-entry="…"></script>` placeholders replaced by the compiled code.
//!
//! Also exports the companion pictures the server embeds into `OUT_DIR/art`,
//! downscaled from the masters in the AnkiQuest-Art submodule (`art/`):
//! share-card art, the small face icons and Anki-tan's web pictures.

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

/// Poses a share card shows, as the fixture names them, and `freeze`
/// (a used streak freeze), which the fixture does not name. A companion
/// without one falls back through the fixture when the card is drawn.
const CARD_POSES: [&str; 5] = ["happy", "streak", "frozen", "freeze", "winner"];

/// Companions whose cards show their own art: id and folder in art/. Bluma's
/// cards show Ankilope's art turned purple (`src/cards.rs`), so hers is not
/// exported yet.
const CARD_COMPANIONS: [(&str, &str); 3] = [
    ("aki", "aki"),
    ("anki_tan", "anki-tan"),
    ("ankilope", "ankilope"),
];

/// Share-card art edge in pixels: cards draw it at up to 470 px.
const CARD_SIZE: u32 = 512;

/// Face icons the pages use: master folder, file name and edge in pixels.
const ICONS: [(&str, &str, u32); 4] = [
    ("aki", "face-192", 192),
    ("aki", "face-32", 32),
    ("ankilope", "face-192", 192),
    ("anki-tan", "face-192", 192),
];

/// Companions whose web pictures are exported smaller, and the poses the
/// pages draw, as `<pose>.png` next to the face icon. `web/aki.ts` maps the
/// page's moods onto these. Anki-tan's masters are larger than the pages
/// need; the other companions' are served as they are (`src/aki.rs`).
const WEB_ART: [(&str, [&str; 5]); 1] = [(
    "anki-tan",
    ["welcome", "review", "happy", "streak", "frozen"],
)];

/// Web picture edge in pixels: pages draw them at up to 132 CSS pixels.
const WEB_SIZE: u32 = 320;

type Error = Box<dyn std::error::Error>;

fn main() -> Result<(), Error> {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let out = PathBuf::from(std::env::var("OUT_DIR")?).join("web");
    std::fs::create_dir_all(&out)?;
    for watched in [
        "web",
        "static",
        "art",
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

/// Writes the card art, face icons and web pictures, and `cards.rs`, which
/// lists the card art by companion and pose for `src/cards.rs`.
fn art(root: &Path, out: &Path) -> Result<(), Error> {
    std::fs::create_dir_all(out)?;
    let masters = root.join(ankiquest_art::MASTERS);
    let mut table = String::from("[\n");
    let mut count = 0;

    // Card art for each companion that has its own.
    for (companion, folder) in CARD_COMPANIONS {
        let folder = masters.join(folder);
        for pose in CARD_POSES {
            let Some(master) = ankiquest_art::master(&folder, pose) else {
                continue;
            };
            let path = out
                .join("cards")
                .join(companion)
                .join(format!("{pose}.png"));
            export(&master, &path, CARD_SIZE)?;
            table += &format!("    ({companion:?}, {pose:?}, include_bytes!({path:?})),\n");
            count += 1;
        }
    }
    table += "]";
    let code = format!(
        "/// Card art by companion and pose, made by build.rs.\n\
         const CARD_ART: [(&str, &str, &[u8]); {count}] = {table};\n"
    );
    std::fs::write(out.join("cards.rs"), code)?;

    // Face icons, from each companion's face master.
    for (folder, name, size) in ICONS {
        let master = required(&masters, folder, "face")?;
        export(&master, &out.join(folder).join(format!("{name}.png")), size)?;
    }

    // Smaller web pictures where the masters are larger than pages need.
    for (folder, poses) in WEB_ART {
        for pose in poses {
            let master = required(&masters, folder, pose)?;
            export(
                &master,
                &out.join(folder).join(format!("{pose}.png")),
                WEB_SIZE,
            )?;
        }
    }
    Ok(())
}

/// The master for `pose` in `folder`, or an error naming it; an empty
/// folder means the submodule was not checked out.
fn required(masters: &Path, folder: &str, pose: &str) -> Result<PathBuf, Error> {
    ankiquest_art::master(&masters.join(folder), pose).ok_or_else(|| {
        format!(
            "{}/{folder} has no {pose} master; is the art submodule checked out? \
             (git submodule update --init)",
            ankiquest_art::MASTERS
        )
        .into()
    })
}

/// Writes `master` fitted into `size` square as PNG to `path`.
fn export(master: &Path, path: &Path, size: u32) -> Result<(), Error> {
    let image = ankiquest_art::fit(ankiquest_art::load(master)?, size);
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
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
