//! Companion packs players upload (`docs/companion-packs.md`). [`canonical`]
//! checks a pack zip under the app's rules (AnkiQuest-Art `docs/packs.md`)
//! and builds it again from decoded pixels, so nothing a player wrote but
//! the checked fields and the pictures' pixels reaches their friends.

use ankiquest_art::{Kind, Poses};
use image::{ImageError, ImageFormat, ImageReader, Limits, RgbaImage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Cursor, Read};

mod zip_out;

pub use zip_out::stored;

/// Largest pack zip a player may upload.
pub const MAX_ARCHIVE: usize = 32 * 1024 * 1024;

/// Most entries in a pack.
pub const MAX_ENTRIES: usize = 128;

/// Largest entry once inflated.
pub const MAX_ENTRY: u64 = 8 * 1024 * 1024;

/// Most bytes inflated from all entries together.
pub const MAX_TOTAL: u64 = 64 * 1024 * 1024;

/// Largest `companion.json`.
pub const MAX_PACK_INDEX: u64 = 64 * 1024;

/// Largest picture edge in an upload, in pixels.
pub const MAX_EDGE: u32 = 2048;

/// Most memory a picture may decode into.
const MAX_DECODED: u64 = 48 * 1024 * 1024;

/// Largest canonical pack.
pub const MAX_CANONICAL: usize = 16 * 1024 * 1024;

/// The pack format this server reads and writes.
pub const FORMAT: u64 = 1;

/// The index inside a pack.
pub const PACK_INDEX: &str = "companion.json";

/// Largest edge of a pose or moment picture in a canonical pack, as in Art's packs.
const POSE_SIZE: u32 = 1024;

/// Largest edge of a picture only launcher icons use.
const ICON_SIZE: u32 = 432;

/// WebP quality of every canonical picture, as in Art's packs.
const QUALITY: f32 = 88.0;

/// Alpha is kept at full quality, as in Art's packs.
const ALPHA_QUALITY: i32 = 100;

/// libwebp's speed and size trade-off: 4 is its default.
const METHOD: i32 = 4;

/// Longest companion name, in characters.
const MAX_NAME: usize = 40;

/// Longest licence expression.
const MAX_LICENCE: usize = 64;

/// Longest credit, in characters.
const MAX_CREDIT: usize = 200;

/// Longest licence notice, in characters.
const MAX_NOTICE: usize = 16 * 1024;

/// Longest id: a letter and up to 31 more characters.
const MAX_ID: usize = 32;

/// Longest picture file name without its extension.
const MAX_STEM: usize = 64;

/// Largest version, as the apps read it (a signed 32-bit integer).
const MAX_VERSION: u64 = i32::MAX as u64;

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Why a pack was refused, as `reason` in the API names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    TooLarge,
    TooManyEntries,
    BadEntryName,
    DuplicateEntry,
    NotAPicture,
    PictureTooLarge,
    Animated,
    BadIndex,
    NewerFormat,
    BadId,
    ReservedId,
    UnknownPose,
    MissingCorePose,
    MissingFile,
    UnusedFile,
    BadLicence,
    BadCredit,
}

impl Refusal {
    /// The name the API answers.
    pub fn reason(self) -> &'static str {
        match self {
            Refusal::TooLarge => "too_large",
            Refusal::TooManyEntries => "too_many_entries",
            Refusal::BadEntryName => "bad_entry_name",
            Refusal::DuplicateEntry => "duplicate_entry",
            Refusal::NotAPicture => "not_a_picture",
            Refusal::PictureTooLarge => "picture_too_large",
            Refusal::Animated => "animated",
            Refusal::BadIndex => "bad_index",
            Refusal::NewerFormat => "newer_format",
            Refusal::BadId => "bad_id",
            Refusal::ReservedId => "reserved_id",
            Refusal::UnknownPose => "unknown_pose",
            Refusal::MissingCorePose => "missing_core_pose",
            Refusal::MissingFile => "missing_file",
            Refusal::UnusedFile => "unused_file",
            Refusal::BadLicence => "bad_licence",
            Refusal::BadCredit => "bad_credit",
        }
    }
}

/// A refused pack: why, and what in it.
#[derive(Debug, PartialEq, Eq)]
pub struct Refused {
    pub reason: Refusal,
    pub detail: String,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.reason.reason(), self.detail)
    }
}

fn refuse<T>(reason: Refusal, detail: impl Into<String>) -> Result<T, Refused> {
    Err(Refused {
        reason,
        detail: detail.into(),
    })
}

/// The checked fields of a pack's `companion.json`, as the canonical pack has them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Info {
    pub format: u64,
    pub id: String,
    pub name: String,
    pub version: u64,
    pub licence: String,
    pub credit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
    pub poses: BTreeMap<String, String>,
}

/// A pack as the server stores and serves it.
#[derive(Debug)]
pub struct Canonical {
    pub info: Info,
    pub zip: Vec<u8>,
    pub sha256: String,
}

/// `companion.json` as written; every field is checked before use.
#[derive(Deserialize)]
struct Written {
    format: Option<serde_json::Value>,
    id: Option<String>,
    name: Option<String>,
    version: Option<serde_json::Value>,
    licence: Option<String>,
    credit: Option<String>,
    notice: Option<String>,
    poses: Option<BTreeMap<String, String>>,
}

/// Lower-case hex SHA-256 of `bytes`.
pub fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Whether `id` is a companion id: a lower-case letter, then up to 31 lower-case letters,
/// digits or underscores.
pub fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    let first = chars.next().is_some_and(|c| c.is_ascii_lowercase());
    first
        && id.len() <= MAX_ID
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Checks the pack zip `bytes` against the pose list `poses`, refusing an `id` that
/// `reserved` accepts, and builds the canonical pack.
pub fn canonical(
    bytes: &[u8],
    poses: &Poses,
    reserved: impl Fn(&str) -> bool,
) -> Result<Canonical, Refused> {
    if bytes.len() > MAX_ARCHIVE {
        return refuse(Refusal::TooLarge, format!("more than {MAX_ARCHIVE} bytes"));
    }
    let mut entries = entries(bytes)?;
    let Some(index) = entries.remove(PACK_INDEX) else {
        return refuse(Refusal::MissingFile, PACK_INDEX);
    };
    let info = info(&index, poses, &reserved)?;

    // Every named file is there, and nothing else is.
    if let Some(file) = info
        .poses
        .values()
        .find(|file| !entries.contains_key(*file))
    {
        return refuse(Refusal::MissingFile, file.clone());
    }
    if let Some(file) = entries
        .keys()
        .find(|file| !info.poses.values().any(|f| f == *file))
    {
        return refuse(Refusal::UnusedFile, file.clone());
    }

    // Each picture once, decoded and encoded again under its WebP name.
    let mut files = BTreeMap::new();
    let mut renamed = BTreeMap::new();
    for (file, bytes) in &entries {
        let out = webp_name(file);
        if files.contains_key(&out) {
            return refuse(Refusal::DuplicateEntry, out);
        }
        let size = if icon_only(&info, file, poses) {
            ICON_SIZE
        } else {
            POSE_SIZE
        };
        files.insert(out.clone(), redraw(file, bytes, size)?);
        renamed.insert(file.clone(), out);
    }
    let info = Info {
        poses: info
            .poses
            .iter()
            .map(|(name, file)| (name.clone(), renamed[file].clone()))
            .collect(),
        ..info
    };

    // The index last, then the zip and its hash.
    let mut index = serde_json::to_string_pretty(&info).map_err(|error| Refused {
        reason: Refusal::BadIndex,
        detail: error.to_string(),
    })?;
    index.push('\n');
    files.insert(PACK_INDEX.to_string(), index.into_bytes());
    let zip = zip_out::stored(&files);
    if zip.len() > MAX_CANONICAL {
        return refuse(
            Refusal::TooLarge,
            format!("{} bytes once rebuilt", zip.len()),
        );
    }
    let sha256 = sha256(&zip);
    Ok(Canonical { info, zip, sha256 })
}

/// Every entry of the zip, by name, within the limits, counting the bytes actually inflated.
fn entries(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, Refused> {
    let mut archive = match zip::ZipArchive::new(Cursor::new(bytes)) {
        Ok(archive) => archive,
        Err(error) => return refuse(Refusal::BadEntryName, format!("not a zip: {error}")),
    };
    if archive.len() > MAX_ENTRIES {
        return refuse(Refusal::TooManyEntries, format!("more than {MAX_ENTRIES}"));
    }

    // The reader keeps one entry per name; the directory's own count shows a name twice.
    if declared_entries(bytes) != Some(archive.len()) {
        return refuse(Refusal::DuplicateEntry, "the directory lists a name twice");
    }

    let mut entries = BTreeMap::new();
    let mut total = 0;
    for at in 0..archive.len() {
        let entry = match archive.by_index(at) {
            Ok(entry) => entry,
            Err(error) => return refuse(Refusal::BadEntryName, error.to_string()),
        };

        // Flat names of known kinds only, each once.
        let name = entry.name().to_string();
        if entry.is_dir() || !entry_name(&name) {
            return refuse(Refusal::BadEntryName, name);
        }
        if entries.contains_key(&name) {
            return refuse(Refusal::DuplicateEntry, name);
        }

        // Read one byte past the limit, so a lying header is caught.
        let limit = if name == PACK_INDEX {
            MAX_PACK_INDEX
        } else {
            MAX_ENTRY
        }
        .min(MAX_TOTAL - total);
        let mut data = Vec::new();
        if let Err(error) = entry.take(limit + 1).read_to_end(&mut data) {
            return refuse(Refusal::BadEntryName, format!("{name}: {error}"));
        }
        if data.len() as u64 > limit {
            return refuse(Refusal::TooLarge, name);
        }
        total += data.len() as u64;
        entries.insert(name, data);
    }
    Ok(entries)
}

/// The entry count the end of central directory record states. `None` without one, or
/// with the Zip64 marker: a pack never needs Zip64.
fn declared_entries(bytes: &[u8]) -> Option<usize> {
    const END: &[u8] = b"PK\x05\x06";
    const END_SIZE: usize = 22;
    const ZIP64: u16 = u16::MAX;
    let at = bytes.windows(END.len()).rposition(|window| window == END)?;
    let record = bytes.get(at..at + END_SIZE)?;
    let count = u16::from_le_bytes([record[10], record[11]]);
    (count != ZIP64).then_some(usize::from(count))
}

/// `companion.json` or a picture name: lower-case letters, digits and underscores, then
/// `.png` or `.webp`.
fn entry_name(name: &str) -> bool {
    if name == PACK_INDEX {
        return true;
    }
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return false;
    };
    let plain = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_';
    matches!(extension, "png" | "webp")
        && !stem.is_empty()
        && stem.len() <= MAX_STEM
        && stem.chars().all(plain)
}

/// `file` with the `.webp` extension.
fn webp_name(file: &str) -> String {
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    format!("{stem}.webp")
}

/// Whether only launcher icons show `file`.
fn icon_only(info: &Info, file: &str, poses: &Poses) -> bool {
    info.poses
        .iter()
        .filter(|(_, f)| *f == file)
        .all(|(name, _)| poses.kind(name) == Some(Kind::Icon))
}

/// The checked fields of `index`.
fn info(index: &[u8], poses: &Poses, reserved: &impl Fn(&str) -> bool) -> Result<Info, Refused> {
    let bad = |detail: &str| refuse(Refusal::BadIndex, detail);
    let Ok(text) = std::str::from_utf8(index) else {
        return bad("not UTF-8");
    };
    let written: Written = match serde_json::from_str(text) {
        Ok(written) => written,
        Err(error) => return bad(&error.to_string()),
    };

    // The format, then the id.
    let format = positive(written.format.as_ref()).ok_or(Refused {
        reason: Refusal::BadIndex,
        detail: "no format".into(),
    })?;
    if format > FORMAT {
        return refuse(Refusal::NewerFormat, format!("format {format}"));
    }
    let Some(id) = written.id else {
        return bad("no id");
    };
    if !valid_id(&id) {
        return refuse(Refusal::BadId, id);
    }
    if reserved(&id) {
        return refuse(Refusal::ReservedId, id);
    }

    // The words friends will read.
    let Some(name) = written.name.filter(|name| line(name, MAX_NAME)) else {
        return bad("name");
    };
    let Some(version) = positive(written.version.as_ref()).filter(|v| *v <= MAX_VERSION) else {
        return bad("version is not a positive number");
    };
    let Some(licence) = written.licence.filter(|licence| spdx(licence)) else {
        return refuse(Refusal::BadLicence, "licence");
    };
    let Some(credit) = written.credit.filter(|credit| line(credit, MAX_CREDIT)) else {
        return refuse(Refusal::BadCredit, "credit");
    };
    if let Some(notice) = &written.notice {
        if notice.chars().count() > MAX_NOTICE {
            return refuse(Refusal::TooLarge, "notice");
        }
        if notice
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        {
            return bad("notice");
        }
    }

    // Only known names, and every core pose.
    let Some(map) = written.poses else {
        return bad("no poses");
    };
    if let Some(name) = map.keys().find(|name| poses.kind(name).is_none()) {
        return refuse(Refusal::UnknownPose, name.clone());
    }
    if let Some(core) = poses
        .of(Kind::CoreMood)
        .into_iter()
        .find(|core| !map.contains_key(*core))
    {
        return refuse(Refusal::MissingCorePose, core);
    }
    if let Some(file) = map
        .values()
        .find(|file| *file == PACK_INDEX || !entry_name(file))
    {
        return refuse(Refusal::BadEntryName, file.clone());
    }
    Ok(Info {
        format: FORMAT,
        id,
        name,
        version,
        licence,
        credit,
        notice: written.notice,
        poses: map,
    })
}

/// A positive JSON integer.
fn positive(value: Option<&serde_json::Value>) -> Option<u64> {
    value.and_then(serde_json::Value::as_u64).filter(|n| *n > 0)
}

/// One line: not blank, no surrounding spaces, at most `max` characters, no control characters.
fn line(text: &str, max: usize) -> bool {
    !text.trim().is_empty()
        && text.trim() == text
        && text.chars().count() <= max
        && !text.chars().any(char::is_control)
}

/// An SPDX expression's characters: letters, digits, `.`, `-`, `+`, parentheses and single
/// spaces between words.
fn spdx(licence: &str) -> bool {
    let allowed =
        |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | '(' | ')' | ' ');
    line(licence, MAX_LICENCE)
        && licence.chars().any(|c| c.is_ascii_alphanumeric())
        && licence.chars().all(allowed)
        && !licence.contains("  ")
}

/// The picture `file` decoded with limits, fitted into `size` and encoded as lossy WebP.
fn redraw(file: &str, bytes: &[u8], size: u32) -> Result<Vec<u8>, Refused> {
    let format = if file.ends_with(".png") {
        if !bytes.starts_with(PNG_SIGNATURE) {
            return refuse(Refusal::NotAPicture, file);
        }
        ImageFormat::Png
    } else {
        if !webp_signature(bytes) {
            return refuse(Refusal::NotAPicture, file);
        }
        if animated(bytes) {
            return refuse(Refusal::Animated, file);
        }
        ImageFormat::WebP
    };

    // Decode within the limits; a header that claims too much fails before allocating.
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_EDGE);
    limits.max_image_height = Some(MAX_EDGE);
    limits.max_alloc = Some(MAX_DECODED);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(limits);
    let image = match reader.decode() {
        Ok(image) => image,
        Err(ImageError::Limits(_)) => return refuse(Refusal::PictureTooLarge, file),
        Err(error) => return refuse(Refusal::NotAPicture, format!("{file}: {error}")),
    };
    if image.width() == 0 || image.height() == 0 {
        return refuse(Refusal::NotAPicture, file);
    }
    let pixels = ankiquest_art::fit(image.into_rgba8(), size);
    encode(&pixels).map_err(|detail| Refused {
        reason: Refusal::NotAPicture,
        detail: format!("{file}: {detail}"),
    })
}

/// `RIFF`, a size, then `WEBP`.
fn webp_signature(bytes: &[u8]) -> bool {
    bytes.len() >= 16 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

/// Whether a WebP's extended header sets the animation flag.
fn animated(bytes: &[u8]) -> bool {
    const ANIMATION: u8 = 0x02;
    bytes.len() > 20 && &bytes[12..16] == b"VP8X" && bytes[20] & ANIMATION != 0
}

/// Lossy WebP with full-quality alpha, as Art's packs.
fn encode(image: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut config = webp::WebPConfig::new().map_err(|()| "libwebp config failed")?;
    config.lossless = 0;
    config.quality = QUALITY;
    config.alpha_quality = ALPHA_QUALITY;
    config.method = METHOD;
    let encoder = webp::Encoder::from_rgba(image, image.width(), image.height());
    let memory = encoder
        .encode_advanced(&config)
        .map_err(|error| format!("WebP encoding failed: {error:?}"))?;
    Ok(memory.to_vec())
}

#[cfg(test)]
mod tests;
