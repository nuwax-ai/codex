use std::io::Cursor;
use std::num::NonZeroUsize;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::LazyLock;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use codex_utils_cache::BlockingLruCache;
use codex_utils_cache::sha1_digest;
use image::ColorType;
use image::DynamicImage;
use image::GenericImageView;
use image::ImageDecoder;
use image::ImageEncoder;
use image::ImageFormat;
use image::ImageReader;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::codecs::webp::WebPEncoder;
use image::imageops::FilterType;

const DATA_URL_PREFIX: &str = "data:";
pub const PROMPT_IMAGE_PATCH_SIZE: u32 = 32;
/// Maximum width or height used when resizing images before uploading.
pub const MAX_DIMENSION: u32 = 2048;

/// Upper bound on the pixels one prompt image may declare, checked from the
/// format header BEFORE the full decode allocates width × height × channels.
/// This decode-safety guard permits 6000 × 6000 (36 MP) originals while
/// rejecting larger sources, including otherwise valid photos above 40 MP.
pub const MAX_PROMPT_IMAGE_PIXELS: u64 = 40_000_000;

/// Wire ceiling for one prepared image's Base64 data URL and request headroom.
/// Check its serialized representation rather than just the smaller binary
/// source so an image cannot individually exceed the whole-request limit.
pub const MAX_PREPARED_IMAGE_BYTES: usize = 32 * 1024 * 1024;
/// Space for request framing and ordinary text around a single inline image.
const PREPARED_IMAGE_REQUEST_HEADROOM_BYTES: usize = 1024 * 1024;
/// Maximum accepted byte length for prompt image input representations.
///
/// This is a high sanity guard against pathological inputs, not a protocol
/// requirement or target upload size.
pub const MAX_PROMPT_IMAGE_INPUT_BYTES: usize = 1024 * 1024 * 1024;
const MAX_IMAGE_CACHE_BYTES: usize = 64 * 1024 * 1024;

pub mod error;
mod original_image_tokens;

pub use crate::error::ImageProcessingError;
pub use original_image_tokens::estimate_original_image_tokens;

#[derive(Debug, Clone)]
pub struct EncodedImage {
    pub bytes: Arc<[u8]>,
    pub mime: String,
    pub source_width: u32,
    pub source_height: u32,
    pub width: u32,
    pub height: u32,
}

impl EncodedImage {
    pub fn into_data_url(self) -> String {
        data_url_from_bytes(&self.mime, &self.bytes)
    }
}

/// Wraps image bytes in a data URL without decoding or validating them.
pub fn data_url_from_bytes(mime: &str, bytes: &[u8]) -> String {
    let encoded = BASE64_STANDARD.encode(bytes);
    format!("data:{mime};base64,{encoded}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptImageMode {
    ResizeToFit,
    Original,
    ResizeWithLimits(PromptImageResizeLimits),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PromptImageResizeLimits {
    pub max_dimension: u32,
    pub max_patches: usize,
}

impl PromptImageMode {
    /// Resize policy for high-detail prompt images.
    pub const HIGH_DETAIL: Self = Self::ResizeWithLimits(PromptImageResizeLimits {
        max_dimension: 2048,
        max_patches: 2_500,
    });
    /// Resize policy for original-detail prompt images.
    pub const ORIGINAL_DETAIL: Self = Self::ResizeWithLimits(PromptImageResizeLimits {
        max_dimension: 6000,
        max_patches: 10_000,
    });
}

struct ImageMetadata {
    icc_profile: Option<Vec<u8>>,
    exif: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ImageCacheKey {
    digest: [u8; 20],
    mode: PromptImageMode,
}

type ImageCache = BlockingLruCache<ImageCacheKey, EncodedImage>;

static IMAGE_CACHE: LazyLock<ImageCache> =
    LazyLock::new(|| BlockingLruCache::new(NonZeroUsize::new(32).unwrap_or(NonZeroUsize::MIN)));

pub fn load_for_prompt_bytes(
    path: &Path,
    file_bytes: Vec<u8>,
    mode: PromptImageMode,
) -> Result<EncodedImage, ImageProcessingError> {
    let path_buf = path.to_path_buf();

    let key = ImageCacheKey {
        digest: sha1_digest(&file_bytes),
        mode,
    };

    if let Some(image) = IMAGE_CACHE.get(&key) {
        return Ok(image);
    }

    let image = load_for_prompt_bytes_uncached(&path_buf, file_bytes, mode)?;
    cache_image(&IMAGE_CACHE, key, image.clone(), MAX_IMAGE_CACHE_BYTES);
    Ok(image)
}

fn load_for_prompt_bytes_uncached(
    path: &Path,
    file_bytes: Vec<u8>,
    mode: PromptImageMode,
) -> Result<EncodedImage, ImageProcessingError> {
    let path_buf = path.to_path_buf();
    (move || {
        let guessed_format = image::guess_format(&file_bytes)
            .map_err(|source| ImageProcessingError::decode_error(&path_buf, source))?;
        let format = match guessed_format {
            ImageFormat::Png => Some(ImageFormat::Png),
            ImageFormat::Jpeg => Some(ImageFormat::Jpeg),
            ImageFormat::Gif => Some(ImageFormat::Gif),
            ImageFormat::WebP => Some(ImageFormat::WebP),
            _ => None,
        };

        let mut decoder = ImageReader::with_format(Cursor::new(&file_bytes), guessed_format)
            .into_decoder()
            .map_err(|source| ImageProcessingError::decode_error(&path_buf, source))?;
        // Reject a declared pixel count the full decode could never afford
        // before any raster allocation happens. The header dimensions are
        // cheap to read; a bomb declares far more pixels than it encodes.
        {
            let (width, height) = decoder.dimensions();
            if u64::from(width) * u64::from(height) > MAX_PROMPT_IMAGE_PIXELS {
                return Err(ImageProcessingError::ImageTooLarge {
                    representation: "declared pixels",
                    size: (u64::from(width) * u64::from(height))
                        .try_into()
                        .unwrap_or(usize::MAX),
                    max: usize::try_from(MAX_PROMPT_IMAGE_PIXELS).unwrap_or(usize::MAX),
                });
            }
        }
        // Preserve the metadata most important for rendering prompt images faithfully: the color
        // profile and EXIF data, including orientation. Other format-specific metadata is
        // intentionally not copied.
        let metadata = ImageMetadata {
            // Only RGB profiles are safe across every re-encoding path. For example, JPEG decoding
            // can convert CMYK/YCCK pixels to RGB while retaining the source profile; copying it
            // would mislabel the output. Bytes 16..20 are the ICC data color space signature.
            icc_profile: decoder
                .icc_profile()
                .ok()
                .flatten()
                .filter(|profile| profile.get(16..20) == Some(b"RGB ")),
            exif: decoder.exif_metadata().ok().flatten(),
        };
        let dynamic = DynamicImage::from_decoder(decoder)
            .map_err(|source| ImageProcessingError::decode_error(&path_buf, source))?;

        let (width, height) = dynamic.dimensions();

        let target_dimensions = match mode {
            PromptImageMode::ResizeToFit if width > MAX_DIMENSION || height > MAX_DIMENSION => {
                let resized = dynamic.resize(MAX_DIMENSION, MAX_DIMENSION, FilterType::Triangle);
                Some((resized.width(), resized.height(), resized))
            }
            PromptImageMode::ResizeWithLimits(limits) => {
                let (target_width, target_height) =
                    prompt_image_output_dimensions_for_limits(width, height, limits);
                if (target_width, target_height) == (width, height) {
                    None
                } else {
                    let resized =
                        dynamic.resize_exact(target_width, target_height, FilterType::Triangle);
                    Some((target_width, target_height, resized))
                }
            }
            PromptImageMode::ResizeToFit | PromptImageMode::Original => None,
        };

        let encoded = if let Some((prepared_width, prepared_height, resized)) = target_dimensions {
            let target_format = format
                .filter(|format| can_preserve_source_bytes(*format))
                .unwrap_or(ImageFormat::Png);
            let (bytes, output_format) = encode_image(&resized, target_format, metadata)?;
            let mime = format_to_mime(output_format);
            EncodedImage {
                bytes: bytes.into(),
                mime,
                source_width: width,
                source_height: height,
                width: prepared_width,
                height: prepared_height,
            }
        } else {
            if let Some(format) = format.filter(|format| can_preserve_source_bytes(*format)) {
                let mime = format_to_mime(format);
                EncodedImage {
                    bytes: file_bytes.into(),
                    mime,
                    source_width: width,
                    source_height: height,
                    width,
                    height,
                }
            } else {
                let (bytes, output_format) = encode_image(&dynamic, ImageFormat::Png, metadata)?;
                let mime = format_to_mime(output_format);
                EncodedImage {
                    bytes: bytes.into(),
                    mime,
                    source_width: width,
                    source_height: height,
                    width,
                    height,
                }
            }
        };

        admit_encoded_image(encoded)
    })()
}



/// Fork (nuwax-codex, 2026-11-10): JPEG quality ladder applied when an encoded
/// image must shrink to fit a byte budget. The first rung that fits wins; if
/// none fits, the smallest (last) rung is kept so the budget path always has
/// an output. Quality is applied only to lossy re-encodes — never to a
/// pass-through of bytes that already fit.
pub const IMAGE_BYTE_BUDGET_QUALITY_LADDER: [u8; 3] = [85, 75, 65];

/// Downscale ladder for images that must keep their alpha channel (JPEG has
/// none). Applied longest-edge-first; the first rung that fits wins.
const ALPHA_PRESERVING_DIMENSION_LADDER: [u32; 3] = [1600, 1280, 1024];

/// Longest-edge caps tried after the quality ladder alone cannot reach the
/// budget. Quality first, then size: a screenshot of UI text stays readable
/// far longer at a lower quality than at a smaller size (Fork 2026-11-10).
const BYTE_BUDGET_DIMENSION_LADDER: [u32; 4] = [2048, 1600, 1280, 1024];

/// True when every pixel's alpha is fully opaque. Screenshot tools commonly
/// emit 32-bit RGBA even for opaque content; those images can be flattened to
/// RGB and JPEG-encoded with no visual loss.
pub fn alpha_is_fully_opaque(image: &DynamicImage) -> bool {
    let rgba = image.to_rgba8();
    rgba.pixels().all(|pixel| pixel.0[3] == 255)
}

/// Encodes `image` as JPEG at `quality` (alpha is dropped), returning the
/// encoded bytes without any metadata side channel.
fn encode_jpeg_at_quality(
    image: &DynamicImage,
    quality: u8,
) -> Result<Vec<u8>, ImageProcessingError> {
    let mut buffer = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut buffer, quality);
    encoder
        .encode_image(image)
        .map_err(|source| ImageProcessingError::Encode {
            format: ImageFormat::Jpeg,
            source,
        })?;
    Ok(buffer)
}

/// Encodes `image` as PNG (alpha preserved).
fn encode_png(image: &DynamicImage) -> Result<Vec<u8>, ImageProcessingError> {
    let mut buffer = Vec::new();
    let rgba = image.to_rgba8();
    let encoder = PngEncoder::new(&mut buffer);
    encoder
        .write_image(
            rgba.as_raw(),
            image.width(),
            image.height(),
            ColorType::Rgba8.into(),
        )
        .map_err(|source| ImageProcessingError::Encode {
            format: ImageFormat::Png,
            source,
        })?;
    Ok(buffer)
}

/// Scales `image` so its longest edge is at most `max_dimension`, leaving
/// smaller images untouched.
fn resize_to_longest_edge(image: &DynamicImage, max_dimension: u32) -> DynamicImage {
    let (width, height) = image.dimensions();
    let longest_edge = width.max(height);
    if longest_edge <= max_dimension {
        return image.clone();
    }
    let scale = f64::from(max_dimension) / f64::from(longest_edge);
    let scaled_width = ((f64::from(width) * scale).round() as u32).max(1);
    let scaled_height = ((f64::from(height) * scale).round() as u32).max(1);
    image.resize_exact(scaled_width, scaled_height, FilterType::Triangle)
}

/// Caps the longest edge of a data-URL image at `max_edge`, re-encoding only
/// when the image is actually larger. Returns `None` when nothing changed.
///
/// Fork (2026-11-10): `detail: Original` images are charged one token per
/// 16x16 patch, so a 4000x4000 screenshot costs the whole per-output visual
/// budget no matter how few bytes it has. Shrinking bytes alone therefore does
/// not make such an image admissible — the pixels must come down too.
pub fn cap_data_url_dimensions(
    image_url: &str,
    max_edge: u32,
) -> Result<Option<String>, ImageProcessingError> {
    let decoded = decode_data_url_to_dynamic(image_url)?;
    let (width, height) = decoded.dimensions();
    if width.max(height) <= max_edge {
        return Ok(None);
    }
    let resized = resize_to_longest_edge(&decoded, max_edge);
    // Keep the format family: opaque images take the JPEG path (no alpha to
    // lose), genuinely transparent ones stay PNG.
    let (bytes, format) = if alpha_is_fully_opaque(&resized) {
        let quality = *IMAGE_BYTE_BUDGET_QUALITY_LADDER
            .last()
            .unwrap_or(&IMAGE_BYTE_BUDGET_QUALITY_LADDER[0]);
        (encode_jpeg_at_quality(&resized, quality)?, ImageFormat::Jpeg)
    } else {
        (encode_png(&resized)?, ImageFormat::Png)
    };
    Ok(Some(data_url_from_bytes(&format_to_mime(format), &bytes)))
}

/// The longest edge that keeps a square-ish image inside `budget_tokens`
/// patches, with headroom for the estimator's ceiling and control overhead.
pub fn longest_edge_for_visual_budget(budget_tokens: usize) -> u32 {
    let patches = (budget_tokens as f64) * 0.8;
    let edge = (PROMPT_IMAGE_PATCH_SIZE as f64) * patches.sqrt();
    (edge.floor() as u32).max(256)
}

/// Shrinks an already-prepared image to fit `max_bytes`, lossless-first
/// (Fork 2026-11-10, see FORK.md §5.5):
///
/// 1. Fully-opaque images (including RGBA screenshots whose alpha is all 255)
///    are flattened and JPEG-laddered at 85/75/65 — screenshots of UI and text
///    shrink 5-10x with no visible change.
/// 2. Genuinely transparent images keep PNG and walk the dimension ladder
///    (2048 -> 1600 -> 1280 -> 1024 longest edge), preserving alpha.
/// 3. If nothing fits, the smallest produced candidate is returned; callers
///    treat it as best-effort.
///
/// Returns `None` when the source bytes already fit, so the caller can pass
/// the original data URL through untouched.
pub fn fit_data_url_to_byte_budget(
    image_url: &str,
    max_bytes: usize,
) -> Result<(String, Vec<u8>, String), ImageProcessingError> {
    let decoded = decode_data_url_to_dynamic(image_url)?;
    let (bytes, format) = match fit_dynamic_to_byte_budget(&decoded, max_bytes)? {
        Some(pair) => pair,
        // Already fits on the ladder's first rung: hand the caller back the
        // source untouched rather than re-encoding for nothing.
        None => return Ok((image_url.to_string(), decoded_bytes(image_url), image_url_mime(image_url))),
    };
    let mime = format_to_mime(format);
    Ok((
        data_url_from_bytes(&mime, &bytes),
        bytes,
        mime,
    ))
}

/// Decodes the base64 payload of a data URL (best-effort; callers already
/// validated the URL before reaching the ladder).
fn decoded_bytes(image_url: &str) -> Vec<u8> {
    image_url
        .split_once(',')
        .map(|(_, encoded)| encoded)
        .map(|encoded| BASE64_STANDARD.decode(encoded).unwrap_or_default())
        .unwrap_or_default()
}

/// Extracts the mime type of a `data:<mime>;base64,<payload>` URL.
fn image_url_mime(image_url: &str) -> String {
    image_url
        .split_once(',')
        .and_then(|(metadata, _)| metadata.split_once(':'))
        .map(|(_, mime)| mime.to_string())
        .unwrap_or_else(|| "image/png".to_string())
}

/// Reads an image's declared dimensions without decoding the raster, so a
/// pixel bomb can be refused before any allocation. `None` when the header
/// cannot be read at all.
fn header_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let format = image::guess_format(bytes).ok()?;
    if format == ImageFormat::Png {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let info = decoder.read_header_info().ok()?;
        return Some((info.width, info.height));
    }
    image::ImageReader::with_format(std::io::Cursor::new(bytes), format)
        .into_dimensions()
        .ok()
}

/// Decodes a `data:...;base64,...` URL into a `DynamicImage`. Only base64
/// data URLs are accepted; anything else is an error at this layer.
fn decode_data_url_to_dynamic(image_url: &str) -> Result<DynamicImage, ImageProcessingError> {
    let rest = image_url
        .get(..DATA_URL_PREFIX.len())
        .filter(|prefix| prefix.eq_ignore_ascii_case(DATA_URL_PREFIX))
        .and_then(|_| image_url.get(DATA_URL_PREFIX.len()..))
        .ok_or_else(|| ImageProcessingError::InvalidDataUrl {
            reason: "missing data: prefix".to_string(),
        })?;
    let (metadata, encoded) =
        rest.split_once(',').ok_or_else(|| ImageProcessingError::InvalidDataUrl {
            reason: "missing comma separator".to_string(),
        })?;
    if !metadata
        .split(';')
        .any(|part| part.eq_ignore_ascii_case("base64"))
    {
        return Err(ImageProcessingError::InvalidDataUrl {
            reason: "only base64 data URLs are supported".to_string(),
        });
    }
    let bytes = BASE64_STANDARD
        .decode(encoded)
        .map_err(|_| ImageProcessingError::InvalidDataUrl {
            reason: "invalid base64 payload".to_string(),
        })?;
    // Pixel-bomb pre-check on the header, before any raster allocation. The
    // tool-output path reaches this function with bytes no upstream guard has
    // seen, and a small file can declare a gigantic canvas. A header that
    // cannot be read is left to `load_from_memory`, which reports it properly.
    if let Some((width, height)) = header_dimensions(&bytes)
        && u64::from(width) * u64::from(height) > MAX_PROMPT_IMAGE_PIXELS
    {
        return Err(ImageProcessingError::ImageTooLarge {
            representation: "declared pixels",
            size: usize::try_from(u64::from(width) * u64::from(height)).unwrap_or(usize::MAX),
            max: usize::try_from(MAX_PROMPT_IMAGE_PIXELS).unwrap_or(usize::MAX),
        });
    }
    image::load_from_memory(&bytes).map_err(|source| ImageProcessingError::Decode {
        path: PathBuf::from("<data-url>"),
        source,
    })
}

/// The shared ladder over a decoded image. Returns `None` when the image
/// already fits at its current encoding path (opaque + quality ladder at the
/// first rung already small enough), or the winning `(bytes, format)`.
fn fit_dynamic_to_byte_budget(
    image: &DynamicImage,
    max_bytes: usize,
) -> Result<Option<(Vec<u8>, ImageFormat)>, ImageProcessingError> {
    if alpha_is_fully_opaque(image) {
        // Flatten to RGB via a JPEG encode; no visual information is lost
        // because the alpha channel carries none. Quality first at the source
        // size, then the dimension ladder crossed with the quality ladder.
        for quality in IMAGE_BYTE_BUDGET_QUALITY_LADDER {
            let bytes = encode_jpeg_at_quality(image, quality)?;
            if bytes.len() <= max_bytes {
                return Ok(Some((bytes, ImageFormat::Jpeg)));
            }
        }
        for max_dimension in BYTE_BUDGET_DIMENSION_LADDER {
            let resized = resize_to_longest_edge(image, max_dimension);
            for quality in IMAGE_BYTE_BUDGET_QUALITY_LADDER {
                let bytes = encode_jpeg_at_quality(&resized, quality)?;
                if bytes.len() <= max_bytes {
                    return Ok(Some((bytes, ImageFormat::Jpeg)));
                }
            }
        }
        // Nothing fits: keep the smallest rung so the caller still has an
        // output to compare against the original.
        let resized = resize_to_longest_edge(
            image,
            *BYTE_BUDGET_DIMENSION_LADDER.last().unwrap_or(&2048),
        );
        let smallest = *IMAGE_BYTE_BUDGET_QUALITY_LADDER
            .last()
            .unwrap_or(&IMAGE_BYTE_BUDGET_QUALITY_LADDER[0]);
        let bytes = encode_jpeg_at_quality(&resized, smallest)?;
        return Ok(Some((bytes, ImageFormat::Jpeg)));
    }

    for max_dimension in ALPHA_PRESERVING_DIMENSION_LADDER {
        let (width, height) = image.dimensions();
        let longest_edge = width.max(height);
        let candidate = if longest_edge > max_dimension {
            let scale = f64::from(max_dimension) / f64::from(longest_edge);
            let scaled_width = (f64::from(width) * scale).round() as u32;
            let scaled_height = (f64::from(height) * scale).round() as u32;
            image.resize_exact(scaled_width.max(1), scaled_height.max(1), FilterType::Triangle)
        } else {
            image.clone()
        };
        let bytes = encode_png(&candidate)?;
        if bytes.len() <= max_bytes {
            return Ok(Some((bytes, ImageFormat::Png)));
        }
    }
    let (width, height) = image.dimensions();
    let longest_edge = width.max(height);
    let scale = f64::from(ALPHA_PRESERVING_DIMENSION_LADDER
        [ALPHA_PRESERVING_DIMENSION_LADDER.len() - 1])
        / f64::from(longest_edge);
    let scaled_width = ((f64::from(width) * scale).round() as u32).max(1);
    let scaled_height = ((f64::from(height) * scale).round() as u32).max(1);
    let resized = image.resize_exact(scaled_width, scaled_height, FilterType::Triangle);
    let bytes = encode_png(&resized)?;
    Ok(Some((bytes, ImageFormat::Png)))
}

/// Refuse images whose data URL and request headroom exceed the wire ceiling,
/// giving an attributable image placeholder before constructing the request.
fn admit_encoded_image(encoded: EncodedImage) -> Result<EncodedImage, ImageProcessingError> {
    validate_prepared_image_size(&encoded.mime, encoded.bytes.len())?;
    Ok(encoded)
}

fn validate_prepared_image_size(mime: &str, byte_len: usize) -> Result<(), ImageProcessingError> {
    // Check the representation actually serialized onto the wire without
    // allocating a Base64 string or a raster just to measure it.
    let data_url_bytes = byte_len
        .checked_add(2)
        .and_then(|bytes| bytes.checked_div(3))
        .and_then(|groups| groups.checked_mul(4))
        .and_then(|encoded| encoded.checked_add("data:;base64,".len()))
        .and_then(|encoded| encoded.checked_add(mime.len()))
        .unwrap_or(usize::MAX);
    let max = MAX_PREPARED_IMAGE_BYTES - PREPARED_IMAGE_REQUEST_HEADROOM_BYTES;
    if data_url_bytes > max {
        return Err(ImageProcessingError::ImageTooLarge {
            representation: "prepared image data URL bytes",
            size: data_url_bytes,
            max,
        });
    }
    Ok(())
}

fn cache_image(cache: &ImageCache, key: ImageCacheKey, image: EncodedImage, byte_capacity: usize) {
    if image.bytes.len() > byte_capacity {
        return;
    }

    cache.with_mut(|cache| {
        cache.put(key, image);
        let mut cached_bytes = cache
            .iter()
            .map(|(_, image)| image.bytes.len())
            .sum::<usize>();
        while cached_bytes > byte_capacity {
            let Some((_, evicted)) = cache.pop_lru() else {
                break;
            };
            cached_bytes -= evicted.bytes.len();
        }
    });
}

pub fn load_data_url_for_prompt(
    image_url: &str,
    mode: PromptImageMode,
) -> Result<EncodedImage, ImageProcessingError> {
    load_data_url_for_prompt_with(image_url, mode, load_for_prompt_bytes)
}

pub fn load_data_url_for_prompt_uncached(
    image_url: &str,
    mode: PromptImageMode,
) -> Result<EncodedImage, ImageProcessingError> {
    load_data_url_for_prompt_with(image_url, mode, load_for_prompt_bytes_uncached)
}

fn load_data_url_for_prompt_with(
    image_url: &str,
    mode: PromptImageMode,
    load: impl FnOnce(&Path, Vec<u8>, PromptImageMode) -> Result<EncodedImage, ImageProcessingError>,
) -> Result<EncodedImage, ImageProcessingError> {
    let rest = image_url
        .get(..DATA_URL_PREFIX.len())
        .filter(|prefix| prefix.eq_ignore_ascii_case(DATA_URL_PREFIX))
        .and_then(|_| image_url.get(DATA_URL_PREFIX.len()..))
        .ok_or_else(|| ImageProcessingError::InvalidDataUrl {
            reason: "missing data: prefix".to_string(),
        })?;
    let (metadata, encoded) =
        rest.split_once(',')
            .ok_or_else(|| ImageProcessingError::InvalidDataUrl {
                reason: "missing comma separator".to_string(),
            })?;
    if !metadata
        .split(';')
        .any(|part| part.eq_ignore_ascii_case("base64"))
    {
        return Err(ImageProcessingError::InvalidDataUrl {
            reason: "only base64 data URLs are supported".to_string(),
        });
    }

    if encoded.len() > MAX_PROMPT_IMAGE_INPUT_BYTES {
        return Err(ImageProcessingError::ImageTooLarge {
            representation: "base64 payload",
            size: encoded.len(),
            max: MAX_PROMPT_IMAGE_INPUT_BYTES,
        });
    }
    let file_bytes =
        BASE64_STANDARD
            .decode(encoded)
            .map_err(|source| ImageProcessingError::InvalidDataUrl {
                reason: format!("invalid base64 payload: {source}"),
            })?;
    if file_bytes.len() > MAX_PROMPT_IMAGE_INPUT_BYTES {
        return Err(ImageProcessingError::ImageTooLarge {
            representation: "decoded input",
            size: file_bytes.len(),
            max: MAX_PROMPT_IMAGE_INPUT_BYTES,
        });
    }

    load(Path::new("<data-url-image>"), file_bytes, mode)
}

fn prompt_image_output_dimensions_for_limits(
    width: u32,
    height: u32,
    limits: PromptImageResizeLimits,
) -> (u32, u32) {
    let width = width.max(1);
    let height = height.max(1);
    if prompt_image_dimensions_fit(width, height, limits) {
        return (width, height);
    }

    let max_dimension_scale =
        (f64::from(limits.max_dimension) / f64::from(width.max(height))).min(1.0);
    let width = ((f64::from(width) * max_dimension_scale).round() as u32).max(1);
    let height = ((f64::from(height) * max_dimension_scale).round() as u32).max(1);
    if prompt_image_dimensions_fit(width, height, limits) {
        return (width, height);
    }

    let width_f64 = f64::from(width);
    let height_f64 = f64::from(height);
    let patch_size = f64::from(PROMPT_IMAGE_PATCH_SIZE);
    let mut scale =
        (patch_size * patch_size * limits.max_patches as f64 / width_f64 / height_f64).sqrt();
    // Match Responses patch-budget math: shrink by area, then round the scaled
    // patch grid down so integer output dimensions remain within the budget.
    let scaled_patches_wide = width_f64 * scale / patch_size;
    let scaled_patches_high = height_f64 * scale / patch_size;
    scale *= (scaled_patches_wide.floor() / scaled_patches_wide)
        .min(scaled_patches_high.floor() / scaled_patches_high);

    (
        ((width_f64 * scale).floor() as u32).max(1),
        ((height_f64 * scale).floor() as u32).max(1),
    )
}

fn prompt_image_dimensions_fit(width: u32, height: u32, limits: PromptImageResizeLimits) -> bool {
    let patches_wide = width.div_ceil(PROMPT_IMAGE_PATCH_SIZE);
    let patches_high = height.div_ceil(PROMPT_IMAGE_PATCH_SIZE);
    let patch_count = u64::from(patches_wide) * u64::from(patches_high);
    width <= limits.max_dimension
        && height <= limits.max_dimension
        && patch_count <= limits.max_patches as u64
}

fn can_preserve_source_bytes(format: ImageFormat) -> bool {
    // Public API docs explicitly call out non-animated GIF support only.
    // Preserve byte-for-byte only for formats we can safely pass through.
    matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP
    )
}

fn encode_image(
    image: &DynamicImage,
    preferred_format: ImageFormat,
    metadata: ImageMetadata,
) -> Result<(Vec<u8>, ImageFormat), ImageProcessingError> {
    let target_format = match preferred_format {
        ImageFormat::Jpeg => ImageFormat::Jpeg,
        ImageFormat::WebP => ImageFormat::WebP,
        _ => ImageFormat::Png,
    };

    let mut buffer = Vec::new();
    let ImageMetadata { icc_profile, exif } = metadata;

    match target_format {
        ImageFormat::Png => {
            let rgba = image.to_rgba8();
            let mut encoder = PngEncoder::new(&mut buffer);
            apply_image_metadata(&mut encoder, icc_profile, exif, target_format)?;
            encoder
                .write_image(
                    rgba.as_raw(),
                    image.width(),
                    image.height(),
                    ColorType::Rgba8.into(),
                )
                .map_err(|source| ImageProcessingError::Encode {
                    format: target_format,
                    source,
                })?;
        }
        ImageFormat::Jpeg => {
            let mut encoder = JpegEncoder::new_with_quality(&mut buffer, 85);
            apply_image_metadata(&mut encoder, icc_profile, exif, target_format)?;
            encoder
                .encode_image(image)
                .map_err(|source| ImageProcessingError::Encode {
                    format: target_format,
                    source,
                })?;
        }
        ImageFormat::WebP => {
            let rgba = image.to_rgba8();
            let mut encoder = WebPEncoder::new_lossless(&mut buffer);
            apply_image_metadata(&mut encoder, icc_profile, exif, target_format)?;
            encoder
                .write_image(
                    rgba.as_raw(),
                    image.width(),
                    image.height(),
                    ColorType::Rgba8.into(),
                )
                .map_err(|source| ImageProcessingError::Encode {
                    format: target_format,
                    source,
                })?;
        }
        _ => unreachable!("unsupported target_format should have been handled earlier"),
    }

    Ok((buffer, target_format))
}

fn apply_image_metadata(
    encoder: &mut impl ImageEncoder,
    icc_profile: Option<Vec<u8>>,
    exif: Option<Vec<u8>>,
    format: ImageFormat,
) -> Result<(), ImageProcessingError> {
    if let Some(icc_profile) = icc_profile {
        encoder
            .set_icc_profile(icc_profile)
            .map_err(|source| ImageProcessingError::Encode {
                format,
                source: image::ImageError::Unsupported(source),
            })?;
    }
    if let Some(exif) = exif {
        encoder
            .set_exif_metadata(exif)
            .map_err(|source| ImageProcessingError::Encode {
                format,
                source: image::ImageError::Unsupported(source),
            })?;
    }
    Ok(())
}

fn format_to_mime(format: ImageFormat) -> String {
    match format {
        ImageFormat::Jpeg => "image/jpeg".to_string(),
        ImageFormat::Gif => "image/gif".to_string(),
        ImageFormat::WebP => "image/webp".to_string(),
        _ => "image/png".to_string(),
    }
}

#[cfg(test)]
#[path = "image_tests.rs"]
mod tests;
