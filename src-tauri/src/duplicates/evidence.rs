//! Versioned replacement evidence. Lossy matches are suggestions until calibrated.
use super::{fingerprint, verification};
use image::{imageops::FilterType, DynamicImage, ImageDecoder, ImageFormat, ImageReader, RgbImage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom},
    path::Path,
};

pub(super) const POLICY: &str = "illuTag-replacement-v2";
pub(super) const LOSSY_CALIBRATED: bool = false;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Evidence {
    pub automatic: bool,
    pub level: String,
    pub reason: String,
    /// Compact diagnostics only; never stores pixels or a full difference map.
    pub metrics: String,
}
impl Evidence {
    pub fn result(level: &str, reason: &str, automatic: bool) -> Self {
        Self {
            automatic,
            level: level.into(),
            reason: reason.into(),
            metrics: "{}".into(),
        }
    }
    pub fn uncertain(reason: &str) -> Self {
        Self::result("uncertain", reason, false)
    }
}

struct Raster {
    pixels: RgbImage,
    metadata: [u8; 32],
}

fn metadata(path: &Path, format: ImageFormat) -> Result<[u8; 32], String> {
    let source = File::open(path).map_err(|_| "read_error")?;
    if source.metadata().map_err(|_| "read_error")?.len() > 128 * 1024 * 1024 {
        return Err("metadata_limit".into());
    }
    let mut file = BufReader::new(source);
    let mut digest = Sha256::new();
    let mut retained = 0_usize;
    if format == ImageFormat::Png {
        file.seek(SeekFrom::Start(8)).map_err(|_| "read_error")?;
        loop {
            let mut header = [0; 8];
            file.read_exact(&mut header).map_err(|_| "decode_error")?;
            let size = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
            let tag = &header[4..];
            if tag == b"acTL" {
                return Err("animation_not_verified".into());
            }
            if ![b"IHDR".as_slice(), b"IDAT", b"IEND"].contains(&tag) {
                retained = retained.saturating_add(size);
                if retained > 16 * 1024 * 1024 {
                    return Err("metadata_limit".into());
                }
                digest.update(header);
                let mut remaining = size;
                let mut chunk = [0; 8192];
                while remaining > 0 {
                    let len = remaining.min(chunk.len());
                    file.read_exact(&mut chunk[..len])
                        .map_err(|_| "decode_error")?;
                    digest.update(&chunk[..len]);
                    remaining -= len;
                }
                file.seek(SeekFrom::Current(4)).map_err(|_| "read_error")?;
            } else {
                file.seek(SeekFrom::Current(size as i64 + 4))
                    .map_err(|_| "read_error")?;
            }
            if tag == b"IEND" {
                break;
            }
        }
    } else {
        file.seek(SeekFrom::Start(2)).map_err(|_| "read_error")?;
        let mut in_scan = false;
        loop {
            let mut marker = [0; 2];
            if in_scan {
                // Walk entropy bytes as well: progressive/multiple scans may
                // carry protected APP/COM markers after the first SOS.
                loop {
                    let mut byte = [0];
                    file.read_exact(&mut byte).map_err(|_| "decode_error")?;
                    if byte[0] != 255 {
                        continue;
                    }
                    loop {
                        file.read_exact(&mut byte).map_err(|_| "decode_error")?;
                        if byte[0] != 255 {
                            break;
                        }
                    }
                    if byte[0] == 0 || (0xd0..=0xd7).contains(&byte[0]) {
                        continue;
                    }
                    marker = [255, byte[0]];
                    break;
                }
                in_scan = false;
            } else {
                file.read_exact(&mut marker).map_err(|_| "decode_error")?;
                if marker[0] != 255 {
                    return Err("decode_error".into());
                }
                while marker[1] == 255 {
                    file.read_exact(&mut marker[1..])
                        .map_err(|_| "decode_error")?;
                }
            }
            if marker[1] == 0xd9 {
                break;
            }
            if marker[1] == 0xdc {
                return Err("format_outside_raster_scope".into());
            }
            if marker[1] == 0x01 || (0xd0..=0xd8).contains(&marker[1]) {
                continue;
            }
            let mut size = [0; 2];
            file.read_exact(&mut size).map_err(|_| "decode_error")?;
            let len = u16::from_be_bytes(size)
                .checked_sub(2)
                .ok_or("decode_error")? as usize;
            if (0xe0..=0xef).contains(&marker[1]) || marker[1] == 0xfe {
                retained += len;
                if retained > 16 * 1024 * 1024 {
                    return Err("metadata_limit".into());
                }
                let mut bytes = vec![0; len];
                file.read_exact(&mut bytes).map_err(|_| "decode_error")?;
                // Keep profile, orientation, author/comment and application metadata.
                digest.update(marker);
                digest.update(size);
                digest.update(bytes);
            } else {
                file.seek(SeekFrom::Current(len as i64))
                    .map_err(|_| "read_error")?;
            }
            if marker[1] == 0xda {
                in_scan = true;
            }
        }
    }
    // Keep appended application data too, even if a decoder ignores it.
    let mut trailing = [0; 8192];
    let mut started = false;
    loop {
        let count = file.read(&mut trailing).map_err(|_| "read_error")?;
        if count == 0 {
            break;
        }
        if !started {
            digest.update(b"\0trailing\0");
            started = true;
        }
        retained = retained.saturating_add(count);
        if retained > 16 * 1024 * 1024 {
            return Err("metadata_limit".into());
        }
        digest.update(&trailing[..count]);
    }
    Ok(digest.finalize().into())
}

fn decode(path: &Path) -> Result<Raster, String> {
    let mut reader = ImageReader::open(path)
        .map_err(|_| "read_error")?
        .with_guessed_format()
        .map_err(|_| "decode_error")?;
    let format = reader.format().ok_or("decode_error")?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg) {
        return Err("format_outside_raster_scope".into());
    }
    let profile = metadata(path, format)?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|_| "decode_error")?;
    let (w, h) = decoder.dimensions();
    if u64::from(w) * u64::from(h) > fingerprint::MAX_PIXELS
        || decoder.total_bytes() > 128 * 1024 * 1024
    {
        return Err("pixel_or_memory_limit".into());
    }
    // No white compositing, precision truncation or ambiguous color conversion.
    if decoder.color_type() != image::ColorType::Rgb8 {
        return Err("alpha_or_precision_outside_scope".into());
    }
    let orientation = decoder.orientation().map_err(|_| "decode_error")?;
    let mut pixels = DynamicImage::from_decoder(decoder).map_err(|_| "decode_error")?;
    pixels.apply_orientation(orientation);
    Ok(Raster {
        pixels: pixels.into_rgb8(),
        metadata: profile,
    })
}

pub(super) fn verify(
    a: &Path,
    b: &Path,
    control: &mut dyn FnMut() -> Result<(), String>,
) -> Result<Evidence, String> {
    control()?;
    let (Ok(before_a), Ok(before_b)) = (
        fingerprint::source_signature(a),
        fingerprint::source_signature(b),
    ) else {
        return Ok(Evidence::uncertain("source_unavailable"));
    };
    let result = (|| -> Result<Evidence, String> {
        let left = match decode(a) {
            Ok(x) => x,
            Err(e) => return Ok(Evidence::uncertain(&e)),
        };
        control()?;
        let right = match decode(b) {
            Ok(x) => x,
            Err(e) => return Ok(Evidence::uncertain(&e)),
        };
        control()?;
        let dims = serde_json::json!({"a":[left.pixels.width(),left.pixels.height()],"b":[right.pixels.width(),right.pixels.height()]});
        let same_metadata = left.metadata == right.metadata;
        if left.pixels == right.pixels {
            if !same_metadata {
                let mut e = Evidence::uncertain("protected_metadata_difference");
                e.metrics = dims.to_string();
                return Ok(e);
            }
            let mut e = Evidence::result("raster", "full_raster_equal", true);
            e.metrics = dims.to_string();
            return Ok(e);
        }
        let (w, h) = (
            left.pixels.width().min(right.pixels.width()),
            left.pixels.height().min(right.pixels.height()),
        );
        if w.min(h) < 64 || w.max(h) < 256 {
            return Ok(Evidence::uncertain("insufficient_resolution"));
        }
        let ratio = ((left.pixels.width() as f64 / left.pixels.height() as f64)
            / (right.pixels.width() as f64 / right.pixels.height() as f64)
            - 1.0)
            .abs();
        if ratio > 0.002 {
            return Ok(Evidence::result(
                "difference",
                "aspect_ratio_changed",
                false,
            ));
        }
        if left.pixels.width().max(right.pixels.width()) > w.saturating_mul(4)
            || left.pixels.height().max(right.pixels.height()) > h.saturating_mul(4)
        {
            return Ok(Evidence::uncertain("resolution_gap"));
        }
        let mut diagnostics = Vec::new();
        let mut last_reason = "unexplained_difference";
        for (name, kernel) in [
            ("triangle", FilterType::Triangle),
            ("catmull_rom", FilterType::CatmullRom),
            ("lanczos3", FilterType::Lanczos3),
        ] {
            control()?;
            // One whole-image resampling hypothesis, never a local deformation fit.
            // Borrow native-sized rasters instead of duplicating both originals.
            let mut x = if left.pixels.dimensions() == (w, h) {
                Cow::Borrowed(&left.pixels)
            } else {
                Cow::Owned(image::imageops::resize(&left.pixels, w, h, kernel))
            };
            let mut y = if right.pixels.dimensions() == (w, h) {
                Cow::Borrowed(&right.pixels)
            } else {
                Cow::Owned(image::imageops::resize(&right.pixels, w, h, kernel))
            };
            let mut passed = true;
            for level in 0..3 {
                if let Some(reason) = verification::compare_level(&x, &y, control)? {
                    last_reason = reason;
                    passed = false;
                    break;
                }
                if level < 2 {
                    let nw = x.width().div_ceil(2);
                    let nh = x.height().div_ceil(2);
                    x = Cow::Owned(image::imageops::resize(&*x, nw, nh, FilterType::Triangle));
                    y = Cow::Owned(image::imageops::resize(&*y, nw, nh, FilterType::Triangle));
                }
            }
            diagnostics.push(serde_json::json!({"kernel":name,"passed":passed,"reason":if passed {""} else {last_reason}}));
            if passed {
                let mut e = Evidence::result(
                    "lossy",
                    "lossy_uncalibrated",
                    LOSSY_CALIBRATED && same_metadata,
                );
                e.metrics=serde_json::json!({"dimensions":dims,"models":diagnostics,"sameMetadata":same_metadata,"calibrated":LOSSY_CALIBRATED}).to_string();
                return Ok(e);
            }
            if left.pixels.dimensions() == right.pixels.dimensions() {
                break;
            }
        }
        let mut e = Evidence::result("difference", last_reason, false);
        e.metrics = serde_json::json!({"dimensions":dims,"models":diagnostics}).to_string();
        Ok(e)
    })()?;
    control()?;
    if fingerprint::source_signature(a).ok() != Some(before_a)
        || fingerprint::source_signature(b).ok() != Some(before_b)
    {
        return Ok(Evidence::uncertain("source_changed"));
    }
    Ok(result)
}
