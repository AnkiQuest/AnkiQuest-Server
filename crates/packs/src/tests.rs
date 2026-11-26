use super::*;
use image::{DynamicImage, Rgba};
use std::io::Write;
use zip::write::SimpleFileOptions;

fn poses() -> Poses {
    Poses::parse(include_str!("../../../tests/fixtures/companion_poses.json")).unwrap()
}

/// Official ids, as the server reserves them.
fn reserved(id: &str) -> bool {
    matches!(id, "aki" | "ankilope" | "none")
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let image = RgbaImage::from_pixel(width, height, Rgba([200, 40, 90, 255]));
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut out, ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

fn index(poses: &BTreeMap<&str, &str>) -> serde_json::Value {
    serde_json::json!({
        "format": 1,
        "id": "mochi",
        "name": "Mochi",
        "version": 3,
        "licence": "CC-BY-4.0",
        "credit": "Ana",
        "poses": poses,
        "unknown": "dropped"
    })
}

/// Every core pose drawn from `face.png`, plus `icon` from `icon.png`.
fn names() -> BTreeMap<&'static str, &'static str> {
    let mut map: BTreeMap<&str, &str> = poses()
        .of(Kind::CoreMood)
        .into_iter()
        .map(|pose| (leak(pose), "face.png"))
        .collect();
    map.insert("icon", "icon.png");
    map
}

fn leak(text: &str) -> &'static str {
    Box::leak(text.to_string().into_boxed_str())
}

/// A zip of `files`, each stored or deflated.
fn zip_of(files: &[(&str, Vec<u8>)], method: zip::CompressionMethod) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(method);
    for (name, bytes) in files {
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn pack_with(index: serde_json::Value, pictures: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
    let mut files = vec![(PACK_INDEX, serde_json::to_vec(&index).unwrap())];
    files.extend(pictures);
    zip_of(&files, zip::CompressionMethod::Deflated)
}

fn pack() -> Vec<u8> {
    pack_with(
        index(&names()),
        vec![("face.png", png(1500, 1200)), ("icon.png", png(600, 600))],
    )
}

fn refusal(bytes: &[u8]) -> Refusal {
    canonical(bytes, &poses(), reserved).unwrap_err().reason
}

fn entries_of(zip: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(zip)).unwrap();
    (0..archive.len())
        .map(|at| {
            let mut entry = archive.by_index(at).unwrap();
            let mut data = Vec::new();
            entry.read_to_end(&mut data).unwrap();
            (entry.name().to_string(), data)
        })
        .collect()
}

#[test]
fn a_pack_is_rebuilt_as_webp_with_its_checked_fields() {
    let pack = canonical(&pack(), &poses(), reserved).unwrap();
    assert_eq!(pack.info.id, "mochi");
    assert_eq!(pack.info.poses["face"], "face.webp");
    assert_eq!(pack.info.poses["icon"], "icon.webp");
    assert_eq!(pack.sha256, sha256(&pack.zip));

    // Only the index and the redrawn pictures, sorted; unknown fields are gone.
    let entries = entries_of(&pack.zip);
    let names: Vec<&str> = entries.keys().map(String::as_str).collect();
    assert_eq!(names, ["companion.json", "face.webp", "icon.webp"]);
    let index: serde_json::Value = serde_json::from_slice(&entries["companion.json"]).unwrap();
    assert!(index.get("unknown").is_none());
    assert!(index.get("notice").is_none());

    // Poses shrink to 1024, a picture only icons use to 432.
    let size = |file: &str| {
        image::load_from_memory_with_format(&entries[file], ImageFormat::WebP)
            .unwrap()
            .into_rgba8()
            .dimensions()
    };
    assert_eq!(size("face.webp"), (1024, 819));
    assert_eq!(size("icon.webp"), (432, 432));
}

#[test]
fn the_same_upload_gives_the_same_bytes_and_a_canonical_pack_passes_again() {
    let first = canonical(&pack(), &poses(), reserved).unwrap();
    let second = canonical(&pack(), &poses(), reserved).unwrap();
    assert_eq!(first.sha256, second.sha256);

    // A canonical pack is a valid upload, and the app's reader rules hold for it.
    let again = canonical(&first.zip, &poses(), reserved).unwrap();
    assert_eq!(again.info, first.info);
}

#[test]
fn metadata_and_trailing_bytes_do_not_survive() {
    // A PNG with a text chunk before IEND and junk after it.
    let mut face = png(64, 64);
    let end = face.len() - 12;
    let mut chunk = Vec::new();
    let text = b"tEXtComment\0secret-location";
    chunk.extend_from_slice(&((text.len() - 4) as u32).to_be_bytes());
    chunk.extend_from_slice(text);
    chunk.extend_from_slice(&crc32fast::hash(text).to_be_bytes());
    face.splice(end..end, chunk);
    face.extend_from_slice(b"<script>trailing</script>");

    let mut map = names();
    map.remove("icon");
    let bytes = pack_with(index(&map), vec![("face.png", face)]);
    let pack = canonical(&bytes, &poses(), reserved).unwrap();
    let haystack = String::from_utf8_lossy(&pack.zip);
    assert!(!haystack.contains("secret-location"));
    assert!(!haystack.contains("<script>"));
}

#[test]
fn names_that_leave_the_folder_or_are_not_pictures_are_refused() {
    for name in [
        "../face.png",
        "/face.png",
        "dir/face.png",
        "dir\\face.png",
        "Face.png",
        "face.svg",
        "face.gif",
        "face.png.exe",
        ".png",
    ] {
        let bytes = pack_with(
            index(&BTreeMap::from([("face", name)])),
            vec![(name, png(4, 4))],
        );
        assert_eq!(refusal(&bytes), Refusal::BadEntryName, "{name}");
    }
}

#[test]
fn a_name_twice_is_refused() {
    let mut map = names();
    map.remove("icon");
    let mut bytes = zip_of(
        &[
            (PACK_INDEX, serde_json::to_vec(&index(&map)).unwrap()),
            ("face.png", png(4, 4)),
            ("fXce.png", png(4, 4)),
        ],
        zip::CompressionMethod::Stored,
    );

    // Writers refuse a name twice, so rename the second entry in its headers.
    let from: &[u8] = b"fXce.png";
    let mut at = 0;
    while let Some(found) = bytes[at..].windows(from.len()).position(|w| w == from) {
        bytes[at + found..at + found + from.len()].copy_from_slice(b"face.png");
        at += found + from.len();
    }
    // The zip reader keeps only one of them, so the pack would pass with a hidden entry.
    assert_eq!(refusal(&bytes), Refusal::DuplicateEntry);
}

#[test]
fn a_png_and_a_webp_of_one_name_are_refused() {
    let mut map = names();
    map.insert("happy", "face.webp");
    map.remove("icon");
    let webp = encode(&RgbaImage::from_pixel(4, 4, Rgba([1, 2, 3, 255]))).unwrap();
    let bytes = pack_with(
        index(&map),
        vec![("face.png", png(4, 4)), ("face.webp", webp)],
    );
    assert_eq!(refusal(&bytes), Refusal::DuplicateEntry);
}

#[test]
fn too_many_entries_are_refused() {
    let mut files = vec![(PACK_INDEX, b"{}".to_vec())];
    let names: Vec<String> = (0..MAX_ENTRIES).map(|n| format!("p{n}.png")).collect();
    files.extend(names.iter().map(|name| (name.as_str(), png(1, 1))));
    let bytes = zip_of(&files, zip::CompressionMethod::Stored);
    assert_eq!(refusal(&bytes), Refusal::TooManyEntries);
}

#[test]
fn an_entry_that_inflates_past_the_limit_is_refused() {
    let bomb = vec![0; MAX_ENTRY as usize + 1];
    let bytes = pack_with(index(&names()), vec![("face.png", bomb)]);
    assert!(bytes.len() < 1024 * 1024, "the bomb is small packed");
    assert_eq!(refusal(&bytes), Refusal::TooLarge);
}

#[test]
fn an_upload_over_the_limit_is_refused_before_reading() {
    assert_eq!(refusal(&vec![0; MAX_ARCHIVE + 1]), Refusal::TooLarge);
    assert_eq!(refusal(b"not a zip"), Refusal::BadEntryName);
}

#[test]
fn pictures_must_be_what_their_name_says_and_small_enough() {
    let mut map = names();
    map.remove("icon");
    let cases = [
        ("face.png", b"GIF89a....".to_vec(), Refusal::NotAPicture),
        (
            "face.png",
            b"\x89PNG\r\n\x1a\nbroken".to_vec(),
            Refusal::NotAPicture,
        ),
        ("face.png", png(MAX_EDGE + 1, 1), Refusal::PictureTooLarge),
        (
            "face.webp",
            b"<svg xmlns='http://www.w3.org/2000/svg'/>".to_vec(),
            Refusal::NotAPicture,
        ),
    ];
    for (file, bytes, reason) in cases {
        let map: BTreeMap<&str, &str> = map.keys().map(|pose| (*pose, file)).collect();
        let pack = pack_with(index(&map), vec![(file, bytes)]);
        assert_eq!(refusal(&pack), reason, "{file}");
    }
}

#[test]
fn an_animated_webp_is_refused() {
    // RIFF, size, WEBP, then a VP8X chunk with the animation flag.
    let mut webp = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0".to_vec();
    webp.push(0x02);
    webp.extend_from_slice(&[0; 9]);
    let map: BTreeMap<&str, &str> = names()
        .keys()
        .filter(|pose| **pose != "icon")
        .map(|pose| (*pose, "face.webp"))
        .collect();
    let bytes = pack_with(index(&map), vec![("face.webp", webp)]);
    assert_eq!(refusal(&bytes), Refusal::Animated);
}

#[test]
fn the_index_follows_the_pack_rules() {
    let pictures = || vec![("face.png", png(4, 4)), ("icon.png", png(4, 4))];
    let with = |field: &str, value: serde_json::Value| {
        let mut index = index(&names());
        index[field] = value;
        pack_with(index, pictures())
    };
    let cases = [
        (with("format", 2.into()), Refusal::NewerFormat),
        (with("format", 0.into()), Refusal::BadIndex),
        (with("id", "Mochi".into()), Refusal::BadId),
        (with("id", "aki".into()), Refusal::ReservedId),
        (with("name", "".into()), Refusal::BadIndex),
        (with("name", "x".repeat(41).into()), Refusal::BadIndex),
        (with("name", "line\nbreak".into()), Refusal::BadIndex),
        (with("version", 0.into()), Refusal::BadIndex),
        (with("licence", "".into()), Refusal::BadLicence),
        (with("licence", "<b>MIT</b>".into()), Refusal::BadLicence),
        (with("credit", serde_json::Value::Null), Refusal::BadCredit),
        (with("credit", " ".into()), Refusal::BadCredit),
        (
            with("notice", "x".repeat(MAX_NOTICE + 1).into()),
            Refusal::TooLarge,
        ),
    ];
    for (bytes, reason) in cases {
        assert_eq!(refusal(&bytes), reason);
    }
    let licence = with("licence", "MIT OR (Apache-2.0 AND CC-BY-4.0)".into());
    assert!(canonical(&licence, &poses(), reserved).is_ok());
}

#[test]
fn poses_and_files_must_match() {
    let pictures = || vec![("face.png", png(4, 4)), ("icon.png", png(4, 4))];

    let mut unknown = names();
    unknown.insert("dancing", "face.png");
    assert_eq!(
        refusal(&pack_with(index(&unknown), pictures())),
        Refusal::UnknownPose
    );

    let mut no_core = names();
    no_core.remove("sad");
    assert_eq!(
        refusal(&pack_with(index(&no_core), pictures())),
        Refusal::MissingCorePose
    );

    let mut missing = names();
    missing.insert("winner", "winner.png");
    assert_eq!(
        refusal(&pack_with(index(&missing), pictures())),
        Refusal::MissingFile
    );

    let mut unused = pictures();
    unused.push(("extra.png", png(4, 4)));
    assert_eq!(
        refusal(&pack_with(index(&names()), unused)),
        Refusal::UnusedFile
    );

    assert_eq!(
        refusal(&zip_of(
            &[("face.png", png(4, 4))],
            zip::CompressionMethod::Stored
        )),
        Refusal::MissingFile
    );
}

#[test]
fn ids_follow_the_pack_rule() {
    assert!(valid_id("mochi_2"));
    assert!(valid_id(&format!("a{}", "b".repeat(31))));
    for id in [
        "",
        "2mochi",
        "_x",
        "Mochi",
        "mo-chi",
        &format!("a{}", "b".repeat(32)),
    ] {
        assert!(!valid_id(id), "{id}");
    }
}
