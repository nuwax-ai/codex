//! Bounded header inspection for original-detail visual accounting.

use std::io::Cursor;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use image::ImageFormat;
use image::ImageReader;
use image::Limits;

use crate::MAX_PROMPT_IMAGE_PIXELS;
use crate::PROMPT_IMAGE_PATCH_SIZE;

const MAX_ESTIMATE_DATA_URL_BYTES: usize = 16 * 1024 * 1024;
const MAX_ESTIMATE_DECODED_BYTES: usize = 12 * 1024 * 1024;
const MAX_ESTIMATE_METADATA_BYTES: usize = 8 * 1024;
const MAX_ESTIMATE_DECODER_ALLOC_BYTES: u64 = 1024 * 1024;
const ORIGINAL_PATCH_CEILING: u64 = 10_000;

/// Estimates original-detail vision tokens from image headers, without decoding pixels.
///
/// Only bounded Base64 data URLs in supported image formats are inspected. A missing
/// estimate means callers must charge the conservative 10,000-patch ceiling; it does
/// not make an unknown image free. PNG inspection stops at IHDR without inflating
/// or interpreting later metadata; other formats use the bounded header decoder.
pub fn estimate_original_image_tokens(image_url: &str) -> Option<usize> {
    if image_url.len() > MAX_ESTIMATE_DATA_URL_BYTES
        || !image_url.get(..5)?.eq_ignore_ascii_case("data:")
    {
        return None;
    }
    let (metadata, encoded) = image_url.split_once(',')?;
    if metadata.len() > MAX_ESTIMATE_METADATA_BYTES
        || !metadata
            .split(';')
            .any(|part| part.eq_ignore_ascii_case("base64"))
        || encoded.len().div_ceil(4).checked_mul(3)? > MAX_ESTIMATE_DECODED_BYTES
    {
        return None;
    }
    let bytes = BASE64_STANDARD.decode(encoded).ok()?;
    if bytes.len() > MAX_ESTIMATE_DECODED_BYTES {
        return None;
    }
    let format = image::guess_format(&bytes).ok()?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP
    ) {
        return None;
    }
    let (width, height) = if format == ImageFormat::Png {
        let mut decoder = png::Decoder::new_with_limits(
            Cursor::new(bytes),
            png::Limits {
                bytes: usize::try_from(MAX_ESTIMATE_DECODER_ALLOC_BYTES).ok()?,
            },
        );
        // read_info/into_dimensions also visit ancillary chunks. The budget only
        // needs IHDR, so do not parse or decompress ICC/EXIF metadata here.
        let info = decoder.read_header_info().ok()?;
        (info.width, info.height)
    } else {
        let mut limits = Limits::default();
        limits.max_alloc = Some(MAX_ESTIMATE_DECODER_ALLOC_BYTES);
        let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
        reader.limits(limits);
        // Header inspection never constructs a DynamicImage or raster.
        reader.into_dimensions().ok()?
    };
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PROMPT_IMAGE_PIXELS {
        return None;
    }
    let patches = u64::from(width.div_ceil(PROMPT_IMAGE_PATCH_SIZE))
        * u64::from(height.div_ceil(PROMPT_IMAGE_PATCH_SIZE));
    usize::try_from(patches.min(ORIGINAL_PATCH_CEILING)).ok()
}
