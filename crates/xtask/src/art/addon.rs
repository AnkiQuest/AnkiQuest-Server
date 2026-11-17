//! Writes the desktop add-on's companion poses into an AnkiQuest-Desktop
//! checkout from the master PNGs in `static/`: every `addon` entry of the
//! checkout's art manifest (`art-exports.json`).
//!
//! Each pose is a 256 px square, 256-colour palette PNG with transparency;
//! an icon pose (`ankiquest_art::ICON_POSES`) is 64 px, and export adds one
//! for every companion with its master. A companion without one shows the
//! pose's fallback. Export writes every pose whose picture changed, retires
//! the file it replaces, and records each file's SHA-256 in the manifest.
//! `--check` writes nothing and fails when a pose differs from what export
//! would write, from the hash in the manifest, or when an icon master has
//! no manifest entry yet.
//!
//! Linux exports are canonical. Another platform, or a new quantiser
//! version, maps a few colours to other palette entries: a few hundred
//! pixels move far while the mean barely moves. Export and check therefore
//! treat a pose as current when it is `close` to a fresh render, so a
//! clean checkout passes everywhere and export leaves the committed files
//! alone.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ankiquest_art::{ICON_POSES, companions_with};
use color_quant::NeuQuant;
use image::RgbaImage;

use super::exports::{self, DESKTOP_MANIFEST, Export, Exporter, Manifest, Retired};
use super::{load, root, shrink, target};

/// Pose edge in pixels: the Today panel shows 96 px at up to 2x, and
/// moments 112 px.
const SIZE: u32 = 256;

/// Icon edge in pixels: an icon beside the streak number, drawn at up to
/// 32 px at 2x.
const ICON: u32 = 64;

/// The add-on's pose folder, relative to a Desktop checkout.
const FOLDER: &str = "addon/art";

/// The masters folder, relative to this repository's root.
const MASTERS: &str = "static";

/// Palette size, the most an indexed PNG holds.
const COLOURS: usize = 256;

/// NeuQuant sampling factor: 1 looks at every pixel, the slowest and best.
const SAMPLING: i32 = 1;

/// Largest mean channel difference, out of 255, between a committed pose
/// and a fresh export: drift moves a few palette entries, a different
/// picture moves far more.
const MEAN_TOLERANCE: f64 = 0.5;

/// A pixel has drifted when one of its channels moves more than this.
const DRIFT: u8 = 16;

/// Largest share of drifted pixels. Drift from Windows/MSVC and from
/// color_quant 2 moves at most 0.4% of a pose; no single pixel is bounded,
/// since one swapped palette entry can move a pixel by 150.
const DRIFTED_SHARE: f64 = 0.01;

/// Where export moves a pose it replaces, inside the Desktop checkout:
/// companion art is never deleted (AGENTS.md, Companion art), and nothing
/// packaged reads this folder.
const RETIRED: &str = "assets/retired";

/// What the command does.
#[derive(PartialEq)]
enum Mode {
    Export,
    Check,
}

/// Runs the exporter: `[--check] <desktop-checkout>`.
pub fn run(args: &[String]) -> ExitCode {
    let (mode, desktop) = match args {
        [desktop] if !desktop.starts_with('-') => (Mode::Export, desktop),
        [flag, desktop] if flag == "--check" && !desktop.starts_with('-') => (Mode::Check, desktop),
        _ => return usage(),
    };
    let desktop = PathBuf::from(desktop);
    let result = match mode {
        Mode::Export => export(&desktop),
        Mode::Check => check(&desktop),
    };
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// Prints the usage text and fails.
fn usage() -> ExitCode {
    eprintln!("usage: xtask export-addon-art [--check] <desktop-checkout>");
    ExitCode::FAILURE
}

/// Whether a manifest entry is an add-on pose or icon.
fn is_pose(export: &Export) -> bool {
    matches!(export.exporter, Exporter::Addon | Exporter::AddonIcon)
}

/// The edge, in pixels, `exporter` writes.
fn size(exporter: Exporter) -> u32 {
    match exporter {
        Exporter::Addon => SIZE,
        Exporter::AddonIcon => ICON,
    }
}

/// The pose `export` describes, made from its master.
fn render(export: &Export) -> Result<Vec<u8>, String> {
    let master = load(&root().join(&export.master))?;
    let (width, height) = target(master.width(), master.height(), size(export.exporter));
    encode(&shrink(master, width, height))
}

/// An entry for every icon master in `masters` that `manifest` does not
/// list yet, as `addon/art/<companion>_<pose>.png`.
fn unlisted(manifest: &Manifest, masters: &Path) -> Result<Vec<Export>, String> {
    let mut icons = Vec::new();
    for pose in ICON_POSES {
        for companion in companions_with(masters, pose)? {
            let file = format!("{FOLDER}/{}_{pose}.png", companion.replace('-', "_"));
            if manifest.exports.iter().any(|export| export.file == file) {
                continue;
            }
            icons.push(Export {
                file,
                master: format!("{MASTERS}/{companion}/{pose}.png"),
                exporter: Exporter::AddonIcon,
                sha256: String::new(),
            });
        }
    }
    Ok(icons)
}

/// Writes every pose whose picture changed into the Desktop checkout,
/// retiring the file it replaces, and records each pose's hash in the
/// checkout's manifest.
fn export(desktop: &Path) -> Result<ExitCode, String> {
    let manifest_path = desktop.join(DESKTOP_MANIFEST);
    let mut manifest = exports::read_at(&manifest_path)?;
    let mut retired = Vec::new();

    // List icons whose masters were added since the last export.
    let icons = unlisted(&manifest, &root().join(MASTERS))?;
    manifest.exports.extend(icons);

    for export in manifest.exports.iter_mut().filter(|export| is_pose(export)) {
        let fresh = render(export)?;
        let path = desktop.join(&export.file);
        let committed = fs::read(&path).ok();
        match step(committed.as_deref(), &fresh)? {
            // Keep a committed pose that differs only by drift.
            Step::Keep => {
                let committed = committed.unwrap_or_default();
                export.sha256 = exports::sha256(&committed);
                println!("{}: current", export.file);
                continue;
            }
            Step::Add => {}

            // Move the previous picture aside before replacing it.
            Step::Replace => {
                let committed = committed.unwrap_or_default();
                retired.push(retire(desktop, export, &committed)?);
            }
        }
        save(&path, &fresh)?;
        export.sha256 = exports::sha256(&fresh);
        println!("{}: {} bytes", export.file, fresh.len());
    }
    // A picture retired before keeps its one entry.
    for entry in retired {
        if !manifest
            .retired
            .iter()
            .any(|known| known.file == entry.file)
        {
            manifest.retired.push(entry);
        }
    }
    exports::write(&manifest_path, &manifest)?;
    Ok(ExitCode::SUCCESS)
}

/// What export does with one pose.
#[derive(PartialEq, Debug)]
enum Step {
    /// No file yet: write it.
    Add,
    /// The committed file is current: leave it.
    Keep,
    /// The picture changed: retire the committed file, then write.
    Replace,
}

/// What export does with a pose, given the committed file and a fresh render.
fn step(committed: Option<&[u8]>, fresh: &[u8]) -> Result<Step, String> {
    let Some(committed) = committed else {
        return Ok(Step::Add);
    };
    if committed == fresh || close(fresh, committed)? {
        return Ok(Step::Keep);
    }
    Ok(Step::Replace)
}

/// Keeps `bytes`, the pose `export` is about to replace, under `RETIRED`
/// of the Desktop checkout with its hash in the name, and returns its
/// manifest entry.
fn retire(desktop: &Path, export: &Export, bytes: &[u8]) -> Result<Retired, String> {
    let file = retired_path(&export.file, &exports::sha256(bytes));
    save(&desktop.join(&file), bytes)?;
    println!("{}: retired to {file}", export.file);
    Ok(Retired {
        file,
        why: format!(
            "Earlier {}, replaced when export-addon-art wrote a new export of {}. Unreferenced.",
            export.file, export.master
        ),
    })
}

/// Where a replaced pose goes: `addon/art/aki_happy.png` with hash
/// `0123abcd…` becomes `assets/retired/addon/art/aki_happy-0123abcd.png`.
fn retired_path(file: &str, sha256: &str) -> String {
    let (stem, extension) = file.rsplit_once('.').unwrap_or((file, "png"));
    format!("{RETIRED}/{stem}-{}.{extension}", &sha256[..8])
}

/// Writes `bytes` to `path`, creating its folder.
fn save(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    }
    fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Compares every pose in the Desktop checkout with a fresh export and
/// with the checkout's manifest.
fn check(desktop: &Path) -> Result<ExitCode, String> {
    let failed = stale(desktop)?;
    for line in &failed {
        println!("{line}");
    }
    if !failed.is_empty() {
        return Ok(ExitCode::FAILURE);
    }
    println!("all add-on poses match");
    Ok(ExitCode::SUCCESS)
}

/// One line for each pose that is missing, edited or out of date.
fn stale(desktop: &Path) -> Result<Vec<String>, String> {
    let mut failed = Vec::new();
    let manifest = exports::read_at(&desktop.join(DESKTOP_MANIFEST))?;

    // An icon master added without running export.
    for icon in unlisted(&manifest, &root().join(MASTERS))? {
        failed.push(format!("not exported: {}", icon.master));
    }

    for export in manifest.exports.iter().filter(|export| is_pose(export)) {
        let path = desktop.join(&export.file);
        let Ok(committed) = fs::read(&path) else {
            failed.push(format!("missing: {}", export.file));
            continue;
        };
        if exports::sha256(&committed) != export.sha256 {
            failed.push(format!("{}: not the file export wrote", export.file));
            continue;
        }
        let fresh = render(export)?;
        if fresh != committed && !close(&fresh, &committed)? {
            failed.push(format!("{}: differs from {}", export.file, export.master));
        }
    }
    Ok(failed)
}

/// Whether two PNGs show the same picture up to drift: a small mean
/// difference and few drifted pixels.
fn close(a: &[u8], b: &[u8]) -> Result<bool, String> {
    let decode = |bytes: &[u8]| {
        image::load_from_memory(bytes)
            .map(|image| image.into_rgba8())
            .map_err(|e| e.to_string())
    };
    let (a, b) = (decode(a)?, decode(b)?);
    if a.dimensions() != b.dimensions() {
        return Ok(false);
    }
    let differences: Vec<u8> = a
        .as_raw()
        .iter()
        .zip(b.as_raw())
        .map(|(x, y)| x.abs_diff(*y))
        .collect();

    // Mean over every channel, share over pixels.
    let total: f64 = differences.iter().map(|&d| f64::from(d)).sum();
    let mean = total / differences.len().max(1) as f64;
    let (pixels, _) = differences.as_chunks::<4>();
    let drifted = pixels
        .iter()
        .filter(|pixel| pixel.iter().any(|&d| d > DRIFT))
        .count();
    let share = drifted as f64 / pixels.len().max(1) as f64;
    Ok(mean <= MEAN_TOLERANCE && share <= DRIFTED_SHARE)
}

/// Encodes `image` as an indexed PNG with a 256-colour RGBA palette.
fn encode(image: &RgbaImage) -> Result<Vec<u8>, String> {
    let quant = NeuQuant::new(SAMPLING, COLOURS, image.as_raw());
    let (pixels, _) = image.as_raw().as_chunks::<4>();
    let indices: Vec<u8> = pixels
        .iter()
        .map(|pixel| quant.index_of(pixel) as u8)
        .collect();

    // Split the RGBA palette into PNG's colour and transparency chunks.
    let rgba = quant.color_map_rgba();
    let (palette, _) = rgba.as_chunks::<4>();
    let colours: Vec<u8> = palette.iter().flat_map(|&[r, g, b, _]| [r, g, b]).collect();
    let alpha: Vec<u8> = palette.iter().map(|&[_, _, _, a]| a).collect();

    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, image.width(), image.height());
    encoder.set_color(png::ColorType::Indexed);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_palette(colours);
    encoder.set_trns(alpha);
    encoder.set_compression(png::Compression::High);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer
        .write_image_data(&indices)
        .map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The add-on's winner pose.
    const WINNER: &str = "addon/art/aki_winner.png";

    /// The manifest entry for the winner pose, before its first export.
    fn winner_export() -> Export {
        Export {
            file: WINNER.into(),
            master: "static/aki/winner.png".into(),
            exporter: Exporter::Addon,
            sha256: String::new(),
        }
    }

    /// A Desktop checkout whose manifest lists the winner pose.
    fn checkout() -> tempfile::TempDir {
        let desktop = tempfile::tempdir().unwrap();
        let manifest = exports::Manifest {
            exports: vec![winner_export()],
            retired: Vec::new(),
        };
        exports::write(&desktop.path().join(DESKTOP_MANIFEST), &manifest).unwrap();
        desktop
    }

    #[test]
    fn export_writes_checks_and_retires() {
        // A fresh checkout misses the pose; export writes it.
        let desktop = checkout();
        let desktop = desktop.path();
        assert_eq!(vec![format!("missing: {WINNER}")], stale(desktop).unwrap());
        export(desktop).unwrap();
        assert!(stale(desktop).unwrap().is_empty());

        // An edited pose fails the check.
        let path = desktop.join(WINNER);
        let blue = RgbaImage::from_pixel(SIZE, SIZE, image::Rgba([0, 0, 255, 255]));
        let edited = encode(&blue).unwrap();
        fs::write(&path, &edited).unwrap();
        assert_eq!(
            vec![format!("{WINNER}: not the file export wrote")],
            stale(desktop).unwrap()
        );

        // Export replaces it and keeps the edited file as retired.
        export(desktop).unwrap();
        assert!(stale(desktop).unwrap().is_empty());
        let manifest = exports::read_at(&desktop.join(DESKTOP_MANIFEST)).unwrap();
        let retired = retired_path(WINNER, &exports::sha256(&edited));
        assert_eq!(
            vec![retired.clone()],
            manifest
                .retired
                .iter()
                .map(|r| r.file.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(edited, fs::read(desktop.join(retired)).unwrap());
    }

    #[test]
    fn retiring_the_same_picture_twice_lists_it_once() {
        let desktop = checkout();
        let desktop = desktop.path();
        export(desktop).unwrap();
        let path = desktop.join(WINNER);
        let blue = RgbaImage::from_pixel(SIZE, SIZE, image::Rgba([0, 0, 255, 255]));
        let edited = encode(&blue).unwrap();

        // Edit, export, edit back to the same bytes, export again.
        for _ in 0..2 {
            fs::write(&path, &edited).unwrap();
            export(desktop).unwrap();
        }
        let manifest = exports::read_at(&desktop.join(DESKTOP_MANIFEST)).unwrap();
        assert_eq!(1, manifest.retired.len());
    }

    #[test]
    fn checkout_is_required() {
        assert_eq!(ExitCode::FAILURE, run(&[]));
        assert_eq!(ExitCode::FAILURE, run(&["--check".into()]));
        let two = ["a".to_string(), "b".to_string()];
        assert_eq!(ExitCode::FAILURE, run(&two));
    }

    #[test]
    fn icon_masters_are_listed_once() {
        let masters = tempfile::tempdir().unwrap();
        for (folder, file) in [
            ("aki", "icon_fire.png"),
            ("anki-tan", "icon_fire.png"),
            ("cow", "happy.png"),
        ] {
            fs::create_dir_all(masters.path().join(folder)).unwrap();
            fs::write(masters.path().join(folder).join(file), b"").unwrap();
        }

        // Both icon masters are new; Bluma has none.
        let mut manifest = Manifest {
            exports: Vec::new(),
            retired: Vec::new(),
        };
        let icons = unlisted(&manifest, masters.path()).unwrap();
        let files: Vec<&str> = icons.iter().map(|icon| icon.file.as_str()).collect();
        assert_eq!(
            files,
            [
                "addon/art/aki_icon_fire.png",
                "addon/art/anki_tan_icon_fire.png"
            ]
        );
        assert_eq!(icons[1].master, "static/anki-tan/icon_fire.png");
        assert!(icons.iter().all(|icon| size(icon.exporter) == ICON));

        // Once listed, an icon is not listed again.
        manifest.exports.extend(icons);
        assert!(unlisted(&manifest, masters.path()).unwrap().is_empty());
    }

    #[test]
    fn a_changed_picture_is_not_close() {
        let red = RgbaImage::from_pixel(8, 8, image::Rgba([255, 0, 0, 255]));
        let blue = RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 255, 255]));
        let (red, blue) = (encode(&red).unwrap(), encode(&blue).unwrap());
        assert!(close(&red, &red).unwrap());
        assert!(!close(&red, &blue).unwrap());
    }

    /// `image` as a plain RGBA PNG.
    fn png(image: &RgbaImage) -> Vec<u8> {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    /// The Aki winner pose as export writes it, decoded.
    fn winner() -> RgbaImage {
        let bytes = render(&winner_export()).unwrap();
        image::load_from_memory(&bytes).unwrap().into_rgba8()
    }

    /// The drift reported on Windows/MSVC for aki_winner.png: one pixel
    /// off by 64, everything else the same.
    #[test]
    fn one_drifted_pixel_is_close() {
        let committed = winner();
        let mut fresh = committed.clone();
        fresh.put_pixel(146, 160, image::Rgba([116, 98, 114, 253]));
        assert!(close(&png(&fresh), &png(&committed)).unwrap());
        assert_eq!(step(Some(&png(&committed)), &png(&fresh)), Ok(Step::Keep));
    }

    /// A 40 px patch painted over the pose is an edit, not drift.
    #[test]
    fn a_painted_patch_is_not_close() {
        let committed = winner();
        let mut edited = committed.clone();
        for (x, y, pixel) in edited.enumerate_pixels_mut() {
            if (100..140).contains(&x) && (100..140).contains(&y) {
                *pixel = image::Rgba([255, 0, 255, 255]);
            }
        }
        assert!(!close(&png(&edited), &png(&committed)).unwrap());
        assert_eq!(
            step(Some(&png(&committed)), &png(&edited)),
            Ok(Step::Replace)
        );
    }

    #[test]
    fn a_missing_pose_is_added() {
        assert_eq!(step(None, &png(&winner())), Ok(Step::Add));
    }

    #[test]
    fn replaced_poses_keep_their_hash() {
        assert_eq!(
            retired_path("addon/art/aki_winner.png", "0123abcd4567"),
            "assets/retired/addon/art/aki_winner-0123abcd.png"
        );
    }
}
