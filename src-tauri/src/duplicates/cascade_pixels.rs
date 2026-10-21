//! Three-pass policy: tolerant content comparison, then lossless sample equality.
use super::{fingerprint, verification};
use image::{imageops::FilterType, DynamicImage, ImageDecoder, ImageFormat, ImageReader, RgbImage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
    cell::Cell,
    fs::File,
    io::{BufReader, Seek, SeekFrom},
    path::Path,
};

#[cfg(test)]
thread_local! {
    static DECODE_COUNT: Cell<u64> = const { Cell::new(0) };
}

#[cfg(test)]
pub(super) fn reset_decode_count() {
    DECODE_COUNT.with(|count| count.set(0));
}

#[cfg(test)]
pub(super) fn decode_count() -> u64 {
    DECODE_COUNT.with(Cell::get)
}

pub(super) const POLICY: &str = "illuTag-three-pass-v1";
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Analysis {
    pub kind: String,
    pub reason: String,
    pub strict: bool,
}
impl Analysis {
    pub fn new(kind: &str, reason: &str, strict: bool) -> Self {
        Self {
            kind: kind.into(),
            reason: reason.into(),
            strict,
        }
    }
    pub fn passed(&self) -> bool {
        self.kind == "near" || self.kind == "exact"
    }
}
enum Failure {
    Input(&'static str),
    Control(String),
}
fn decode(
    path: &Path,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<DynamicImage, Failure> {
    #[cfg(test)]
    DECODE_COUNT.with(|count| count.set(count.get() + 1));
    check().map_err(Failure::Control)?;
    let mut file = File::open(path).map_err(|_| Failure::Input("read_error"))?;
    let format = ImageReader::new(BufReader::new(&mut file))
        .with_guessed_format()
        .map_err(|_| Failure::Input("decode_error"))?
        .format();
    file.seek(SeekFrom::Start(0))
        .map_err(|_| Failure::Input("read_error"))?;
    match format {
        Some(ImageFormat::Gif) => return Err(Failure::Input("animation_not_verified")),
        Some(ImageFormat::Tiff | ImageFormat::Ico | ImageFormat::Avif) => {
            return Err(Failure::Input("multi_image_container_not_verified"))
        }
        Some(ImageFormat::WebP) => {
            let d = image::codecs::webp::WebPDecoder::new(BufReader::new(&mut file))
                .map_err(|_| Failure::Input("decode_error"))?;
            if d.has_animation() {
                return Err(Failure::Input("animation_not_verified"));
            }
        }
        Some(ImageFormat::Png) => {
            let d = image::codecs::png::PngDecoder::new(BufReader::new(&mut file))
                .map_err(|_| Failure::Input("decode_error"))?;
            if d.is_apng().map_err(|_| Failure::Input("decode_error"))? {
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
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let mut d = reader
        .into_decoder()
        .map_err(|_| Failure::Input("decode_error"))?;
    let (w, h) = d.dimensions();
    if w == 0
        || h == 0
        || u64::from(w) * u64::from(h) > 16 * 1024 * 1024
        || d.total_bytes() > 64 * 1024 * 1024
    {
        return Err(Failure::Input("pixel_or_memory_limit"));
    }
    if matches!(
        d.color_type(),
        image::ColorType::Rgb32F | image::ColorType::Rgba32F
    ) {
        return Err(Failure::Input("floating_point_not_verified"));
    }
    let orientation = d
        .orientation()
        .map_err(|_| Failure::Input("decode_error"))?;
    check().map_err(Failure::Control)?;
    let mut decoded = DynamicImage::from_decoder(d).map_err(|_| Failure::Input("decode_error"))?;
    decoded.apply_orientation(orientation);
    check().map_err(Failure::Control)?;
    Ok(decoded)
}

// No white composition, byte-depth truncation or resampling in strict comparison.
fn sample(image: &DynamicImage, n: usize) -> [u16; 4] {
    fn pixel<T: Copy + Into<u16>>(v: &[T], n: usize, c: usize, scale: u16) -> [u16; 4] {
        let v = &v[n * c..n * c + c];
        let get = |i: usize| v[i].into() * scale;
        match c {
            1 => [get(0), get(0), get(0), 65535],
            2 => [get(0), get(0), get(0), get(1)],
            3 => [get(0), get(1), get(2), 65535],
            _ => [get(0), get(1), get(2), get(3)],
        }
    }
    match image {
        DynamicImage::ImageLuma8(x) => pixel(x.as_raw(), n, 1, 257),
        DynamicImage::ImageLumaA8(x) => pixel(x.as_raw(), n, 2, 257),
        DynamicImage::ImageRgb8(x) => pixel(x.as_raw(), n, 3, 257),
        DynamicImage::ImageRgba8(x) => pixel(x.as_raw(), n, 4, 257),
        DynamicImage::ImageLuma16(x) => pixel(x.as_raw(), n, 1, 1),
        DynamicImage::ImageLumaA16(x) => pixel(x.as_raw(), n, 2, 1),
        DynamicImage::ImageRgb16(x) => pixel(x.as_raw(), n, 3, 1),
        DynamicImage::ImageRgba16(x) => pixel(x.as_raw(), n, 4, 1),
        _ => unreachable!("floating point input rejected before sample comparison"),
    }
}
fn strict_equal(
    a: &DynamicImage,
    b: &DynamicImage,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<bool, String> {
    if a.width() != b.width() || a.height() != b.height() {
        return Ok(false);
    }
    for n in 0..a.width() as usize * a.height() as usize {
        if n % 32768 == 0 {
            check()?;
        }
        if sample(a, n) != sample(b, n) {
            return Ok(false);
        }
    }
    Ok(true)
}
fn rgb(
    image: DynamicImage,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<RgbImage, String> {
    if let DynamicImage::ImageRgb8(x) = image {
        return Ok(x);
    }
    let mut out = RgbImage::new(image.width(), image.height());
    for (n, p) in out.pixels_mut().enumerate() {
        if n % 32768 == 0 {
            check()?;
        }
        let s = sample(&image, n);
        let alpha = u64::from(s[3]);
        for c in 0..3 {
            p[c] = (((u64::from(s[c]) * alpha + 65535 * (65535 - alpha) + 32767) / 65535 + 128)
                / 257) as u8;
        }
    }
    Ok(out)
}
pub(super) fn compare(
    a: DynamicImage,
    b: DynamicImage,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<Analysis, String> {
    if strict_equal(&a, &b, check)? {
        return Ok(Analysis::new("exact", "all_native_samples_equal", true));
    }
    if a.color().has_alpha() || b.color().has_alpha() {
        if a.width() != b.width() || a.height() != b.height() {
            return Ok(Analysis::new(
                "uncertain",
                "alpha_resampling_not_verified",
                false,
            ));
        }
        for n in 0..a.width() as usize * a.height() as usize {
            if n % 32768 == 0 {
                check()?;
            }
            if sample(&a, n)[3] != sample(&b, n)[3] {
                return Ok(Analysis::new("similar", "alpha_detail_difference", false));
            }
        }
    }
    let (aw, ah, bw, bh) = (a.width(), a.height(), b.width(), b.height());
    let (w, h) = (aw.min(bw), ah.min(bh));
    if w.min(h) < 64 || w.max(h) < 256 {
        return Ok(Analysis::new("uncertain", "insufficient_resolution", false));
    }
    if ((aw as f64 / ah as f64) / (bw as f64 / bh as f64) - 1.).abs() > 0.002 {
        return Ok(Analysis::new("similar", "aspect_ratio_difference", false));
    }
    if aw.max(bw) > w.saturating_mul(4) || ah.max(bh) > h.saturating_mul(4) {
        return Ok(Analysis::new("uncertain", "resolution_gap", false));
    }
    let left = rgb(a, check)?;
    let right = rgb(b, check)?;
    let mut reason = "local_detail_difference";
    for kernel in [
        FilterType::Triangle,
        FilterType::CatmullRom,
        FilterType::Lanczos3,
    ] {
        check()?;
        let mut x = if left.dimensions() == (w, h) {
            Cow::Borrowed(&left)
        } else {
            Cow::Owned(image::imageops::resize(&left, w, h, kernel))
        };
        let mut y = if right.dimensions() == (w, h) {
            Cow::Borrowed(&right)
        } else {
            Cow::Owned(image::imageops::resize(&right, w, h, kernel))
        };
        let mut passed = true;
        for scale in 0..3 {
            if let Some(why) = verification::compare_level_tolerance(&x, &y, 1.5, check)? {
                reason = why;
                passed = false;
                break;
            }
            if scale < 2 {
                let (nw, nh) = (x.width().div_ceil(2), x.height().div_ceil(2));
                x = Cow::Owned(image::imageops::resize(&*x, nw, nh, FilterType::Triangle));
                y = Cow::Owned(image::imageops::resize(&*y, nw, nh, FilterType::Triangle));
            }
        }
        if passed {
            return Ok(Analysis::new("near", "multiscale_content_passed", false));
        }
        if left.dimensions() == right.dimensions() {
            break;
        }
    }
    Ok(Analysis::new("similar", reason, false))
}
pub(super) fn verify(
    a: &Path,
    b: &Path,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<Analysis, String> {
    let (Ok(sa), Ok(sb)) = (
        fingerprint::source_signature(a),
        fingerprint::source_signature(b),
    ) else {
        return Ok(Analysis::new("uncertain", "source_unavailable", false));
    };
    let result = (|| {
        let x = decode(a, check)?;
        let y = decode(b, check)?;
        compare(x, y, check).map_err(Failure::Control)
    })();
    let result = match result {
        Ok(x) => x,
        Err(Failure::Input(why)) => Analysis::new("uncertain", why, false),
        Err(Failure::Control(e)) => return Err(e),
    };
    check()?;
    if fingerprint::source_signature(a).ok() != Some(sa)
        || fingerprint::source_signature(b).ok() != Some(sb)
    {
        return Ok(Analysis::new("uncertain", "source_changed", false));
    }
    Ok(result)
}
pub(super) fn native_digest(
    path: &Path,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<Option<Vec<u8>>, String> {
    let before = match fingerprint::source_signature(path) {
        Ok(s) => s,
        Err(_) => return Ok(None),
    };
    let image = match decode(path, check) {
        Ok(x) => x,
        Err(Failure::Input(_)) => return Ok(None),
        Err(Failure::Control(e)) => return Err(e),
    };
    let mut hash = Sha256::new();
    hash.update(POLICY);
    hash.update(image.width().to_le_bytes());
    hash.update(image.height().to_le_bytes());
    let mut block = Vec::with_capacity(65536);
    for n in 0..image.width() as usize * image.height() as usize {
        for value in sample(&image, n) {
            block.extend_from_slice(&value.to_le_bytes());
        }
        if block.len() >= 65536 {
            check()?;
            hash.update(&block);
            block.clear();
        }
    }
    hash.update(block);
    check()?;
    if fingerprint::source_signature(path).ok() != Some(before) {
        return Ok(None);
    }
    Ok(Some(hash.finalize().to_vec()))
}
