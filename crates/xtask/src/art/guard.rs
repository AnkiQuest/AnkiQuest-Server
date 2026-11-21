//! Fails when companion art sits outside `static/<companion>/` without being
//! generated from a master and checked (AGENTS.md, Companion art).
//!
//! An image is companion art when its path names a companion, or when it
//! looks like a master scaled to another size. It may only be a master in
//! `static/<companion>/`, a picture in an archive folder there, an export in
//! the art manifest, or a retired copy the manifest keeps. Animations are not
//! compared with masters.

use std::fs;
use std::path::{Path, PathBuf};

use image::RgbaImage;
use image::imageops::{self, FilterType};

use super::exports::{self, MANIFEST};
use super::root;

/// Image formats companion art comes in.
const FORMATS: [&str; 3] = ["png", "webp", "gif"];

/// Folders never searched: build output, dependencies, version control.
const SKIPPED: [&str; 3] = ["target", "node_modules", ".git"];

/// The fixture that names every companion but Aki.
const UNLOCKS: &str = "tests/fixtures/companion_unlock.json";

/// Fingerprint edge: images are compared at this size.
const PRINT: u32 = 16;

/// Largest mean channel difference, out of 255, between the fingerprints of
/// a master and a scaled copy of it. Copies measure up to 1.2 (a 32 px
/// icon), the closest two different masters 1.9 (Ankilope's places 2 and 3).
const SAME_PICTURE: f64 = 1.5;

/// Folders whose files may refer to images at runtime or build time.
const CODE: [&str; 5] = ["src", "web", "addon", "crates", "build.rs"];

/// An image's path relative to the repository root, with `/` separators.
fn relative(path: &Path) -> String {
    let path = path.strip_prefix(root()).unwrap_or(path);
    path.to_string_lossy().replace('\\', "/")
}

/// Every file under `folder`, skipping [`SKIPPED`] and hidden folders.
fn files(folder: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !path.is_dir() {
            found.push(path);
            continue;
        }
        if SKIPPED.contains(&name.as_str()) || name.starts_with('.') {
            continue;
        }
        files(&path, found);
    }
}

/// Every image in the repository, sorted.
fn images() -> Vec<String> {
    let mut found = Vec::new();
    files(&root(), &mut found);
    let mut images: Vec<String> = found
        .iter()
        .filter(|path| {
            let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            FORMATS.contains(&extension.to_ascii_lowercase().as_str())
        })
        .map(|path| relative(path))
        .collect();
    images.sort();
    images
}

/// Lower-case letters and digits only, so `anki-tan`, `anki_tan` and
/// `AnkiTan` compare equal.
fn letters(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Every companion's name: Aki, each one the unlock fixture lists, and each
/// folder of masters.
fn companions() -> Vec<String> {
    let text = fs::read_to_string(root().join(UNLOCKS)).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    let mut names = vec!["aki".to_string()];
    for companion in value["companions"].as_array().unwrap() {
        names.push(companion["companion"].as_str().unwrap().to_string());
    }
    let masters = root().join(ankiquest_art::MASTERS);
    for folder in fs::read_dir(masters).unwrap().flatten() {
        if folder.path().is_dir() && folder.file_name() != ankiquest_art::ARCHIVE {
            names.push(folder.file_name().to_string_lossy().into_owned());
        }
    }
    let mut names: Vec<String> = names.iter().map(|name| letters(name)).collect();
    names.sort();
    names.dedup();
    names
}

/// The companion folder in `static/` that holds `image`, if any. An
/// archive folder holds no masters.
fn master_folder(image: &str) -> Option<&str> {
    let rest = image
        .strip_prefix(ankiquest_art::MASTERS)?
        .strip_prefix('/')?;
    let (folder, _) = rest.split_once('/')?;
    (folder != ankiquest_art::ARCHIVE).then_some(folder)
}

/// Whether `image` is in an archive folder among the masters: replaced
/// masters, a source pack, earlier copies. They may be anything.
fn archived(image: &str) -> bool {
    let masters = format!("{}/", ankiquest_art::MASTERS);
    image.starts_with(&masters) && ankiquest_art::archived(image)
}

/// A small picture of an image over grey, and its pixel size; `None` when
/// it cannot be decoded.
struct Print {
    size: (u32, u32),
    pixels: RgbaImage,
}

/// Whether `path` is an animated PNG. An animation is not a scaled copy of
/// a still, even when its first frame is (the Anki-tan pack's sprites).
fn animated(path: &Path) -> bool {
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let reader = std::io::BufReader::new(file);
    image::codecs::png::PngDecoder::new(reader)
        .and_then(|decoder| decoder.is_apng())
        .unwrap_or(false)
}

/// Decodes `image` and shrinks it to a fingerprint; `None` for animations.
fn print(image: &str) -> Option<Print> {
    let path = root().join(image);
    let decoded = if image.ends_with(".webp") {
        let bytes = fs::read(&path).ok()?;
        let decoded = webp::Decoder::new(&bytes).decode()?;
        let (width, height) = (decoded.width(), decoded.height());
        if decoded.is_alpha() {
            RgbaImage::from_raw(width, height, decoded.to_vec())?
        } else {
            let rgb = image::RgbImage::from_raw(width, height, decoded.to_vec())?;
            image::DynamicImage::ImageRgb8(rgb).into_rgba8()
        }
    } else {
        if animated(&path) {
            return None;
        }
        image::open(&path).ok()?.into_rgba8()
    };
    let size = decoded.dimensions();

    // Flatten over grey, so transparent pixels compare by what shows.
    let mut flat = RgbaImage::from_pixel(size.0, size.1, image::Rgba([128, 128, 128, 255]));
    imageops::overlay(&mut flat, &decoded, 0, 0);
    let pixels = imageops::resize(&flat, PRINT, PRINT, FilterType::Triangle);
    Some(Print { size, pixels })
}

/// How far apart two fingerprints are; `None` at different aspects.
fn distance(a: &Print, b: &Print) -> Option<f64> {
    let aspect = |(w, h): (u32, u32)| f64::from(w) / f64::from(h.max(1));
    if (aspect(a.size) - aspect(b.size)).abs() > 0.01 {
        return None;
    }
    let total: f64 = a
        .pixels
        .as_raw()
        .iter()
        .zip(b.pixels.as_raw())
        .map(|(x, y)| f64::from(x.abs_diff(*y)))
        .sum();
    Some(total / a.pixels.as_raw().len() as f64)
}

/// Why `image` is companion art in the wrong place, or `None` when it is
/// not companion art or may be where it is.
fn misplaced(
    image: &str,
    print: Option<&Print>,
    masters: &[(String, Print)],
    names: &[String],
) -> Option<String> {
    if archived(image) {
        return None;
    }

    // A master's scaled copy is a copy, even inside static/. Outside it, so
    // is a copy at full size.
    let outside = master_folder(image).is_none();
    let copy_of = print.and_then(|print| {
        masters
            .iter()
            .filter(|(master, other)| {
                let smaller = other.size.0 > print.size.0;
                let full = outside && other.size == print.size;
                master != image && (smaller || full)
            })
            .filter_map(|(master, other)| Some((master, distance(print, other)?)))
            .filter(|(_, distance)| *distance <= SAME_PICTURE)
            .min_by(|a, b| a.1.total_cmp(&b.1))
    });
    if let Some((master, _)) = copy_of {
        return Some(format!("{image}: a copy of {master}"));
    }
    if master_folder(image).is_some() {
        return None;
    }

    // Outside static/, a path that names a companion is companion art.
    let path = letters(image);
    let named = names.iter().find(|name| path.contains(name.as_str()))?;
    Some(format!("{image}: names {named}"))
}

/// Every image that breaks the rule, each with its reason.
fn violations() -> Vec<String> {
    let manifest = exports::read().unwrap();
    let listed = |image: &str| {
        manifest.exports.iter().any(|export| export.file == image)
            || manifest.retired.iter().any(|retired| retired.file == image)
    };
    let images = images();
    let names = companions();

    // Fingerprint the masters once.
    let masters: Vec<(String, Print)> = images
        .iter()
        .filter(|image| master_folder(image).is_some() && !archived(image))
        .filter_map(|image| Some((image.clone(), print(image)?)))
        .collect();

    images
        .iter()
        .filter(|image| !listed(image))
        .filter_map(|image| {
            let own = masters
                .iter()
                .find(|(master, _)| master == image)
                .map(|(_, print)| print);
            let decoded = if own.is_none() { print(image) } else { None };
            misplaced(image, own.or(decoded.as_ref()), &masters, &names)
        })
        .collect()
}

#[test]
fn companion_art_lives_in_static_or_is_exported() {
    let violations = violations();
    assert!(
        violations.is_empty(),
        "companion art outside static/<companion>/ must be exported from a master and \
         listed in {MANIFEST}, or retired there: {violations:#?}"
    );
}

#[test]
fn listed_files_exist() {
    // Companion art is never deleted, and exports are committed.
    let manifest = exports::read().unwrap();
    let files = manifest.exports.iter().map(|e| &e.file);
    for file in files.chain(manifest.retired.iter().map(|r| &r.file)) {
        assert!(root().join(file).is_file(), "{file}");
    }
    for export in &manifest.exports {
        assert!(master_folder(&export.master).is_some(), "{}", export.master);
        assert!(root().join(&export.master).is_file(), "{}", export.master);
    }
}

#[test]
fn nothing_reads_retired_files() {
    let manifest = exports::read().unwrap();
    let mut code = Vec::new();
    for folder in CODE {
        let path = root().join(folder);
        if path.is_file() {
            code.push(path);
        } else {
            files(&path, &mut code);
        }
    }
    let manifest_path = root().join(MANIFEST);
    for file in code.iter().filter(|file| **file != manifest_path) {
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };
        for retired in &manifest.retired {
            assert!(
                !text.contains(&retired.file),
                "{} reads retired {}",
                relative(file),
                retired.file
            );
        }
    }
}

#[test]
fn a_scaled_copy_is_found_anywhere() {
    let master = Print {
        size: (1254, 1254),
        pixels: RgbaImage::from_fn(PRINT, PRINT, |x, y| {
            image::Rgba([(x * 16) as u8, (y * 16) as u8, 90, 255])
        }),
    };
    let copy = Print {
        size: (512, 512),
        pixels: master.pixels.clone(),
    };
    let other = Print {
        size: (512, 512),
        pixels: RgbaImage::from_pixel(PRINT, PRINT, image::Rgba([250, 250, 250, 255])),
    };
    let masters = [("static/aki/face.png".to_string(), master)];
    let names = ["aki".to_string(), "cow".to_string()];

    // Copies of masters, anywhere; pictures named after a companion outside static/.
    for (image, print, expected) in [
        ("assets/cards/card.png", Some(&copy), true),
        ("static/aki/face-64.png", Some(&copy), true),
        ("static/archive/aki/face-64.png", Some(&copy), false),
        ("static/cow/archive/face-64.png", Some(&copy), false),
        ("docs/archive/aki-64.png", Some(&copy), true),
        ("assets/cow.png", Some(&other), true),
        ("static/aki/face.png", Some(&masters[0].1), false),
        ("static/aki/new_pose.png", Some(&other), false),
        ("docs/screenshot.png", Some(&other), false),
        ("docs/screenshot.gif", None, false),
    ] {
        assert_eq!(
            misplaced(image, print, &masters, &names).is_some(),
            expected,
            "{image}"
        );
    }
}

#[test]
fn an_animation_is_not_a_copy_of_its_frame() {
    // The Anki-tan pack's 240 px sprites are APNGs whose first frame matches
    // a frame master; an animation is a picture of its own, not a copy.
    let animation = "static/anki-tan/sprite/Anki-tan_hello_240px.png";
    let frame = "static/anki-tan/sprite/idle/anki-tan_idle_0024.png";
    assert!(print(animation).is_none(), "{animation}");
    assert!(print(frame).is_some(), "{frame}");
}
