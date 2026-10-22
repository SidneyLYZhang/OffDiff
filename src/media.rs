//! Embedded media: content-addressed by SHA-256 so a replaced image shows up as
//! a hash change instead of leaking bytes into the JSON artifact.

use std::collections::BTreeMap;
use std::io::{Read, Seek};

use sha2::{Digest, Sha256};

use crate::error::{OffDiffError, Result};
use crate::model::Media;
use crate::ooxml::Package;

/// Upper bound on the bytes kept in memory for one document's media.
const MAX_TOTAL_BYTES: usize = 512 * 1024 * 1024;

#[derive(Debug, Default)]
pub struct MediaStore {
    meta: BTreeMap<String, Media>,
    bytes: BTreeMap<String, Vec<u8>>,
    total: usize,
}

impl MediaStore {
    pub fn new() -> Self {
        MediaStore::default()
    }

    /// Read a media part, hash it, and return its content address.
    pub fn add<R: Read + Seek>(&mut self, package: &mut Package<R>, part: &str) -> Result<String> {
        let data = package.read(part)?;
        let (width, height) = image_size(&data);
        self.add_bytes(data, mime_for(part).to_string(), width, height)
    }

    /// Register already-decoded media bytes (used by the PDF backend).
    pub fn add_bytes(
        &mut self,
        data: Vec<u8>,
        mime: String,
        width: Option<u32>,
        height: Option<u32>,
    ) -> Result<String> {
        let sha = sha256_hex(&data);
        if self.meta.contains_key(&sha) {
            return Ok(sha);
        }
        if self.total.saturating_add(data.len()) > MAX_TOTAL_BYTES {
            return Err(OffDiffError::Corrupt(format!(
                "embedded media exceeds {MAX_TOTAL_BYTES} bytes"
            )));
        }
        self.meta.insert(
            sha.clone(),
            Media {
                mime,
                bytes: data.len() as u64,
                width,
                height,
                data: None,
            },
        );
        self.total += data.len();
        self.bytes.insert(sha.clone(), data);
        Ok(sha)
    }

    pub fn into_parts(self) -> (BTreeMap<String, Media>, BTreeMap<String, Vec<u8>>) {
        (self.meta, self.bytes)
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    use std::fmt::Write as _;

    let digest = Sha256::digest(data);
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

pub fn mime_for(part: &str) -> &'static str {
    let extension = part
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "emf" => "image/emf",
        "wmf" => "image/wmf",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

/// Best-effort intrinsic pixel size for the formats Word and PowerPoint embed
/// most often. Returns `None` for vector formats and unknown bytes.
pub fn image_size(data: &[u8]) -> (Option<u32>, Option<u32>) {
    if let Some(size) = png_size(data) {
        return size;
    }
    if let Some(size) = gif_size(data) {
        return size;
    }
    if let Some(size) = bmp_size(data) {
        return size;
    }
    if let Some(size) = jpeg_size(data) {
        return size;
    }
    (None, None)
}

fn png_size(data: &[u8]) -> Option<(Option<u32>, Option<u32>)> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if data.len() < 24 || data[..8] != SIGNATURE || &data[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    Some((Some(width), Some(height)))
}

fn gif_size(data: &[u8]) -> Option<(Option<u32>, Option<u32>)> {
    if data.len() < 10 || &data[..4] != b"GIF8" {
        return None;
    }
    let width = u16::from_le_bytes([data[6], data[7]]) as u32;
    let height = u16::from_le_bytes([data[8], data[9]]) as u32;
    Some((Some(width), Some(height)))
}

fn bmp_size(data: &[u8]) -> Option<(Option<u32>, Option<u32>)> {
    if data.len() < 26 || &data[..2] != b"BM" {
        return None;
    }
    let width = i32::from_le_bytes([data[18], data[19], data[20], data[21]]).unsigned_abs();
    let height = i32::from_le_bytes([data[22], data[23], data[24], data[25]]).unsigned_abs();
    Some((Some(width), Some(height)))
}

fn jpeg_size(data: &[u8]) -> Option<(Option<u32>, Option<u32>)> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }
    let mut offset = 2usize;
    while offset + 9 < data.len() {
        if data[offset] != 0xFF {
            offset += 1;
            continue;
        }
        let marker = data[offset + 1];
        if (0xD0..=0xD9).contains(&marker) || marker == 0x01 {
            offset += 2;
            continue;
        }
        let length = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize;
        if length < 2 {
            return None;
        }
        let is_sof = matches!(
            marker,
            0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF
        );
        if is_sof {
            if offset + 9 > data.len() {
                return None;
            }
            let height = u16::from_be_bytes([data[offset + 5], data[offset + 6]]) as u32;
            let width = u16::from_be_bytes([data[offset + 7], data[offset + 8]]) as u32;
            return Some((Some(width), Some(height)));
        }
        offset += 2 + length;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_stable_and_lowercase_hex() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(sha256_hex(b"abc").len(), 64);
    }

    #[test]
    fn reads_png_dimensions() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&640u32.to_be_bytes());
        png.extend_from_slice(&480u32.to_be_bytes());
        assert_eq!(image_size(&png), (Some(640), Some(480)));
    }

    #[test]
    fn reads_gif_dimensions() {
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&320u16.to_le_bytes());
        gif.extend_from_slice(&200u16.to_le_bytes());
        assert_eq!(image_size(&gif), (Some(320), Some(200)));
    }

    #[test]
    fn reads_jpeg_dimensions() {
        let mut jpeg = vec![0xFF, 0xD8];
        jpeg.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]);
        jpeg.extend_from_slice(&[0u8; 14]);
        jpeg.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
        jpeg.extend_from_slice(&300u16.to_be_bytes());
        jpeg.extend_from_slice(&400u16.to_be_bytes());
        jpeg.extend_from_slice(&[0u8; 8]);
        assert_eq!(image_size(&jpeg), (Some(400), Some(300)));
    }

    #[test]
    fn unknown_media_has_no_dimensions() {
        assert_eq!(image_size(b"not an image"), (None, None));
        assert_eq!(mime_for("word/media/image1.png"), "image/png");
        assert_eq!(mime_for("word/media/blob.bin"), "application/octet-stream");
    }
}
