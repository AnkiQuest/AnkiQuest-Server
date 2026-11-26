//! Companion art shared by the server's build script and the repository
//! tools: the pose list with its fallbacks (the pinned release's `poses.json`,
//! staged into [`MASTERS`]) and the downscaler every exported copy of a master
//! goes through.
//!
//! Masters are the PNGs in the pinned AnkiQuest-Art release, staged into [`MASTERS`], found
//! by companion and pose through its `manifest.json` ([`Manifest`]). Its
//! `archive/` holds pictures that are not masters; nothing reads it as one
//! ([`archived`]). Every copy is made from
//! the masters with [`shrink`], so all exports resample the same way.

use std::path::Path;

use image::codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder};
use image::imageops::{self, FilterType};
use image::{ImageEncoder, Rgba32FImage, RgbaImage};

mod manifest;
mod poses;

pub use manifest::{Manifest, Master, Shared};
pub use poses::{Kind, Poses};

/// The pose list of the staged AnkiQuest-Art release, relative to the repository root.
pub const POSES: &str = "art/poses.json";

/// The folder that holds the masters, relative to the repository root: the
/// staged AnkiQuest-Art release, `<companion>/<pose>.png` below it. Every tool
/// finds masters through this.
pub const MASTERS: &str = "art";

/// The name of a folder of pictures that are not masters: replaced masters,
/// earlier copies, source packs. It is never exported to a client.
pub const ARCHIVE: &str = "archive";

/// Whether `path`, relative to any root, is inside an [`ARCHIVE`] folder.
pub fn archived(path: &str) -> bool {
    path.split(['/', '\\']).any(|part| part == ARCHIVE)
}

/// Decodes a PNG master as 8-bit RGBA.
pub fn load(path: &Path) -> Result<RgbaImage, String> {
    let image = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(image.into_rgba8())
}

/// The size that fits `width` by `height` into `size` square, never upscaled.
pub fn target(width: u32, height: u32, size: u32) -> (u32, u32) {
    let scale = (f64::from(size) / f64::from(width.max(height))).min(1.0);
    let side = |n: u32| (f64::from(n) * scale).round_ties_even() as u32;
    (side(width), side(height))
}

/// `image` fitted into `size` square with [`shrink`].
pub fn fit(image: RgbaImage, size: u32) -> RgbaImage {
    let (width, height) = target(image.width(), image.height(), size);
    shrink(image, width, height)
}

/// Lanczos downscale in premultiplied alpha, as Pillow does, so fully
/// transparent pixels do not bleed their colour into the edges.
pub fn shrink(image: RgbaImage, width: u32, height: u32) -> RgbaImage {
    if image.dimensions() == (width, height) {
        return image;
    }
    let mut linear = Rgba32FImage::from(image::DynamicImage::ImageRgba8(image));
    for pixel in linear.pixels_mut() {
        let alpha = pixel[3];
        pixel.0[..3].iter_mut().for_each(|c| *c *= alpha);
    }
    let mut small = imageops::resize(&linear, width, height, FilterType::Lanczos3);
    for pixel in small.pixels_mut() {
        unpremultiply(&mut pixel.0);
    }
    image::DynamicImage::ImageRgba32F(small).into_rgba8()
}

/// Divides colour by alpha, clamping Lanczos overshoot.
fn unpremultiply(pixel: &mut [f32; 4]) {
    let alpha = pixel[3].clamp(0.0, 1.0);
    pixel[3] = alpha;
    if alpha == 0.0 {
        pixel[..3].fill(0.0);
        return;
    }
    pixel[..3]
        .iter_mut()
        .for_each(|c| *c = (*c / alpha).clamp(0.0, 1.0));
}

/// Encodes an 8-bit RGBA PNG, compact and the same bytes on every run.
pub fn encode(image: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let encoder =
        PngEncoder::new_with_quality(&mut bytes, CompressionType::Best, PngFilter::Adaptive);
    encoder
        .write_image(
            image,
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| format!("PNG encoding failed: {e}"))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_folders_are_found_anywhere() {
        assert!(archived("art/archive/cow/face.png"));
        assert!(archived("art/cow/archive/face.png"));
        assert!(archived("archive/anki-tan-pack/emoji/a.png"));
        assert!(!archived("art/cow/face.png"));
        assert!(!archived("art/archives/face.png"));
    }

    #[test]
    fn target_never_upscales() {
        assert_eq!((336, 336), target(1254, 1254, 336));
        assert_eq!((200, 100), target(200, 100, 336));
        assert_eq!((336, 168), target(1254, 627, 336));
    }

    #[test]
    fn transparent_edges_stay_clean() {
        // A red opaque half next to transparent green must not turn green.
        let mut image = RgbaImage::new(64, 64);
        for (x, _, pixel) in image.enumerate_pixels_mut() {
            *pixel = if x < 32 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 255, 0, 0])
            };
        }
        let small = shrink(image, 16, 16);
        let edge = small.get_pixel(7, 8);
        assert!(edge[1] < 8, "{edge:?}");
    }

    #[test]
    fn encoding_is_reproducible() {
        let image = fit(
            RgbaImage::from_pixel(64, 32, image::Rgba([9, 99, 199, 128])),
            16,
        );
        assert_eq!((16, 8), image.dimensions());
        assert_eq!(encode(&image).unwrap(), encode(&image).unwrap());
    }
}
