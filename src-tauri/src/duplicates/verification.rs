//! Conservative, source-image verification after the inexpensive fingerprint filter.
//!
//! This is a safety filter, not proof of semantic identity. In particular, detail
//! absent from the smaller source cannot be recovered. No crop, translation or
//! content rotation is fitted: such fitting could hide a genuine edit.
use super::fingerprint::{source_signature, SourceSignature, MAX_PIXELS};
use image::{imageops::FilterType, DynamicImage, ImageDecoder, ImageFormat, ImageReader, RgbImage};
use std::{
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom},
    path::Path,
};

/// Bump when normalization or any acceptance threshold changes.
pub const ALGORITHM_VERSION: &str = "illuTag-duplicate-verification-v1";
const DECODE_BYTES: u64 = 128 * 1024 * 1024;
const BLOCK: u32 = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerificationOutcome {
    Identical,
    Equivalent,
    Variant(&'static str),
    Uncertain(&'static str),
}

impl VerificationOutcome {
    pub fn is_equivalent(&self) -> bool {
        matches!(self, Self::Identical | Self::Equivalent)
    }

    pub fn reason(&self) -> &'static str {
        match self {
            Self::Identical => "byte_identical",
            Self::Equivalent => "pixel_verification_passed",
            Self::Variant(reason) | Self::Uncertain(reason) => reason,
        }
    }
}

/// `exact` compares every byte, in bounded chunks. Otherwise both original files
/// are decoded, EXIF-corrected and composited onto white, never via thumbnails.
/// Cancellation is an error; unavailable/unsupported input is Uncertain and must
/// be retained. File signatures are checked again before returning acceptance.
pub fn verify_pair(
    reference: &Path,
    candidate: &Path,
    exact: bool,
    checkpoint: &mut dyn FnMut() -> Result<(), String>,
) -> Result<VerificationOutcome, String> {
    checkpoint()?;
    let (Ok(before_a), Ok(before_b)) = (source_signature(reference), source_signature(candidate))
    else {
        return Ok(VerificationOutcome::Uncertain("read_error"));
    };
    let result = if exact {
        compare_bytes(
            reference,
            candidate,
            before_a.file_size,
            before_b.file_size,
            checkpoint,
        )?
    } else {
        let a = match decode(reference, checkpoint) {
            Ok(value) => value,
            Err(Failure::Input(reason)) => return Ok(VerificationOutcome::Uncertain(reason)),
            Err(Failure::Interrupted(error)) => return Err(error),
        };
        checkpoint()?;
        let b = match decode(candidate, checkpoint) {
            Ok(value) => value,
            Err(Failure::Input(reason)) => return Ok(VerificationOutcome::Uncertain(reason)),
            Err(Failure::Interrupted(error)) => return Err(error),
        };
        compare_pixels(a, b, checkpoint)?
    };
    checkpoint()?;
    if !unchanged(reference, &before_a) || !unchanged(candidate, &before_b) {
        return Ok(VerificationOutcome::Uncertain("source_changed"));
    }
    Ok(result)
}

fn unchanged(path: &Path, before: &SourceSignature) -> bool {
    source_signature(path).is_ok_and(|after| after == *before)
}

fn compare_bytes(
    a: &Path,
    b: &Path,
    size_a: u64,
    size_b: u64,
    checkpoint: &mut dyn FnMut() -> Result<(), String>,
) -> Result<VerificationOutcome, String> {
    if size_a != size_b {
        return Ok(VerificationOutcome::Variant("bytes_different"));
    }
    let (Ok(mut a), Ok(mut b)) = (File::open(a), File::open(b)) else {
        return Ok(VerificationOutcome::Uncertain("read_error"));
    };
    // Heap allocation avoids large stack frames on the Windows task worker.
    let mut left = vec![0_u8; 256 * 1024];
    let mut right = vec![0_u8; left.len()];
    let mut remaining = size_a;
    while remaining != 0 {
        checkpoint()?;
        let length = remaining.min(left.len() as u64) as usize;
        if a.read_exact(&mut left[..length]).is_err() || b.read_exact(&mut right[..length]).is_err()
        {
            return Ok(VerificationOutcome::Uncertain("read_error"));
        }
        if left[..length] != right[..length] {
            return Ok(VerificationOutcome::Variant("bytes_different"));
        }
        remaining -= length as u64;
    }
    // Detect growth even if it happened after the initial length check.
    let mut byte = [0_u8; 1];
    if !matches!(a.read(&mut byte), Ok(0)) || !matches!(b.read(&mut byte), Ok(0)) {
        return Ok(VerificationOutcome::Uncertain("source_changed"));
    }
    Ok(VerificationOutcome::Identical)
}

enum Failure {
    Input(&'static str),
    Interrupted(String),
}

fn decode(
    path: &Path,
    checkpoint: &mut dyn FnMut() -> Result<(), String>,
) -> Result<RgbImage, Failure> {
    checkpoint().map_err(Failure::Interrupted)?;
    let mut file = File::open(path).map_err(|_| Failure::Input("read_error"))?;
    let format = ImageReader::new(BufReader::new(&mut file))
        .with_guessed_format()
        .map_err(|_| Failure::Input("decode_error"))?
        .format();
    file.seek(SeekFrom::Start(0))
        .map_err(|_| Failure::Input("read_error"))?;
    match format {
        // GIF containers are conservatively retained, including single-frame GIFs.
        Some(ImageFormat::Gif) => return Err(Failure::Input("animation_not_verified")),
        Some(ImageFormat::WebP) => {
            let decoder = image::codecs::webp::WebPDecoder::new(BufReader::new(&mut file))
                .map_err(|_| Failure::Input("decode_error"))?;
            if decoder.has_animation() {
                return Err(Failure::Input("animation_not_verified"));
            }
        }
        Some(ImageFormat::Png) => {
            let decoder = image::codecs::png::PngDecoder::new(BufReader::new(&mut file))
                .map_err(|_| Failure::Input("decode_error"))?;
            if decoder
                .is_apng()
                .map_err(|_| Failure::Input("decode_error"))?
            {
                return Err(Failure::Input("animation_not_verified"));
            }
        }
        _ => {}
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| Failure::Input("read_error"))?;
    let mut reader = ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .map_err(|_| Failure::Input("decode_error"))?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(DECODE_BYTES);
    reader.limits(limits);
    let mut decoder = reader
        .into_decoder()
        .map_err(|_| Failure::Input("decode_error"))?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 {
        return Err(Failure::Input("decode_error"));
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS || decoder.total_bytes() > DECODE_BYTES {
        return Err(Failure::Input("pixel_or_memory_limit"));
    }
    let orientation = decoder
        .orientation()
        .map_err(|_| Failure::Input("decode_error"))?;
    checkpoint().map_err(Failure::Interrupted)?;
    let mut decoded =
        DynamicImage::from_decoder(decoder).map_err(|_| Failure::Input("decode_error"))?;
    checkpoint().map_err(Failure::Interrupted)?;
    decoded.apply_orientation(orientation);
    if !decoded.color().has_alpha() {
        return Ok(decoded.into_rgb8());
    }
    let mut rgba = decoded.into_rgba8();
    for (y, row) in rgba.rows_mut().enumerate() {
        if y % 64 == 0 {
            checkpoint().map_err(Failure::Interrupted)?;
        }
        for pixel in row {
            let alpha = u32::from(pixel[3]);
            for channel in 0..3 {
                pixel[channel] =
                    ((u32::from(pixel[channel]) * alpha + 255 * (255 - alpha) + 127) / 255) as u8;
            }
            pixel[3] = 255;
        }
    }
    Ok(DynamicImage::ImageRgba8(rgba).into_rgb8())
}

#[derive(Clone, Copy, Default)]
struct Difference {
    rgb: u64,
    edge: u64,
    signed: [i64; 3],
    pixels: u32,
}

impl Difference {
    fn add(&mut self, other: Self) {
        self.rgb += other.rgb;
        self.edge += other.edge;
        self.pixels += other.pixels;
        for channel in 0..3 {
            self.signed[channel] += other.signed[channel];
        }
    }
    fn exceeds(self, rgb: f64, edge: f64) -> bool {
        self.pixels != 0
            && (self.rgb as f64 > rgb * 3.0 * f64::from(self.pixels)
                || self.edge as f64 > edge * 2.0 * f64::from(self.pixels))
    }
}

fn gray(raw: &[u8], offset: usize) -> i32 {
    (77 * i32::from(raw[offset])
        + 150 * i32::from(raw[offset + 1])
        + 29 * i32::from(raw[offset + 2])
        + 128)
        >> 8
}

/// No full-sized difference/blur copies. A pair of 32MP RGB8 images costs 183MiB;
/// decoding/alpha/orientation peak stays below 352MiB of owned pixel buffers.
/// The 4px block accumulator costs at most 92MiB and shrinks at coarser scales.
pub(super) fn compare_level(
    a: &RgbImage,
    b: &RgbImage,
    checkpoint: &mut dyn FnMut() -> Result<(), String>,
) -> Result<Option<&'static str>, String> {
    compare_level_tolerance(a, b, 1.0, checkpoint)
}

pub(super) fn compare_level_tolerance(
    a: &RgbImage,
    b: &RgbImage,
    tolerance: f64,
    checkpoint: &mut dyn FnMut() -> Result<(), String>,
) -> Result<Option<&'static str>, String> {
    let (width, height) = a.dimensions();
    let columns = width.div_ceil(BLOCK) as usize;
    let rows = height.div_ceil(BLOCK) as usize;
    let mut blocks = vec![Difference::default(); columns * rows];
    let left = a.as_raw();
    let right = b.as_raw();
    let stride = width as usize * 3;
    for y in 0..height as usize {
        if y % 32 == 0 {
            checkpoint()?;
        }
        for x in 0..width as usize {
            let index = y * stride + x * 3;
            let mut difference = Difference {
                pixels: 1,
                ..Default::default()
            };
            for channel in 0..3 {
                let delta = i32::from(left[index + channel]) - i32::from(right[index + channel]);
                difference.rgb += u64::from(delta.unsigned_abs());
                difference.signed[channel] = i64::from(delta);
            }
            let previous_x = if x == 0 { index } else { index - 3 };
            let previous_y = if y == 0 { index } else { index - stride };
            let center_a = gray(left, index);
            let center_b = gray(right, index);
            difference.edge = u64::from(
                ((center_a - gray(left, previous_x)) - (center_b - gray(right, previous_x)))
                    .unsigned_abs(),
            ) + u64::from(
                ((center_a - gray(left, previous_y)) - (center_b - gray(right, previous_y)))
                    .unsigned_abs(),
            );
            blocks[(y / BLOCK as usize) * columns + x / BLOCK as usize].add(difference);
        }
    }
    let mut total = Difference::default();
    for value in &blocks {
        total.add(*value);
    }
    if total
        .signed
        .iter()
        .any(|v| v.unsigned_abs() as f64 > 3.0 * tolerance * f64::from(total.pixels))
    {
        return Ok(Some("color_variant"));
    }
    if total.exceeds(3.5 * tolerance, 5.0 * tolerance) {
        return Ok(Some("global_pixel_difference"));
    }
    // Overlapping blocks (half-window stride) prevent an edit being hidden by a
    // convenient tile boundary. Every passing scale must satisfy every window.
    for (side, rgb_limit, edge_limit) in [
        (1_usize, 24.0, 32.0),
        (2, 16.0, 22.0),
        (4, 10.0, 15.0),
        (8, 7.0, 10.0),
        (16, 5.5, 7.0),
    ] {
        let step = (side / 2).max(1);
        for y in (0..rows).step_by(step) {
            if y % 32 == 0 {
                checkpoint()?;
            }
            for x in (0..columns).step_by(step) {
                let mut window = Difference::default();
                for row in y..(y + side).min(rows) {
                    for column in x..(x + side).min(columns) {
                        window.add(blocks[row * columns + column]);
                    }
                }
                if window.exceeds(rgb_limit * tolerance, edge_limit * tolerance) {
                    return Ok(Some("local_detail_difference"));
                }
            }
        }
    }
    Ok(None)
}

fn compare_pixels(
    mut a: RgbImage,
    mut b: RgbImage,
    checkpoint: &mut dyn FnMut() -> Result<(), String>,
) -> Result<VerificationOutcome, String> {
    checkpoint()?;
    let (aw, ah) = a.dimensions();
    let (bw, bh) = b.dimensions();
    let width = aw.min(bw);
    let height = ah.min(bh);
    if width.min(height) < 64 || width.max(height) < 256 {
        return Ok(VerificationOutcome::Uncertain("insufficient_resolution"));
    }
    let ratio_difference =
        ((f64::from(aw) / f64::from(ah)) / (f64::from(bw) / f64::from(bh)) - 1.0).abs();
    if ratio_difference > 0.002 {
        return Ok(VerificationOutcome::Variant("aspect_ratio_changed"));
    }
    if aw.max(bw) > width.saturating_mul(4) || ah.max(bh) > height.saturating_mul(4) {
        return Ok(VerificationOutcome::Uncertain("resolution_gap"));
    }
    // Compare at the smaller ORIGINAL resolution, with no arbitrary thumbnail
    // cap. Larger originals retain their full coverage when scaled to this grid.
    if a.dimensions() != (width, height) {
        a = image::imageops::resize(&a, width, height, FilterType::Triangle);
        checkpoint()?;
    }
    if b.dimensions() != (width, height) {
        b = image::imageops::resize(&b, width, height, FilterType::Triangle);
        checkpoint()?;
    }
    if a == b {
        return Ok(VerificationOutcome::Equivalent);
    }
    for level in 0..3 {
        checkpoint()?;
        if let Some(reason) = compare_level(&a, &b, checkpoint)? {
            return Ok(VerificationOutcome::Variant(reason));
        }
        if level < 2 {
            let next = (a.width().div_ceil(2), a.height().div_ceil(2));
            a = image::imageops::resize(&a, next.0, next.1, FilterType::Triangle);
            checkpoint()?;
            b = image::imageops::resize(&b, next.0, next.1, FilterType::Triangle);
        }
    }
    Ok(VerificationOutcome::Equivalent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{codecs::jpeg::JpegEncoder, metadata::Orientation, Rgb, Rgba, RgbaImage};
    use std::{
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    struct Scratch {
        base: PathBuf,
        root: PathBuf,
    }
    impl Scratch {
        fn new() -> Self {
            let base = std::env::var_os("ILLUTAG_TEST_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::temp_dir().join("illutag-verification-tests"));
            std::fs::create_dir_all(&base).unwrap();
            let base = base.canonicalize().unwrap();
            let root = base.join(format!(
                "verification-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&root).unwrap();
            Self { base, root }
        }
        fn path(&self, name: &str) -> PathBuf {
            self.root.join(name)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            if self.root.is_absolute() && self.root.parent() == Some(self.base.as_path()) {
                let _ = std::fs::remove_dir_all(&self.root);
            }
        }
    }
    fn scene() -> RgbImage {
        RgbImage::from_fn(512, 384, |x, y| {
            let wave =
                ((f64::from(x) / 35.0).sin() * 15.0 + (f64::from(y) / 47.0).cos() * 10.0) as i32;
            if (i64::from(x) - 310).pow(2) + (i64::from(y) - 130).pow(2) < 4200 {
                Rgb([(190 + wave) as u8, (100 + wave) as u8, (85 + wave) as u8])
            } else if x > 115 && x < 285 && y > 115 && y < 315 {
                Rgb([(70 + wave) as u8, (125 + wave) as u8, (170 + wave) as u8])
            } else {
                Rgb([
                    (125 + wave + (x / 12) as i32) as u8,
                    (150 + wave) as u8,
                    (155 + wave + (y / 16) as i32) as u8,
                ])
            }
        })
    }
    fn check(a: &RgbImage, b: &RgbImage) -> VerificationOutcome {
        compare_pixels(a.clone(), b.clone(), &mut || Ok(())).unwrap()
    }
    #[test]
    fn copied_bytes_are_identical_and_equal_size_edits_are_not() {
        let scratch = Scratch::new();
        let a = scratch.path("original.bin");
        let b = scratch.path("renamed.bin");
        let bytes = vec![42; 700_001];
        std::fs::write(&a, &bytes).unwrap();
        std::fs::copy(&a, &b).unwrap();
        assert_eq!(
            verify_pair(&a, &b, true, &mut || Ok(())).unwrap(),
            VerificationOutcome::Identical
        );
        let mut changed = bytes;
        changed[600_000] = 43;
        std::fs::write(&b, changed).unwrap();
        assert_eq!(
            verify_pair(&a, &b, true, &mut || Ok(())).unwrap(),
            VerificationOutcome::Variant("bytes_different")
        );
    }
    #[test]
    fn light_jpeg_reencoding_and_proportional_scaling_pass() {
        let original = scene();
        let mut bytes = Vec::new();
        JpegEncoder::new_with_quality(&mut bytes, 92)
            .encode_image(&original)
            .unwrap();
        let jpeg = image::load_from_memory(&bytes).unwrap().into_rgb8();
        assert_eq!(check(&original, &jpeg), VerificationOutcome::Equivalent);
        let scaled = image::imageops::resize(&original, 256, 192, FilterType::Triangle);
        assert_eq!(check(&original, &scaled), VerificationOutcome::Equivalent);
        let larger = image::imageops::resize(&original, 1024, 768, FilterType::Triangle);
        assert_eq!(check(&original, &larger), VerificationOutcome::Equivalent);
    }
    #[test]
    fn local_edits_at_tile_boundaries_and_color_variants_are_retained() {
        let original = scene();
        for (x, y, side) in [(151, 151, 6), (255, 127, 4), (300, 120, 12)] {
            let mut changed = original.clone();
            for yy in y..y + side {
                for xx in x..x + side {
                    changed.put_pixel(xx, yy, Rgb([5, 5, 5]));
                }
            }
            assert!(
                !check(&original, &changed).is_equivalent(),
                "local {side}px edit must remain"
            );
        }
        let changed = RgbImage::from_fn(512, 384, |x, y| {
            let mut p = *original.get_pixel(x, y);
            p[0] = p[0].saturating_add(9);
            p
        });
        assert_eq!(
            check(&original, &changed),
            VerificationOutcome::Variant("color_variant")
        );
    }
    #[test]
    fn white_alpha_and_exif_orientation_are_normalized() {
        let scratch = Scratch::new();
        let a = scratch.path("opaque.png");
        let b = scratch.path("alpha.png");
        let opaque = scene();
        let rgba = RgbaImage::from_fn(512, 384, |x, y| {
            if x < 40 {
                Rgba([17, 88, 199, 0])
            } else {
                let p = opaque.get_pixel(x, y);
                Rgba([p[0], p[1], p[2], 255])
            }
        });
        let mut white = opaque.clone();
        for y in 0..384 {
            for x in 0..40 {
                white.put_pixel(x, y, Rgb([255, 255, 255]));
            }
        }
        white.save(&a).unwrap();
        rgba.save(&b).unwrap();
        assert_eq!(
            verify_pair(&a, &b, false, &mut || Ok(())).unwrap(),
            VerificationOutcome::Equivalent
        );
        let mut oriented = DynamicImage::ImageRgb8(image::imageops::rotate270(&opaque));
        oriented.apply_orientation(Orientation::Rotate90);
        assert_eq!(
            check(&opaque, &oriented.into_rgb8()),
            VerificationOutcome::Equivalent
        );
        // Exercise decoder metadata, not only apply_orientation in isolation.
        let sideways = image::imageops::rotate270(&opaque);
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, 100)
            .encode_image(&sideways)
            .unwrap();
        let tiff = [
            b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 1, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0,
        ];
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff);
        let mut tagged = jpeg[..2].to_vec();
        tagged.extend_from_slice(&[0xff, 0xe1]);
        tagged.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        tagged.extend_from_slice(&app1);
        tagged.extend_from_slice(&jpeg[2..]);
        let oriented_path = scratch.path("oriented.jpg");
        let original_path = scratch.path("original.png");
        std::fs::write(&oriented_path, tagged).unwrap();
        opaque.save(&original_path).unwrap();
        assert_eq!(
            verify_pair(&original_path, &oriented_path, false, &mut || Ok(())).unwrap(),
            VerificationOutcome::Equivalent
        );
    }
    #[test]
    fn broken_small_large_gap_and_cancelled_inputs_are_never_accepted() {
        let scratch = Scratch::new();
        let a = scratch.path("bad.png");
        let b = scratch.path("good.png");
        std::fs::write(&a, b"broken").unwrap();
        scene().save(&b).unwrap();
        assert!(matches!(
            verify_pair(&a, &b, false, &mut || Ok(())).unwrap(),
            VerificationOutcome::Uncertain(_)
        ));
        let small = image::imageops::resize(&scene(), 128, 96, FilterType::Triangle);
        assert_eq!(
            check(&small, &small),
            VerificationOutcome::Uncertain("insufficient_resolution")
        );
        let large = image::imageops::resize(&scene(), 2560, 1920, FilterType::Triangle);
        assert_eq!(
            check(&scene(), &large),
            VerificationOutcome::Uncertain("resolution_gap")
        );
        assert_eq!(
            verify_pair(&b, &b, false, &mut || Err("cancelled".into())).unwrap_err(),
            "cancelled"
        );
        let mut checkpoints = 0;
        assert_eq!(
            verify_pair(&b, &b, false, &mut || {
                checkpoints += 1;
                if checkpoints > 4 {
                    Err("cancelled".into())
                } else {
                    Ok(())
                }
            })
            .unwrap_err(),
            "cancelled"
        );
        assert!(checkpoints > 4);
    }
}
