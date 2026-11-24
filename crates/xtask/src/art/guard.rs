//! Fails when companion art is checked in to this repository (AGENTS.md,
//! Companion art): every master, archived picture and record lives in the
//! AnkiQuest-Art submodule (`art/`), and builds generate every copy.
//!
//! An image outside the staged release is companion art when its path names a
//! companion, or when it looks like a master at any size. Animations are not
//! compared with masters.

use std::fs;
use std::path::{Path, PathBuf};

use image::RgbaImage;
use image::imageops::{self, FilterType};

use super::root;

/// Image formats companion art comes in.
const FORMATS: [&str; 3] = ["png", "webp", "gif"];

/// Folders never searched: build output, dependencies, version control,
/// and the staged art release, where companion art belongs.
const SKIPPED: [&str; 4] = ["target", "node_modules", ".git", ankiquest_art::MASTERS];

/// The fixture that names every companion but Aki.
const UNLOCKS: &str = "tests/fixtures/companion_unlock.json";

/// Fingerprint edge: images are compared at this size.
const PRINT: u32 = 16;

/// Largest mean channel difference, out of 255, between the fingerprints of
/// a master and a scaled copy of it. Copies measure up to 1.2 (a 32 px
/// icon), the closest two different masters 1.9 (Ankilope's places 2 and 3).
const SAME_PICTURE: f64 = 1.5;

/// An image's path relative to the repository root, with `/` separators.
fn relative(path: &Path) -> String {
    let path = path.strip_prefix(root()).unwrap_or(path);
    path.to_string_lossy().replace('\\', "/")
}

/// Every file under `folder`, skipping [`SKIPPED`] folders at the root and
/// hidden folders anywhere.
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
        let top = folder == root();
        if (top && SKIPPED.contains(&name.as_str())) || name.starts_with('.') {
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
/// one with masters in AnkiQuest-Art.
fn companions(manifest: &ankiquest_art::Manifest) -> Vec<String> {
    let text = fs::read_to_string(root().join(UNLOCKS)).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    let mut names = vec!["aki".to_string()];
    for companion in value["companions"].as_array().unwrap() {
        names.push(companion["companion"].as_str().unwrap().to_string());
    }
    names.extend(manifest.masters.iter().map(|m| m.companion.clone()));
    let mut names: Vec<String> = names.iter().map(|name| letters(name)).collect();
    names.sort();
    names.dedup();
    names
}

/// The folder that holds the masters (`ankiquest_art::MASTERS`).
fn masters() -> PathBuf {
    root().join(ankiquest_art::MASTERS)
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

/// Why `image`, outside the staged release, is companion art, or `None` when it
/// is not.
fn misplaced(
    image: &str,
    print: Option<&Print>,
    masters: &[(String, Print)],
    names: &[String],
) -> Option<String> {
    // A master at any size, smaller or the same, is a copy.
    let copy_of = print.and_then(|print| {
        masters
            .iter()
            .filter(|(_, other)| other.size.0 >= print.size.0)
            .filter_map(|(master, other)| Some((master, distance(print, other)?)))
            .filter(|(_, distance)| *distance <= SAME_PICTURE)
            .min_by(|a, b| a.1.total_cmp(&b.1))
    });
    if let Some((master, _)) = copy_of {
        return Some(format!("{image}: a copy of {master}"));
    }

    // A path that names a companion is companion art.
    let path = letters(image);
    let named = names.iter().find(|name| path.contains(name.as_str()))?;
    Some(format!("{image}: names {named}"))
}

/// Every image that breaks the rule, each with its reason.
fn violations() -> Vec<String> {
    let manifest = ankiquest_art::Manifest::read(&masters()).unwrap();
    let names = companions(&manifest);

    // Fingerprint the masters once.
    let masters: Vec<(String, Print)> = manifest
        .files()
        .filter_map(|file| {
            let path = format!("{}/{file}", ankiquest_art::MASTERS);
            Some((path.clone(), print(&path)?))
        })
        .collect();
    images()
        .iter()
        .filter_map(|image| misplaced(image, print(image).as_ref(), &masters, &names))
        .collect()
}

#[test]
fn companion_art_lives_in_the_staged_release() {
    let violations = violations();
    assert!(
        violations.is_empty(),
        "companion art belongs in AnkiQuest-Art ({}/), and builds generate its \
         copies: {violations:#?}",
        ankiquest_art::MASTERS
    );
}

#[test]
fn the_release_is_staged() {
    // Without it every check here would pass on an empty folder.
    let manifest = ankiquest_art::Manifest::read(&masters()).unwrap();
    assert!(!manifest.masters.is_empty());
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
    let masters = [("art/aki/face.png".to_string(), master)];
    let names = ["aki".to_string(), "cow".to_string()];

    // Copies of masters at any size, and pictures named after a companion.
    for (image, print, expected) in [
        ("assets/cards/card.png", Some(&copy), true),
        ("static/face-64.png", Some(&copy), true),
        ("docs/archive/aki-64.png", Some(&copy), true),
        ("static/face.png", Some(&masters[0].1), true),
        ("assets/cow.png", Some(&other), true),
        ("static/new_pose.png", Some(&other), false),
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
    let animation = "art/archive/anki-tan-pack/sprite/Anki-tan_hello_240px.png";
    let frame = "art/archive/anki-tan-pack/sprite/idle/anki-tan_idle_0024.png";
    assert!(print(animation).is_none(), "{animation}");
    assert!(print(frame).is_some(), "{frame}");
}
