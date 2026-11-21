//! Writes the Android app's companion drawables from the master PNGs in the
//! AnkiQuest-Art submodule (`art/`).
//!
//! Each drawable is a square lossy WebP with full-quality alpha, downscaled
//! with Lanczos to about three times its largest place in the app and never
//! upscaled. `--check` writes nothing and fails when a drawable is missing
//! or its bytes differ from what export would write. Exports are
//! deterministic on Linux, where CI runs the check.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ankiquest_art::{load, shrink, target};
use image::RgbaImage;

use crate::root;

mod addon;
mod banner;
mod coverage;
mod exports;
#[cfg(test)]
mod guard;

/// Drawable folder inside an AnkiQuest-Android checkout.
const DRAWABLES: &str = "ankiquest-ui/src/main/res/drawable-nodpi";

/// WebP quality for most drawables.
const QUALITY: f32 = 88.0;

/// WebP quality for the streak widget poses, which are shown large.
const WIDGET_QUALITY: f32 = 95.0;

/// Alpha plane quality; edges must stay clean on any background.
const ALPHA_QUALITY: i32 = 100;

/// Slowest, smallest libwebp encoding method.
const METHOD: i32 = 6;

/// Large and huge streak widgets show the pose full-width, up to about
/// 1100 px (AnkiQuest-Android#168), so smaller poses were upscaled.
const WIDGET: u32 = 1024;

/// Luminance below which a pixel counts as line art in [`Look::Outline`].
const LINE_LUMA: f32 = 90.0;

/// Alpha above which a pixel is part of the picture in [`Look::Outline`].
const LINE_ALPHA: u8 = 128;

/// How far [`Look::BoldOutline`] widens each line, in thousandths of the
/// master's width: 5 px on a 1254 px master. Thinner lines fade out when a
/// themed launcher icon is drawn at 48 dp.
const BOLD_SPREAD: u32 = 4;

/// How a drawable shows its master.
#[derive(Clone, Copy, PartialEq)]
enum Look {
    /// The master as drawn.
    Colour,
    /// Only the master's dark line art, in black on transparent: the
    /// monochrome layer of a themed launcher icon, which keeps alpha only.
    Outline,
    /// [`Look::Outline`] with every line widened by [`BOLD_SPREAD`], for
    /// masters drawn with thin lines.
    BoldOutline,
}

/// One drawable: name, master PNG relative to [`masters`], square size,
/// WebP quality and look.
struct Art {
    name: &'static str,
    source: &'static str,
    size: u32,
    quality: f32,
    look: Look,
}

/// A drawable at the default quality.
const fn art(name: &'static str, source: &'static str, size: u32) -> Art {
    Art {
        name,
        source,
        size,
        quality: QUALITY,
        look: Look::Colour,
    }
}

/// A themed icon's monochrome layer, from the master's line art.
const fn outline(name: &'static str, source: &'static str, size: u32) -> Art {
    Art {
        name,
        source,
        size,
        quality: QUALITY,
        look: Look::Outline,
    }
}

/// A themed icon's monochrome layer from a master with thin line art.
const fn bold_outline(name: &'static str, source: &'static str, size: u32) -> Art {
    Art {
        name,
        source,
        size,
        quality: QUALITY,
        look: Look::BoldOutline,
    }
}

/// A pose the big streak widget shows, for either companion.
const fn widget(name: &'static str, source: &'static str) -> Art {
    Art {
        name,
        source,
        size: WIDGET,
        quality: WIDGET_QUALITY,
        look: Look::Colour,
    }
}

const ART: &[Art] = &[
    widget("aki_face", "aki/face.png"),
    widget("aki_sad", "aki/sad.png"),
    widget("aki_hopeful", "aki/hopeful.png"),
    widget("aki_frozen", "aki/frozen.png"),
    widget("aki_neglected", "aki/neglected.png"),
    widget("ankilope_face", "ankilope/face.png"),
    widget("ankilope_sad", "ankilope/sad.png"),
    widget("ankilope_hopeful", "ankilope/hopeful.png"),
    widget("ankilope_frozen", "ankilope/frozen.png"),
    widget("ankilope_asleep", "ankilope/asleep.png"),
    widget("ankilope_neglected", "ankilope/neglected.png"),
    widget("bluma_face", "cow/face.png"),
    widget("bluma_asleep", "cow/asleep.png"),
    widget("bluma_happy", "cow/happy.png"),
    widget("bluma_streak", "cow/streak.png"),
    widget("bluma_sad", "cow/sad.png"),
    widget("bluma_hopeful", "cow/hopeful.png"),
    widget("bluma_frozen", "cow/frozen.png"),
    widget("bluma_neglected", "cow/neglected.png"),
    widget("aki_celebrate", "aki/happy.png"),
    widget("aki_streak", "aki/streak.png"),
    widget("ankilope_celebrate", "ankilope/happy.png"),
    widget("ankilope_study", "ankilope/study.png"),
    widget("aki_welcome", "aki/welcome.png"),
    widget("aki_review", "aki/review.png"),
    widget("aki_freeze", "aki/freeze.png"),
    widget("ankilope_welcome", "ankilope/welcome.png"),
    art("aki_winner", "aki/winner.png", 168),
    // Moment art from #88: app icon moods, streak widget, celebrations.
    art("aki_icon_happy", "aki/icon_happy.png", 432),
    art("aki_icon_worried", "aki/icon_worried.png", 432),
    art("aki_icon_frozen", "aki/icon_frozen.png", 432),
    art("aki_icon_sleepy", "aki/icon_sleepy.png", 432),
    widget("aki_streak_hot", "aki/streak_hot.png"),
    widget("aki_streak_blazing", "aki/streak_blazing.png"),
    widget("aki_streak_legend", "aki/streak_legend.png"),
    widget("aki_streak_milestone", "aki/streak_milestone.png"),
    widget("aki_freeze_used", "aki/freeze_used.png"),
    widget("aki_comeback", "aki/comeback.png"),
    widget("aki_minimum", "aki/minimum.png"),
    widget("aki_goal", "aki/goal.png"),
    art("aki_achievement", "aki/achievement.png", 336),
    art("aki_level_up", "aki/level_up.png", 336),
    art("aki_deck_conquered", "aki/deck_conquered.png", 336),
    art("aki_leech", "aki/leech_defeated.png", 336),
    widget("aki_asleep", "aki/asleep.png"),
    widget("aki_at_risk", "aki/at_risk.png"),
    art("aki_backlog", "aki/backlog.png", 336),
    art("ankilope_backlog", "ankilope/backlog.png", 336),
    art("aki_place_1", "aki/place_1.png", 336),
    art("aki_place_2", "aki/place_2.png", 336),
    art("aki_place_3", "aki/place_3.png", 336),
    art("ankilope_place_1", "ankilope/place_1.png", 336),
    art("ankilope_place_2", "ankilope/place_2.png", 336),
    art("ankilope_place_3", "ankilope/place_3.png", 336),
    art("aki_nudge", "aki/nudge.png", 336),
    art("aki_challenge", "aki/challenge.png", 336),
    art("aki_challenge_won", "aki/challenge_won.png", 336),
    art("aki_challenge_lost", "aki/challenge_lost.png", 336),
    art("ankilope_nudge", "ankilope/nudge.png", 336),
    art("ankilope_challenge", "ankilope/challenge.png", 336),
    art("ankilope_challenge_won", "ankilope/challenge_won.png", 336),
    art(
        "ankilope_challenge_lost",
        "ankilope/challenge_lost.png",
        336,
    ),
    art("duo_friends", "duo/friends.png", 336),
    widget("anki_tan_frozen", "anki-tan/frozen.png"),
    widget("anki_tan_asleep", "anki-tan/asleep.png"),
    widget("anki_tan_sad", "anki-tan/sad.png"),
    widget("anki_tan_neglected", "anki-tan/neglected.png"),
    widget("anki_tan_chibi_frozen", "anki-tan/chibi/frozen.png"),
    // Anki-tan's other poses, from Shigeyuki's pack (art/anki-tan/README.md).
    // The pack's pictures are smaller than WIDGET and are kept at their size.
    widget("anki_tan_face", "anki-tan/face.png"),
    widget("anki_tan_happy", "anki-tan/happy.png"),
    widget("anki_tan_streak", "anki-tan/streak.png"),
    widget("anki_tan_hopeful", "anki-tan/hopeful.png"),
    widget("anki_tan_welcome", "anki-tan/welcome.png"),
    widget("anki_tan_review", "anki-tan/review.png"),
    widget("anki_tan_streak_legend", "anki-tan/streak_legend.png"),
    widget("anki_tan_streak_milestone", "anki-tan/streak_milestone.png"),
    widget("anki_tan_hurt", "anki-tan/hurt.png"),
    widget("anki_tan_dead", "anki-tan/dead.png"),
    // Anki-tan's launcher icons (AnkiQuest-Android AnkiquestIcon), CC BY-SA 4.0.
    // The pack's icon is 256 px and is kept at that size. She has no mood
    // icon masters; the moods show her streak pose and the chibi's sad and
    // asleep poses, and the monochrome layer the chibi's face.
    art("anki_tan_icon", "anki-tan/icon.png", 432),
    art("anki_tan_icon_happy", "anki-tan/streak.png", 432),
    art("anki_tan_icon_worried", "anki-tan/chibi/sad.png", 432),
    art("anki_tan_icon_sleepy", "anki-tan/chibi/asleep.png", 432),
    outline("anki_tan_icon_mono", "anki-tan/chibi/face.png", 432),
    // Ankilope and Bluma have no mood faces yet; the app shows the face for every mood.
    art("ankilope_icon", "ankilope/face.png", 432),
    bold_outline("ankilope_icon_mono", "ankilope/face.png", 432),
    art("cow_icon", "cow/face.png", 432),
    bold_outline("cow_icon_mono", "cow/face.png", 432),
];

/// Whether to write drawables or only compare them with a fresh export.
#[derive(PartialEq)]
enum Mode {
    Export,
    Check,
}

/// The folder that holds the masters (`ankiquest_art::MASTERS`).
pub(crate) fn masters() -> PathBuf {
    root().join(ankiquest_art::MASTERS)
}

/// Runs the exporter: `[--check] <android-checkout>`.
pub fn run(args: &[String]) -> ExitCode {
    let mut mode = Mode::Export;
    let mut android = None;
    for arg in args {
        match arg.as_str() {
            "--check" => mode = Mode::Check,
            _ if arg.starts_with('-') || android.is_some() => return usage(),
            _ => android = Some(PathBuf::from(arg)),
        }
    }
    let Some(android) = android else {
        return usage();
    };

    let all: Vec<&Art> = ART.iter().collect();
    let result = match mode {
        Mode::Export => export(&android, &all),
        Mode::Check => check(&android, &all),
    };
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// Runs the add-on pose exporter: `[--check] <desktop-checkout>`.
pub fn addon(args: &[String]) -> ExitCode {
    addon::run(args)
}

/// Runs the art coverage report.
pub fn coverage(args: &[String]) -> ExitCode {
    coverage::run(args)
}

/// Runs the banner command: `[--social] [--out <file>]`.
pub fn banner(args: &[String]) -> ExitCode {
    banner::run(args)
}

/// Prints the usage text and fails.
fn usage() -> ExitCode {
    eprintln!("usage: xtask export-android-art [--check] <android-checkout>");
    ExitCode::FAILURE
}

/// The drawable's path relative to an Android checkout.
fn drawable(name: &str) -> String {
    format!("{DRAWABLES}/{name}.webp")
}

/// What export writes for `art`: its pixel size and WebP bytes.
fn render(art: &Art) -> Result<((u32, u32), Vec<u8>), String> {
    let master = look(load(&masters().join(art.source))?, art.look);
    let (width, height) = target(master.width(), master.height(), art.size);
    if (width, height) != (art.size, art.size) {
        println!(
            "{}: {} is {}x{}, kept at {width}x{height}",
            art.name,
            art.source,
            master.width(),
            master.height()
        );
    }

    // Downscale and encode.
    let image = shrink(master, width, height);
    Ok(((width, height), encode(&image, art.quality)?))
}

/// Writes `arts` into the checkout.
fn export(android: &Path, arts: &[&Art]) -> Result<ExitCode, String> {
    let folder = android.join(DRAWABLES);
    fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    for art in arts {
        let ((width, height), bytes) = render(art)?;
        let name = drawable(art.name);
        let path = android.join(&name);
        fs::write(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("{name}: {width}x{height}, {} bytes", bytes.len());
    }
    Ok(ExitCode::SUCCESS)
}

/// Compares `arts` in the checkout with a fresh export, byte for byte.
fn check(android: &Path, arts: &[&Art]) -> Result<ExitCode, String> {
    let failed = stale(android, arts)?;
    for line in &failed {
        println!("{line}");
    }
    if !failed.is_empty() {
        return Ok(ExitCode::FAILURE);
    }
    println!("all {} drawables match", arts.len());
    Ok(ExitCode::SUCCESS)
}

/// One line for each drawable that is missing or not what export writes.
///
/// A size check alone passes an edited or unrelated file of the right size
/// (AnkiQuest-Android#314), so the bytes must match.
fn stale(android: &Path, arts: &[&Art]) -> Result<Vec<String>, String> {
    let mut failed = Vec::new();
    for art in arts {
        let name = drawable(art.name);
        let Ok(committed) = fs::read(android.join(&name)) else {
            failed.push(format!("missing: {name}"));
            continue;
        };
        let (_, fresh) = render(art)?;
        if committed != fresh {
            failed.push(format!(
                "{name}: not what export writes ({} bytes, export writes {})",
                committed.len(),
                fresh.len()
            ));
        }
    }
    Ok(failed)
}

/// The master as [`Look`] asks for it.
fn look(master: RgbaImage, look: Look) -> RgbaImage {
    if look == Look::Colour {
        return master;
    }

    // Keep opaque dark pixels as black line art; drop everything else.
    let mut lines = master;
    for pixel in lines.pixels_mut() {
        let [r, g, b, a] = pixel.0;
        let luma = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
        let line = a > LINE_ALPHA && luma < LINE_LUMA;
        pixel.0 = if line { [0, 0, 0, 255] } else { [0, 0, 0, 0] };
    }
    if look == Look::Outline {
        return lines;
    }

    // Widen the lines so they survive the downscale to launcher size.
    let spread = (lines.width() * BOLD_SPREAD).div_ceil(1000);
    let wide = widen(&lines, spread, Axis::Across);
    widen(&wide, spread, Axis::Down)
}

/// Direction of one [`widen`] pass.
#[derive(Clone, Copy, PartialEq)]
enum Axis {
    Across,
    Down,
}

/// Makes every pixel opaque that has an opaque pixel within `spread`
/// along `axis`; two passes widen lines by a square of that radius.
fn widen(lines: &RgbaImage, spread: u32, axis: Axis) -> RgbaImage {
    let (width, height) = lines.dimensions();
    RgbaImage::from_fn(width, height, |x, y| {
        let (at, end) = if axis == Axis::Across {
            (x, width)
        } else {
            (y, height)
        };
        let from = at.saturating_sub(spread);
        let to = (at + spread).min(end - 1);
        let hit = (from..=to).any(|i| {
            let (px, py) = if axis == Axis::Across { (i, y) } else { (x, i) };
            lines.get_pixel(px, py)[3] > 0
        });
        if hit {
            image::Rgba([0, 0, 0, 255])
        } else {
            image::Rgba([0, 0, 0, 0])
        }
    })
}

/// Encodes lossy WebP with full-quality alpha.
fn encode(image: &RgbaImage, quality: f32) -> Result<Vec<u8>, String> {
    let mut config = webp::WebPConfig::new().map_err(|()| "libwebp config failed")?;
    config.lossless = 0;
    config.quality = quality;
    config.alpha_quality = ALPHA_QUALITY;
    config.method = METHOD;
    let encoder = webp::Encoder::from_rgba(image, image.width(), image.height());
    let memory = encoder
        .encode_advanced(&config)
        .map_err(|e| format!("WebP encoding failed: {e:?}"))?;
    Ok(memory.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widget_poses_are_large() {
        // Large widget poses, including core art awaiting companion selection wiring.
        let poses = [
            "aki_face",
            "aki_sad",
            "aki_hopeful",
            "aki_frozen",
            "aki_neglected",
            "ankilope_face",
            "ankilope_sad",
            "ankilope_hopeful",
            "ankilope_frozen",
            "ankilope_asleep",
            "ankilope_neglected",
            "bluma_face",
            "bluma_asleep",
            "bluma_happy",
            "bluma_streak",
            "bluma_sad",
            "bluma_hopeful",
            "bluma_frozen",
            "bluma_neglected",
            "aki_celebrate",
            "aki_streak",
            "ankilope_celebrate",
            "ankilope_study",
            "aki_welcome",
            "aki_review",
            "aki_freeze",
            "ankilope_welcome",
            "aki_streak_hot",
            "aki_streak_blazing",
            "aki_streak_legend",
            "aki_streak_milestone",
            "aki_freeze_used",
            "aki_comeback",
            "aki_minimum",
            "aki_goal",
            "aki_asleep",
            "aki_at_risk",
            "anki_tan_frozen",
            "anki_tan_asleep",
            "anki_tan_sad",
            "anki_tan_neglected",
            "anki_tan_chibi_frozen",
            "anki_tan_face",
            "anki_tan_happy",
            "anki_tan_streak",
            "anki_tan_hopeful",
            "anki_tan_welcome",
            "anki_tan_review",
            "anki_tan_streak_legend",
            "anki_tan_streak_milestone",
            "anki_tan_hurt",
            "anki_tan_dead",
        ];
        let large: Vec<_> = ART.iter().filter(|art| art.size == WIDGET).collect();
        let names: Vec<_> = large.iter().map(|art| art.name).collect();
        assert_eq!(poses.to_vec(), names);
        assert!(large.iter().all(|art| art.quality == WIDGET_QUALITY));
    }

    #[test]
    fn asleep_has_one_name() {
        // #32: every companion's asleep export and master use the pose name.
        let asleep: Vec<_> = ART
            .iter()
            .filter(|art| art.name.contains("sleep") && art.size == WIDGET)
            .collect();
        assert_eq!(4, asleep.len(), "aki, ankilope, bluma, anki_tan");
        for art in asleep {
            assert!(art.name.ends_with("_asleep"), "{}", art.name);
            assert!(art.source.ends_with("/asleep.png"), "{}", art.source);
        }
    }

    #[test]
    fn names_are_unique() {
        let mut names: Vec<_> = ART.iter().map(|art| art.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(ART.len(), names.len());
    }

    #[test]
    fn masters_exist() {
        // Every source is a master AnkiQuest-Art's manifest lists.
        let manifest = ankiquest_art::Manifest::read(&masters()).unwrap();
        for art in ART {
            assert!(masters().join(art.source).is_file(), "{}", art.source);
            let listed = manifest.files().any(|file| file == art.source);
            assert!(listed, "{}", art.source);
        }
    }

    #[test]
    fn nothing_is_exported_from_an_archive() {
        // An archive folder holds pictures that are not masters (AGENTS.md, Companion art).
        for art in ART {
            assert!(!ankiquest_art::archived(art.source), "{}", art.source);
        }
    }

    #[test]
    fn encodes_lossy_webp() {
        let image = RgbaImage::from_pixel(64, 64, image::Rgba([200, 40, 40, 128]));
        let bytes = encode(&shrink(image, 32, 32), QUALITY).unwrap();
        let features = webp::BitstreamFeatures::new(&bytes).unwrap();
        assert_eq!((32, 32), (features.width(), features.height()));
        assert!(features.has_alpha());
        assert!(matches!(
            features.format(),
            Some(webp::BitstreamFormat::Lossy)
        ));
    }

    #[test]
    fn check_compares_bytes() {
        // The smallest drawable keeps the test fast.
        let art = ART.iter().min_by_key(|art| art.size).unwrap();
        let android = tempfile::tempdir().unwrap();
        assert_eq!(ExitCode::SUCCESS, export(android.path(), &[art]).unwrap());
        assert!(stale(android.path(), &[art]).unwrap().is_empty());

        // Same size, other bytes: an edited file must fail.
        let path = android.path().join(drawable(art.name));
        let mut bytes = fs::read(&path).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 0xff;
        fs::write(&path, &bytes).unwrap();
        let failed = stale(android.path(), &[art]).unwrap();
        assert_eq!(1, failed.len());
        assert!(failed[0].contains("not what export writes"), "{failed:?}");

        fs::remove_file(&path).unwrap();
        let failed = stale(android.path(), &[art]).unwrap();
        assert_eq!(vec![format!("missing: {}", drawable(art.name))], failed);
    }

    #[test]
    fn outline_keeps_only_dark_lines() {
        let mut image = RgbaImage::new(3, 1);
        image.put_pixel(0, 0, image::Rgba([10, 10, 10, 255]));
        image.put_pixel(1, 0, image::Rgba([250, 200, 180, 255]));
        image.put_pixel(2, 0, image::Rgba([10, 10, 10, 40]));
        let lines = look(image, Look::Outline);
        assert_eq!([0, 0, 0, 255], lines.get_pixel(0, 0).0, "black line");
        assert_eq!(0, lines.get_pixel(1, 0)[3], "skin");
        assert_eq!(0, lines.get_pixel(2, 0)[3], "faint dark pixel");
    }

    #[test]
    fn bold_outline_widens_lines() {
        // One dark pixel in the middle of a 1000 px row spreads 4 px each way.
        let mut image = RgbaImage::from_pixel(1000, 9, image::Rgba([250, 200, 180, 255]));
        image.put_pixel(500, 4, image::Rgba([10, 10, 10, 255]));
        let lines = look(image, Look::BoldOutline);
        assert_eq!(255, lines.get_pixel(504, 8)[3], "within the spread");
        assert_eq!(0, lines.get_pixel(505, 4)[3], "beyond the spread");
        assert_eq!(0, lines.get_pixel(0, 0)[3], "skin");
    }

    #[test]
    fn every_companion_has_launcher_icons() {
        // AnkiQuest-Android AnkiquestIcon shows these; a missing one fell back to Aki (#313 there).
        for name in [
            "aki_icon_happy",
            "anki_tan_icon",
            "anki_tan_icon_mono",
            "ankilope_icon",
            "ankilope_icon_mono",
            "cow_icon",
            "cow_icon_mono",
        ] {
            let art = ART.iter().find(|art| art.name == name);
            assert!(art.is_some_and(|art| art.size == 432), "{name}");
        }
    }
}
