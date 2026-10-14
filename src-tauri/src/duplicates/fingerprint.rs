//! Versioned, deterministic duplicate fingerprints. No model or main database access.
use image::{imageops::FilterType, metadata::Orientation, DynamicImage, ImageDecoder, ImageFormat, ImageReader, RgbImage};
use sha2::{Digest, Sha256};
use std::{fs::File, io::{BufReader, Read, Seek, SeekFrom}, path::Path, time::UNIX_EPOCH};

pub const ALGORITHM_VERSION: &str = "illuTag-duplicate-v1";
pub const MAX_PIXELS: u64 = 32_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceSignature {
    pub file_size: u64,
    pub modified_ns: i64,
    pub identity: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NearFingerprint {
    pub phash: u64,
    pub dhash: u64,
    pub color: [u8; 48],
    pub width: u32,
    pub height: u32,
    pub low_information: bool,
}

#[derive(Clone, Debug)]
pub struct FileFingerprint {
    pub signature: SourceSignature,
    pub sha256: [u8; 32],
    pub near: Option<NearFingerprint>,
    pub skip_reason: Option<String>,
}

pub fn source_signature(path: &Path) -> Result<SourceSignature, String> {
    let file = File::open(path).map_err(|e| format!("read_error: {e}"))?;
    signature_from_file(&file)
}

fn signature_from_file(file: &File) -> Result<SourceSignature, String> {
    let metadata = file.metadata().map_err(|e| format!("read_error: {e}"))?;
    if !metadata.is_file() { return Err("not_regular_file".into()); }
    let modified = metadata.modified().map_err(|e| format!("read_error: {e}"))?;
    let modified_ns = match modified.duration_since(UNIX_EPOCH) {
        Ok(value) => value.as_nanos().min(i64::MAX as u128) as i64,
        Err(value) => -(value.duration().as_nanos().min(i64::MAX as u128) as i64),
    };
    Ok(SourceSignature { file_size: metadata.len(), modified_ns, identity: file_identity(file, &metadata)? })
}

#[cfg(windows)]
fn file_identity(file: &File, _: &std::fs::Metadata) -> Result<String, String> {
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    #[derive(Default)]
    struct FileTime { low: u32, high: u32 }
    #[repr(C)]
    #[derive(Default)]
    struct ByHandleFileInformation {
        attributes: u32, creation: FileTime, access: FileTime, write: FileTime,
        volume_serial: u32, size_high: u32, size_low: u32, links: u32,
        index_high: u32, index_low: u32,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetFileInformationByHandle(handle: *mut std::ffi::c_void, info: *mut ByHandleFileInformation) -> i32;
    }
    let mut info = ByHandleFileInformation::default();
    // SAFETY: the live File owns the handle; info is the documented, fully sized C structure.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(format!("read_error: {}", std::io::Error::last_os_error()));
    }
    Ok(format!("win:{:08x}:{:08x}{:08x}", info.volume_serial, info.index_high, info.index_low))
}

#[cfg(unix)]
fn file_identity(_: &File, metadata: &std::fs::Metadata) -> Result<String, String> {
    use std::os::unix::fs::MetadataExt;
    Ok(format!("unix:{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(not(any(windows, unix)))]
fn file_identity(_: &File, _: &std::fs::Metadata) -> Result<String, String> {
    Err("unsupported_file_identity".into())
}

/// Hash bytes in bounded chunks, then decode outside any database write transaction.
/// Decoder failures retain the byte hash; source replacement/modification does not.
pub fn fingerprint(path: &Path, checkpoint: &mut dyn FnMut() -> Result<(), String>) -> Result<FileFingerprint, String> {
    checkpoint()?;
    let mut file = File::open(path).map_err(|e| format!("read_error: {e}"))?;
    let signature = signature_from_file(&file)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 256 * 1024];
    let mut read_bytes = 0_u64;
    loop {
        checkpoint()?;
        let count = file.read(&mut buffer).map_err(|e| format!("read_error: {e}"))?;
        if count == 0 { break; }
        read_bytes += count as u64;
        hasher.update(&buffer[..count]);
    }
    if read_bytes != signature.file_size || signature_from_file(&file)? != signature || source_signature(path)? != signature {
        return Err("source_changed".into());
    }
    let sha256: [u8; 32] = hasher.finalize().into();
    checkpoint()?;
    let result = decode_near(&mut file, checkpoint);
    checkpoint()?;
    if signature_from_file(&file)? != signature || source_signature(path)? != signature {
        return Err("source_changed".into());
    }
    let (near, skip_reason) = match result {
        Ok(near) => (Some(near), None),
        Err(DecodeFailure::Skip(reason)) => (None, Some(reason.into())),
        Err(DecodeFailure::Interrupted(error)) => return Err(error),
    };
    Ok(FileFingerprint { signature, sha256, near, skip_reason })
}

enum DecodeFailure { Skip(&'static str), Interrupted(String) }

fn decode_near(file: &mut File, checkpoint: &mut dyn FnMut() -> Result<(), String>) -> Result<NearFingerprint, DecodeFailure> {
    // All readers borrow the original opened handle. Reopening the pathname could
    // mix SHA(A) with decoded pixels(B) if a file is swapped out and restored.
    file.seek(SeekFrom::Start(0)).map_err(|_| DecodeFailure::Skip("decode_error"))?;
    let reader = ImageReader::new(BufReader::new(&mut *file)).with_guessed_format().map_err(|_| DecodeFailure::Skip("decode_error"))?;
    let format = reader.format();
    drop(reader);
    match format {
        Some(ImageFormat::Gif) => {
            if gif_is_animated(file, checkpoint)? { return Err(DecodeFailure::Skip("animated")); }
        }
        Some(ImageFormat::WebP) => {
            if webp_is_animated(file, checkpoint)? { return Err(DecodeFailure::Skip("animated")); }
        }
        _ => {}
    }
    file.seek(SeekFrom::Start(0)).map_err(|_| DecodeFailure::Skip("decode_error"))?;
    let mut reader = ImageReader::new(BufReader::new(&mut *file)).with_guessed_format().map_err(|_| DecodeFailure::Skip("decode_error"))?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|_| DecodeFailure::Skip("decode_error"))?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 { return Err(DecodeFailure::Skip("decode_error")); }
    if u64::from(width) * u64::from(height) > MAX_PIXELS || decoder.total_bytes() > 256 * 1024 * 1024 {
        return Err(DecodeFailure::Skip("pixel_limit"));
    }
    let orientation = decoder.orientation().map_err(|_| DecodeFailure::Skip("decode_error"))?;
    checkpoint().map_err(DecodeFailure::Interrupted)?;
    let decoded = DynamicImage::from_decoder(decoder).map_err(|_| DecodeFailure::Skip("decode_error"))?;
    near_from_image(decoded, orientation, checkpoint).map_err(DecodeFailure::Interrupted)
}

fn gif_is_animated(file: &mut File, checkpoint: &mut dyn FnMut() -> Result<(), String>) -> Result<bool, DecodeFailure> {
    file.seek(SeekFrom::Start(0)).map_err(|_| DecodeFailure::Skip("decode_error"))?;
    let mut reader = BufReader::new(file);
    let mut header = [0_u8; 13];
    reader.read_exact(&mut header).map_err(|_| DecodeFailure::Skip("decode_error"))?;
    if !matches!(&header[..6], b"GIF87a" | b"GIF89a") { return Err(DecodeFailure::Skip("decode_error")); }
    let mut scratch = [0_u8; 768];
    if header[10] & 0x80 != 0 {
        let length = 3 * (1_usize << ((header[10] & 7) + 1));
        reader.read_exact(&mut scratch[..length]).map_err(|_| DecodeFailure::Skip("decode_error"))?;
    }
    let mut frames = 0;
    loop {
        checkpoint().map_err(DecodeFailure::Interrupted)?;
        let mut marker = [0_u8; 1];
        reader.read_exact(&mut marker).map_err(|_| DecodeFailure::Skip("decode_error"))?;
        match marker[0] {
            0x3b => return Ok(false),
            0x21 => {
                reader.read_exact(&mut marker).map_err(|_| DecodeFailure::Skip("decode_error"))?;
                skip_gif_blocks(&mut reader, checkpoint)?;
            }
            0x2c => {
                frames += 1;
                if frames >= 2 { return Ok(true); }
                let mut descriptor = [0_u8; 9];
                reader.read_exact(&mut descriptor).map_err(|_| DecodeFailure::Skip("decode_error"))?;
                if descriptor[8] & 0x80 != 0 {
                    let length = 3 * (1_usize << ((descriptor[8] & 7) + 1));
                    reader.read_exact(&mut scratch[..length]).map_err(|_| DecodeFailure::Skip("decode_error"))?;
                }
                reader.read_exact(&mut marker).map_err(|_| DecodeFailure::Skip("decode_error"))?; // LZW code size
                skip_gif_blocks(&mut reader, checkpoint)?;
            }
            _ => return Err(DecodeFailure::Skip("decode_error")),
        }
    }
}

fn skip_gif_blocks(reader: &mut impl Read, checkpoint: &mut dyn FnMut() -> Result<(), String>) -> Result<(), DecodeFailure> {
    let mut block = [0_u8; 255];
    let mut size = [0_u8; 1];
    let mut blocks = 0_u32;
    loop {
        reader.read_exact(&mut size).map_err(|_| DecodeFailure::Skip("decode_error"))?;
        if size[0] == 0 { return Ok(()); }
        reader.read_exact(&mut block[..size[0] as usize]).map_err(|_| DecodeFailure::Skip("decode_error"))?;
        blocks += 1;
        if blocks % 256 == 0 { checkpoint().map_err(DecodeFailure::Interrupted)?; }
    }
}

fn webp_is_animated(file: &mut File, checkpoint: &mut dyn FnMut() -> Result<(), String>) -> Result<bool, DecodeFailure> {
    file.seek(SeekFrom::Start(0)).map_err(|_| DecodeFailure::Skip("decode_error"))?;
    let length = file.metadata().map_err(|_| DecodeFailure::Skip("decode_error"))?.len();
    let mut reader = BufReader::new(file);
    let mut header = [0_u8; 12];
    reader.read_exact(&mut header).map_err(|_| DecodeFailure::Skip("decode_error"))?;
    if &header[..4] != b"RIFF" || &header[8..] != b"WEBP" { return Err(DecodeFailure::Skip("decode_error")); }
    let riff_end = u64::from(u32::from_le_bytes(header[4..8].try_into().unwrap())) + 8;
    if riff_end > length { return Err(DecodeFailure::Skip("decode_error")); }
    while reader.stream_position().map_err(|_| DecodeFailure::Skip("decode_error"))? < riff_end {
        checkpoint().map_err(DecodeFailure::Interrupted)?;
        let mut chunk = [0_u8; 8];
        reader.read_exact(&mut chunk).map_err(|_| DecodeFailure::Skip("decode_error"))?;
        let size = u64::from(u32::from_le_bytes(chunk[4..].try_into().unwrap()));
        let start = reader.stream_position().map_err(|_| DecodeFailure::Skip("decode_error"))?;
        let end = start.checked_add(size + (size & 1)).ok_or(DecodeFailure::Skip("decode_error"))?;
        if end > riff_end { return Err(DecodeFailure::Skip("decode_error")); }
        if &chunk[..4] == b"ANIM" || &chunk[..4] == b"ANMF" { return Ok(true); }
        if &chunk[..4] == b"VP8X" {
            let mut flags = [0_u8; 1];
            if size < 10 { return Err(DecodeFailure::Skip("decode_error")); }
            reader.read_exact(&mut flags).map_err(|_| DecodeFailure::Skip("decode_error"))?;
            if flags[0] & 0x02 != 0 { return Ok(true); }
        }
        reader.seek(SeekFrom::Start(end)).map_err(|_| DecodeFailure::Skip("decode_error"))?;
    }
    Ok(false)
}

fn near_from_image(mut image: DynamicImage, orientation: Orientation, checkpoint: &mut dyn FnMut() -> Result<(), String>) -> Result<NearFingerprint, String> {
    image.apply_orientation(orientation);
    let (width, height) = (image.width(), image.height());
    let rgba = image.into_rgba8();
    let mut rgb = RgbImage::new(width, height);
    for (row, pixels) in rgba.rows().enumerate() {
        if row % 256 == 0 { checkpoint()?; }
        for (column, pixel) in pixels.enumerate() {
            let alpha = u32::from(pixel[3]);
            let mut composite = [0_u8; 3];
            for channel in 0..3 {
                composite[channel] = ((u32::from(pixel[channel]) * alpha + 255 * (255 - alpha) + 127) / 255) as u8;
            }
            rgb.put_pixel(column as u32, row as u32, image::Rgb(composite));
        }
    }
    drop(rgba);
    checkpoint()?;
    let reduced = image::imageops::resize(&rgb, 32, 32, FilterType::Triangle);
    let mut gray = [0_f64; 1024];
    for (i, pixel) in reduced.pixels().enumerate() { gray[i] = f64::from(luma(pixel.0)); }
    let phash = phash_from_gray(&gray);
    let difference = image::imageops::resize(&rgb, 9, 8, FilterType::Triangle);
    let mut dhash = 0_u64;
    for y in 0..8 {
        for x in 0..8 {
            if luma(difference.get_pixel(x, y).0) > luma(difference.get_pixel(x + 1, y).0) {
                dhash |= 1_u64 << (y * 8 + x);
            }
        }
    }
    let colors = image::imageops::resize(&rgb, 4, 4, FilterType::Triangle);
    let mut color = [0_u8; 48];
    color.copy_from_slice(colors.as_raw());
    let low_information = low_information(&reduced, &gray);
    checkpoint()?;
    Ok(NearFingerprint { phash, dhash, color, width, height, low_information })
}

fn luma(rgb: [u8; 3]) -> u8 {
    ((77 * u32::from(rgb[0]) + 150 * u32::from(rgb[1]) + 29 * u32::from(rgb[2]) + 128) >> 8) as u8
}

fn low_information(rgb: &RgbImage, gray: &[f64; 1024]) -> bool {
    let mean = gray.iter().sum::<f64>() / 1024.0;
    let gray_variance = gray.iter().map(|value| (value - mean).powi(2)).sum::<f64>() / 1024.0;
    let mut variance = 0.0;
    for channel in 0..3 {
        let mean = rgb.pixels().map(|p| f64::from(p[channel])).sum::<f64>() / 1024.0;
        variance += rgb.pixels().map(|p| (f64::from(p[channel]) - mean).powi(2)).sum::<f64>() / 1024.0;
    }
    gray_variance < 16.0 && variance < 48.0
}

/// Separable orthonormal DCT-II, keeping low 8x8 coefficients. DC never sets bit 0.
fn dct_low_coefficients(gray: &[f64; 1024]) -> [f64; 64] {
    static BASIS: std::sync::OnceLock<[[f64; 32]; 8]> = std::sync::OnceLock::new();
    let basis = BASIS.get_or_init(|| std::array::from_fn(|frequency| {
        let scale = if frequency == 0 { (1.0_f64 / 32.0).sqrt() } else { (2.0_f64 / 32.0).sqrt() };
        std::array::from_fn(|position| scale * (std::f64::consts::PI * (2 * position + 1) as f64 * frequency as f64 / 64.0).cos())
    }));
    let mut horizontal = [[0.0; 8]; 32];
    for y in 0..32 {
        for u in 0..8 { horizontal[y][u] = (0..32).map(|x| gray[y * 32 + x] * basis[u][x]).sum(); }
    }
    let mut coefficients = [0.0; 64];
    for v in 0..8 {
        for u in 0..8 { coefficients[v * 8 + u] = (0..32).map(|y| horizontal[y][u] * basis[v][y]).sum(); }
    }
    coefficients
}

fn phash_from_gray(gray: &[f64; 1024]) -> u64 {
    let coefficients = dct_low_coefficients(gray);
    let mut ac = coefficients[1..].to_vec();
    ac.sort_unstable_by(f64::total_cmp);
    let median = ac[31];
    coefficients.iter().enumerate().skip(1).fold(0, |bits, (index, value)| {
        if *value > median { bits | (1_u64 << index) } else { bits }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{codecs::{gif::GifEncoder, jpeg::JpegEncoder}, Frame, Rgb, Rgba, RgbaImage};
    use std::{io::Cursor, path::PathBuf, sync::atomic::{AtomicU64, Ordering}};

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            static SERIAL: AtomicU64 = AtomicU64::new(0);
            let base = std::env::var_os("ILLUTAG_TEST_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
            let path = base.join(format!("illutag-fingerprint-{}-{}", std::process::id(), SERIAL.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir_all(&path).unwrap(); Self(path)
        }
        fn file(&self, name: &str) -> PathBuf { self.0.join(name) }
    }
    impl Drop for Scratch { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

    fn sample() -> RgbImage {
        RgbImage::from_fn(160, 120, |x, y| {
            let inside = x > 30 && x < 110 && y > 20 && y < 100;
            let circle = (i64::from(x) - 90).pow(2) + (i64::from(y) - 45).pow(2) < 400;
            if circle { Rgb([230, 50, 65]) } else if inside { Rgb([40, 130, 200]) } else { Rgb([(x / 2) as u8 + 100, (y / 3) as u8 + 110, 190]) }
        })
    }

    #[test]
    fn bytes_are_hashed_even_when_decode_fails_and_cancellation_is_propagated() {
        let scratch = Scratch::new(); let path = scratch.file("broken.png");
        std::fs::write(&path, b"not an image").unwrap();
        let value = fingerprint(&path, &mut || Ok(())).unwrap();
        assert_eq!(value.sha256.as_slice(), Sha256::digest(b"not an image").as_slice());
        assert_eq!(value.skip_reason.as_deref(), Some("decode_error"));
        assert_eq!(fingerprint(&path, &mut || Err("cancelled".into())).unwrap_err(), "cancelled");
    }

    #[test]
    fn dct_is_orthonormal_and_dc_is_excluded_from_phash() {
        let constant = [200.0; 1024];
        let coefficients = dct_low_coefficients(&constant);
        assert!((coefficients[0] - 6400.0).abs() < 1e-8);
        assert!(coefficients[1..].iter().all(|value| value.abs() < 1e-8));
        let basis_image = std::array::from_fn(|index| {
            let (x, y) = (index % 32, index / 32);
            let x_basis = (2.0_f64 / 32.0).sqrt() * (std::f64::consts::PI * (2 * x + 1) as f64 * 3.0 / 64.0).cos();
            let y_basis = (2.0_f64 / 32.0).sqrt() * (std::f64::consts::PI * (2 * y + 1) as f64 * 2.0 / 64.0).cos();
            x_basis * y_basis * 400.0
        });
        let coefficients = dct_low_coefficients(&basis_image);
        assert!((coefficients[2 * 8 + 3] - 400.0).abs() < 1e-8);
        assert_eq!(phash_from_gray(&constant) & 1, 0);
    }

    #[test]
    fn copied_bytes_have_same_hash_but_distinct_identity() {
        let scratch = Scratch::new(); let a = scratch.file("a.png"); let b = scratch.file("renamed.png");
        sample().save(&a).unwrap(); std::fs::copy(&a, &b).unwrap();
        let a = fingerprint(&a, &mut || Ok(())).unwrap(); let b = fingerprint(&b, &mut || Ok(())).unwrap();
        assert_eq!(a.sha256, b.sha256); assert_eq!(a.near, b.near);
        assert_ne!(a.signature.identity, b.signature.identity);
    }

    #[test]
    fn white_alpha_composite_matches_opaque_white() {
        let transparent = RgbaImage::from_pixel(12, 8, Rgba([17, 88, 199, 0]));
        let opaque = RgbImage::from_pixel(12, 8, Rgb([255, 255, 255]));
        let a = near_from_image(transparent.into(), Orientation::NoTransforms, &mut || Ok(())).unwrap();
        let b = near_from_image(opaque.into(), Orientation::NoTransforms, &mut || Ok(())).unwrap();
        assert_eq!(a, b); assert!(a.low_information); assert_eq!(a.phash & 1, 0);
    }

    #[test]
    fn resize_jpeg_and_small_color_change_keep_close_fingerprints() {
        let original = sample();
        let a = near_from_image(original.clone().into(), Orientation::NoTransforms, &mut || Ok(())).unwrap();
        let resized = image::imageops::resize(&original, 320, 240, FilterType::Triangle);
        let b = near_from_image(resized.into(), Orientation::NoTransforms, &mut || Ok(())).unwrap();
        assert!((a.phash ^ b.phash).count_ones() <= 4);
        assert!(super::super::index::compare_near(&a, &b, super::super::index::Threshold::Standard).is_some(), "resize must pass the complete standard filter");
        let mut bytes = Vec::new(); JpegEncoder::new_with_quality(&mut bytes, 75).encode_image(&original).unwrap();
        let c = near_from_image(image::load_from_memory(&bytes).unwrap(), Orientation::NoTransforms, &mut || Ok(())).unwrap();
        assert!((a.phash ^ c.phash).count_ones() <= 4);
        assert!(super::super::index::compare_near(&a, &c, super::super::index::Threshold::Standard).is_some(), "JPEG recompression must pass the complete standard filter");
        let changed = RgbImage::from_fn(160, 120, |x, y| { let mut p = *original.get_pixel(x, y); for c in &mut p.0 { *c = c.saturating_add(4); } p });
        let d = near_from_image(changed.into(), Orientation::NoTransforms, &mut || Ok(())).unwrap();
        assert!((a.phash ^ d.phash).count_ones() <= 4);
        assert!(super::super::index::compare_near(&a, &d, super::super::index::Threshold::Standard).is_some(), "small color changes must pass the complete standard filter");
        assert!(!a.low_information);
    }

    #[test]
    fn exif_orientation_normalizes_the_visual_pixels_and_dimensions() {
        let original = sample();
        let sideways = image::imageops::rotate270(&original);
        let a = near_from_image(original.into(), Orientation::NoTransforms, &mut || Ok(())).unwrap();
        let b = near_from_image(sideways.into(), Orientation::Rotate90, &mut || Ok(())).unwrap();
        assert_eq!(a, b);
        // A tiny valid little-endian TIFF IFD containing orientation 6.
        let exif = [b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 1, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(Orientation::from_exif_chunk(&exif), Some(Orientation::Rotate90));
    }

    #[test]
    fn animated_gif_and_webp_are_not_near_matches_of_first_frame() {
        let scratch = Scratch::new(); let path = scratch.file("animation.gif");
        let mut bytes = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut bytes);
            encoder.encode_frames([Frame::new(RgbaImage::from_pixel(4, 4, Rgba([0, 0, 0, 255]))), Frame::new(RgbaImage::from_pixel(4, 4, Rgba([255, 255, 255, 255])))].into_iter()).unwrap();
        }
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(fingerprint(&path, &mut || Ok(())).unwrap().skip_reason.as_deref(), Some("animated"));
        let path = scratch.file("animation.webp");
        let mut bytes = Vec::new(); bytes.extend_from_slice(b"RIFF"); bytes.extend_from_slice(&22_u32.to_le_bytes()); bytes.extend_from_slice(b"WEBPVP8X"); bytes.extend_from_slice(&10_u32.to_le_bytes()); bytes.extend_from_slice(&[2, 0, 0, 0, 3, 0, 0, 3, 0, 0]);
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(fingerprint(&path, &mut || Ok(())).unwrap().skip_reason.as_deref(), Some("animated"));
    }

    #[test]
    fn oversized_header_retains_file_hash_without_allocating_the_pixels() {
        let scratch = Scratch::new(); let path = scratch.file("huge.jpg");
        let mut jpeg = Vec::new(); JpegEncoder::new_with_quality(&mut jpeg, 90).encode_image(&sample()).unwrap();
        let sof = jpeg.windows(2).position(|bytes| bytes == [0xff, 0xc0]).unwrap();
        // SOF length(2), precision(1), height(2), width(2). No large fixture is written.
        jpeg[sof + 5..sof + 7].copy_from_slice(&4001_u16.to_be_bytes());
        jpeg[sof + 7..sof + 9].copy_from_slice(&8000_u16.to_be_bytes());
        std::fs::write(&path, &jpeg).unwrap();
        let result = fingerprint(&path, &mut || Ok(())).unwrap();
        assert_eq!(result.skip_reason.as_deref(), Some("pixel_limit"));
        assert_eq!(result.sha256.as_slice(), Sha256::digest(&jpeg).as_slice());
    }

    #[test]
    fn actual_jpeg_exif_is_applied_before_fingerprinting() {
        let scratch = Scratch::new();
        let sideways = image::imageops::rotate270(&sample());
        let mut jpeg = Vec::new(); JpegEncoder::new_with_quality(&mut jpeg, 98).encode_image(&sideways).unwrap();
        let tiff = [b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 1, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0];
        let mut exif = b"Exif\0\0".to_vec(); exif.extend_from_slice(&tiff);
        let mut with_exif = jpeg[..2].to_vec(); with_exif.extend_from_slice(&[0xff, 0xe1]); with_exif.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes()); with_exif.extend_from_slice(&exif); with_exif.extend_from_slice(&jpeg[2..]);
        let path = scratch.file("oriented.jpg"); std::fs::write(&path, &with_exif).unwrap();
        let actual = fingerprint(&path, &mut || Ok(())).unwrap().near.unwrap();
        let decoded = ImageReader::new(Cursor::new(jpeg)).with_guessed_format().unwrap().decode().unwrap();
        let expected = near_from_image(decoded, Orientation::Rotate90, &mut || Ok(())).unwrap();
        assert_eq!(actual, expected); assert_eq!((actual.width, actual.height), (160, 120));
    }

    #[test]
    fn replacing_the_source_during_hashing_is_rejected() {
        let scratch = Scratch::new(); let path = scratch.file("changing.png");
        sample().save(&path).unwrap();
        let mut calls = 0;
        let result = fingerprint(&path, &mut || { calls += 1; if calls == 3 { std::fs::write(&path, b"changed").unwrap(); } Ok(()) });
        assert_eq!(result.unwrap_err(), "source_changed");
    }

    #[test]
    fn pathname_swap_and_restore_cannot_mix_byte_hash_and_another_images_pixels() {
        let scratch = Scratch::new();
        let path = scratch.file("source.png");
        let original_aside = scratch.file("original-aside.png");
        let other = scratch.file("other.png");
        sample().save(&path).unwrap();
        RgbImage::from_pixel(30, 20, Rgb([80, 10, 200])).save(&other).unwrap();
        let expected = fingerprint(&path, &mut || Ok(())).unwrap();
        let mut calls = 0;
        let actual = fingerprint(&path, &mut || {
            calls += 1;
            if calls == 4 {
                std::fs::rename(&path, &original_aside).unwrap();
                std::fs::rename(&other, &path).unwrap();
            } else if calls == 5 {
                std::fs::rename(&path, &other).unwrap();
                std::fs::rename(&original_aside, &path).unwrap();
            }
            Ok(())
        }).unwrap();
        assert_eq!(actual.sha256, expected.sha256);
        assert_eq!(actual.near, expected.near);
        assert_eq!(actual.signature, expected.signature);
    }
}
