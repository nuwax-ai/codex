use crate::TruncationPolicy;
use crate::approx_bytes_for_tokens;
use codex_protocol::models::FunctionCallOutputContentItem;
use std::sync::OnceLock;

/// Hard cap for model-visible text and charged media in one image-bearing tool result.
pub const MAX_TOOL_OUTPUT_VISIBLE_TOKENS: usize = 10_000;
/// Bound reference identifiers and data-URL headers separately from image payloads.
pub const MAX_IMAGE_REFERENCE_BYTES: usize = 8 * 1024;
// Covers one bounded timing/status header, three consolidated omission markers,
// and the existing inline truncation notice. It is inside the hard cap.
const CONTROL_HEADROOM_TOKENS: usize = 256;
const MAX_TOOL_OUTPUT_HEADER_BYTES: usize = 256;

fn bounded_image_output_policy(policy: TruncationPolicy) -> TruncationPolicy {
    let content_tokens = MAX_TOOL_OUTPUT_VISIBLE_TOKENS - CONTROL_HEADROOM_TOKENS;
    match policy {
        TruncationPolicy::Tokens(tokens) => TruncationPolicy::Tokens(tokens.min(content_tokens)),
        TruncationPolicy::Bytes(bytes) => {
            TruncationPolicy::Bytes(bytes.min(approx_bytes_for_tokens(content_tokens)))
        }
    }
}

pub(crate) fn tool_output_items_policy(
    items: &[FunctionCallOutputContentItem],
    policy: TruncationPolicy,
) -> TruncationPolicy {
    // The screenshot backstop must not override explicit exec/MCP text limits.
    // Keep the image cap on later history passes even if all images were omitted.
    if items.iter().any(|item| match item {
        FunctionCallOutputContentItem::InputImage { .. } => true,
        FunctionCallOutputContentItem::InputText { text } => {
            matches!(omission_marker(text), Some((OmissionKind::Image, _)))
        }
        FunctionCallOutputContentItem::InputAudio { .. }
        | FunctionCallOutputContentItem::EncryptedContent { .. } => false,
    }) {
        bounded_image_output_policy(policy)
    } else {
        policy
    }
}

pub(crate) fn is_tool_output_header(text: &str) -> bool {
    if text.len() > MAX_TOOL_OUTPUT_HEADER_BYTES {
        return false;
    }
    if let Some(seconds) = text
        .strip_prefix("Wall time: ")
        .and_then(|text| text.strip_suffix(" seconds\nOutput:"))
    {
        return seconds.parse::<f64>().is_ok();
    }
    let Some((status, timing)) = text.split_once('\n') else {
        return false;
    };
    let status_valid = matches!(
        status,
        "Script completed" | "Script failed" | "Script terminated"
    ) || status
        .strip_prefix("Script running with cell ID ")
        .is_some_and(|id| {
            !id.is_empty()
                && id.len() <= 64
                && id
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        });
    status_valid
        && timing.starts_with("Wall time ")
        && (timing.ends_with(" seconds\nOutput:\n")
            || timing.contains(" seconds (code-mode ") && timing.ends_with(" seconds)\nOutput:\n"))
}

pub(crate) enum OmissionKind {
    Image,
    Text,
    Audio,
}

pub(crate) fn omission_marker(text: &str) -> Option<(OmissionKind, usize)> {
    if text.len() > MAX_TOOL_OUTPUT_HEADER_BYTES {
        return None;
    }
    let (count, suffix) = text.strip_prefix("[omitted ")?.split_once(' ')?;
    let count = count.parse::<usize>().ok()?;
    match suffix {
        "text items ...]" => Some((OmissionKind::Text, count)),
        "audio items ...]" => Some((OmissionKind::Audio, count)),
        _ if omitted_images_marker(count)
            == (FunctionCallOutputContentItem::InputText {
                text: text.to_owned(),
            }) =>
        {
            Some((OmissionKind::Image, count))
        }
        _ => None,
    }
}

/// Bounded omission text shared by the producer and the history-preparation backstop.
pub fn omitted_images_marker(dropped: usize) -> FunctionCallOutputContentItem {
    omitted_images_marker_with(dropped, current())
}

fn omitted_images_marker_with(
    dropped: usize,
    limits: ImageLimits,
) -> FunctionCallOutputContentItem {
    FunctionCallOutputContentItem::InputText {
        text: format!(
            "[omitted {dropped} image(s): over the per-output image limits (max {} images, {} MiB each, {} MiB total, {} KiB reference/header, {} visual tokens; unknown original costs {} tokens plus control headroom) ...]",
            limits.max_images,
            limits.max_image_bytes / (1024 * 1024),
            limits.max_total_bytes / (1024 * 1024),
            MAX_IMAGE_REFERENCE_BYTES / 1024,
            MAX_TOOL_OUTPUT_VISIBLE_TOKENS,
            TOOL_OUTPUT_IMAGE_ORIGINAL_VISUAL_TOKENS,
        ),
    }
}

/// Named per-tool-output media limits. These are entrance bounds for images
/// inside ONE tool result (screenshots, MCP `image` blocks, Code Mode
/// `image()`): they complement — never replace — the wire byte limit, the
/// output reserve, and the decode-safety pixel cap in `codex-utils-image`.
///
/// The values deliberately bound the model-visible cost of a single tool
/// result without silently discarding the newest evidence: when limits bite,
/// the newest images are retained and the dropped ones are reported through
/// one bounded text marker.
pub const MAX_TOOL_OUTPUT_IMAGES: usize = 8;
pub const MAX_TOOL_OUTPUT_IMAGE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_TOOL_OUTPUT_IMAGE_TOTAL_BYTES: usize = 24 * 1024 * 1024;
/// Visual token cost charged for one standard (high/auto) prepared image.
/// Mirrors the fork's history estimator and the bridge's wire-image budget
/// (~7,373-byte prepared PNG ≈ 1,844 tokens); keep the three in sync.
pub const TOOL_OUTPUT_IMAGE_VISUAL_TOKENS: usize = 1_844;
/// Visual token cost for a `detail: original` image: the conservative patch
/// ceiling used when pixel dimensions are unknown at this layer.
pub const TOOL_OUTPUT_IMAGE_ORIGINAL_VISUAL_TOKENS: usize = 10_000;

/// Fork (nuwax-codex, 2026-11-20): the entrance bounds above are defaults;
/// deployments may tune them with `NUWAX_IMAGE_*` environment variables when a
/// model or workflow legitimately needs larger screenshots. Parsed once, and
/// validated at process startup so a bad value fails fast instead of silently
/// using the default deep in the truncation path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageLimits {
    /// Images kept per tool result (newest first).
    pub max_images: usize,
    /// Data-URL length bound for one image before anything decodes it.
    pub max_image_bytes: usize,
    /// Data-URL length bound for all images of one tool result.
    pub max_total_bytes: usize,
}

impl Default for ImageLimits {
    fn default() -> Self {
        Self {
            max_images: MAX_TOOL_OUTPUT_IMAGES,
            max_image_bytes: MAX_TOOL_OUTPUT_IMAGE_BYTES,
            max_total_bytes: MAX_TOOL_OUTPUT_IMAGE_TOTAL_BYTES,
        }
    }
}

/// `NUWAX_IMAGE_MAX_COUNT` / `CODEX_IMAGE_MAX_COUNT`.
const ENV_MAX_COUNT: (&str, &str) = ("NUWAX_IMAGE_MAX_COUNT", "CODEX_IMAGE_MAX_COUNT");
/// `NUWAX_IMAGE_ENTRY_MAX_BYTES` / `CODEX_IMAGE_ENTRY_MAX_BYTES`.
const ENV_ENTRY_MAX_BYTES: (&str, &str) =
    ("NUWAX_IMAGE_ENTRY_MAX_BYTES", "CODEX_IMAGE_ENTRY_MAX_BYTES");
/// `NUWAX_IMAGE_ENTRY_MAX_TOTAL_BYTES` / `CODEX_IMAGE_ENTRY_MAX_TOTAL_BYTES`.
const ENV_ENTRY_MAX_TOTAL_BYTES: (&str, &str) = (
    "NUWAX_IMAGE_ENTRY_MAX_TOTAL_BYTES",
    "CODEX_IMAGE_ENTRY_MAX_TOTAL_BYTES",
);

const MIN_IMAGE_COUNT: usize = 1;
const MAX_IMAGE_COUNT: usize = 16;
const MIN_IMAGE_BYTES: usize = 1;
const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
const MIN_TOTAL_BYTES: usize = 8 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 128 * 1024 * 1024;

static IMAGE_LIMITS: OnceLock<Result<ImageLimits, String>> = OnceLock::new();

/// Limits for this process, initialised from the environment on first use.
///
/// An invalid environment value keeps the built-in defaults here; call
/// [`env_error`] at startup to turn it into a hard failure naming the variable.
pub fn current() -> ImageLimits {
    match IMAGE_LIMITS.get_or_init(|| parse_limits(&|name| std::env::var(name).ok())) {
        Ok(limits) => *limits,
        Err(_) => ImageLimits::default(),
    }
}

/// The environment-parsing error, if any. Startup paths call this to fail fast.
pub fn env_error() -> Option<&'static str> {
    match IMAGE_LIMITS.get_or_init(|| parse_limits(&|name| std::env::var(name).ok())) {
        Ok(_) => None,
        Err(error) => Some(error.as_str()),
    }
}

fn parse_limits(get: &dyn Fn(&str) -> Option<String>) -> Result<ImageLimits, String> {
    let mut limits = ImageLimits::default();
    if let Some((raw, name)) = read_setting(get, ENV_MAX_COUNT)? {
        limits.max_images = parse_bounded(&raw, name, MIN_IMAGE_COUNT, MAX_IMAGE_COUNT)?;
    }
    if let Some((raw, name)) = read_setting(get, ENV_ENTRY_MAX_BYTES)? {
        limits.max_image_bytes = parse_bounded(&raw, name, MIN_IMAGE_BYTES, MAX_IMAGE_BYTES)?;
    }
    if let Some((raw, name)) = read_setting(get, ENV_ENTRY_MAX_TOTAL_BYTES)? {
        limits.max_total_bytes = parse_bounded(&raw, name, MIN_TOTAL_BYTES, MAX_TOTAL_BYTES)?;
    }
    if limits.max_total_bytes < limits.max_image_bytes {
        return Err(format!(
            "{} ({}) must be at least {} ({})",
            ENV_ENTRY_MAX_TOTAL_BYTES.0,
            limits.max_total_bytes,
            ENV_ENTRY_MAX_BYTES.0,
            limits.max_image_bytes,
        ));
    }
    Ok(limits)
}

fn read_setting(
    get: &dyn Fn(&str) -> Option<String>,
    names: (&'static str, &'static str),
) -> Result<Option<(String, &'static str)>, String> {
    let (nuwax_name, codex_name) = names;
    let nuwax = get(nuwax_name).filter(|value| !value.trim().is_empty());
    let codex = get(codex_name).filter(|value| !value.trim().is_empty());
    match (nuwax, codex) {
        (Some(nuwax_value), Some(codex_value)) if nuwax_value.trim() != codex_value.trim() => Err(
            format!("{nuwax_name} ({nuwax_value:?}) and {codex_name} ({codex_value:?}) disagree"),
        ),
        (Some(value), _) => Ok(Some((value, nuwax_name))),
        (None, Some(value)) => Ok(Some((value, codex_name))),
        (None, None) => Ok(None),
    }
}

fn parse_bounded(raw: &str, name: &str, min: usize, max: usize) -> Result<usize, String> {
    let parsed: i64 = raw.trim().parse().map_err(|_| {
        format!("Invalid {name} value {raw:?}; expected an integer between {min} and {max}")
    })?;
    let value = usize::try_from(parsed).map_err(|_| {
        format!("Invalid {name} value {raw:?}; expected an integer between {min} and {max}")
    })?;
    if !(min..=max).contains(&value) {
        return Err(format!(
            "Invalid {name} value {raw:?}; expected an integer between {min} and {max}"
        ));
    }
    Ok(value)
}

/// Conservative visual cost, without decoding untrusted image bytes.
fn charged_visual_tokens(
    image: &codex_protocol::models::ImageReference,
    detail: Option<codex_protocol::models::ImageDetail>,
) -> usize {
    match detail {
        Some(codex_protocol::models::ImageDetail::Original) => match image {
            codex_protocol::models::ImageReference::Inline { image_url } => {
                codex_utils_image::estimate_original_image_tokens(image_url)
                    .unwrap_or(TOOL_OUTPUT_IMAGE_ORIGINAL_VISUAL_TOKENS)
            }
            codex_protocol::models::ImageReference::File { .. } => {
                TOOL_OUTPUT_IMAGE_ORIGINAL_VISUAL_TOKENS
            }
        },
        _ => TOOL_OUTPUT_IMAGE_VISUAL_TOKENS,
    }
}

pub(crate) fn image_budget_charge(visual_tokens: usize, policy: TruncationPolicy) -> usize {
    match policy {
        TruncationPolicy::Bytes(_) => approx_bytes_for_tokens(visual_tokens),
        TruncationPolicy::Tokens(_) => visual_tokens,
    }
}

/// Which images of one tool result survive the entrance limits.
pub struct ImageAdmission {
    /// Per input index: whether the InputImage item is kept.
    pub keep: Vec<bool>,
    /// Images replaced by the single consolidated omission marker.
    pub dropped: usize,
    /// Total visual tokens of the kept images.
    pub visual_tokens: usize,
}

pub fn admit_tool_output_images(
    items: &[FunctionCallOutputContentItem],
    policy: TruncationPolicy,
) -> ImageAdmission {
    admit_tool_output_images_with(items, policy, current())
}

/// Same as [`admit_tool_output_images`] with explicit limits (used by tests and
/// by callers that already hold a resolved configuration).
pub fn admit_tool_output_images_with(
    items: &[FunctionCallOutputContentItem],
    policy: TruncationPolicy,
    limits: ImageLimits,
) -> ImageAdmission {
    let image_indices: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| matches!(item, FunctionCallOutputContentItem::InputImage { .. }))
        .map(|(index, _)| index)
        .collect();
    let mut keep = vec![false; items.len()];
    let mut dropped = 0usize;

    // Entrance bound 1: one image may not exceed the per-image encoded size
    // before anything decodes it.
    let mut candidates: Vec<usize> = image_indices
        .iter()
        .copied()
        .filter(|&index| {
            let FunctionCallOutputContentItem::InputImage { image, .. } = &items[index] else {
                unreachable!("filtered to images");
            };
            match image {
                codex_protocol::models::ImageReference::Inline { image_url } => {
                    let header_fits = !image_url
                        .get(..5)
                        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
                        || image_url
                            .split_once(',')
                            .map_or(image_url.len(), |(header, _)| header.len())
                            <= MAX_IMAGE_REFERENCE_BYTES;
                    image_url.len() <= limits.max_image_bytes && header_fits
                }
                // File references carry no payload here; the wire layer
                // already charges them full visual cost.
                codex_protocol::models::ImageReference::File { file_id } => {
                    file_id.len() <= MAX_IMAGE_REFERENCE_BYTES
                }
            }
        })
        .collect();
    dropped += image_indices.len().saturating_sub(candidates.len());

    // Entrance bound 2: at most `limits.max_images`, keeping the NEWEST
    // (the latest screenshot is the evidence the turn still needs).
    if candidates.len() > limits.max_images {
        let excess = candidates.len() - limits.max_images;
        dropped += excess;
        candidates.drain(..excess);
    }

    // Entrance bound 3: aggregate encoded bytes, again dropping oldest first.
    let encoded_len = |index: usize| {
        let FunctionCallOutputContentItem::InputImage { image, .. } = &items[index] else {
            unreachable!("filtered to images");
        };
        match image {
            codex_protocol::models::ImageReference::Inline { image_url } => image_url.len(),
            codex_protocol::models::ImageReference::File { .. } => 0,
        }
    };
    let mut total: usize = candidates.iter().copied().map(encoded_len).sum();
    while total > limits.max_total_bytes && candidates.len() > 1 {
        let removed = candidates.remove(0);
        total -= encoded_len(removed);
        dropped += 1;
    }

    // Entrance bound 4: text and standard-detail images share the
    // per-output budget. Keep images newest-first while their cumulative
    // visual cost fits. Original detail uses bounded header dimensions where
    // available and otherwise costs the conservative patch ceiling.
    let budget_bytes = bounded_image_output_policy(policy).byte_budget();
    let mut visual_total = 0usize;
    let mut visual_kept: Vec<usize> = Vec::new();
    for &index in candidates.iter().rev() {
        let FunctionCallOutputContentItem::InputImage { image, detail } = &items[index] else {
            unreachable!("filtered to images");
        };
        let cost = charged_visual_tokens(image, *detail);
        if approx_bytes_for_tokens(visual_total + cost) <= budget_bytes {
            visual_total += cost;
            visual_kept.push(index);
        } else {
            dropped += 1;
        }
    }
    for &index in &visual_kept {
        keep[index] = true;
    }

    ImageAdmission {
        keep,
        dropped,
        visual_tokens: visual_total,
    }
}

#[cfg(test)]
mod limits_tests {
    use super::ImageLimits;
    use super::parse_limits;
    use std::collections::HashMap;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn defaults_without_environment() {
        let limits = parse_limits(&lookup(&[])).expect("defaults");
        assert_eq!(limits, ImageLimits::default());
    }

    #[test]
    fn nuwax_values_override_defaults() {
        let limits = parse_limits(&lookup(&[
            ("NUWAX_IMAGE_MAX_COUNT", "12"),
            ("NUWAX_IMAGE_ENTRY_MAX_BYTES", "33554432"),
            ("NUWAX_IMAGE_ENTRY_MAX_TOTAL_BYTES", "50331648"),
        ]))
        .expect("valid limits");
        assert_eq!(limits.max_images, 12);
        assert_eq!(limits.max_image_bytes, 33_554_432);
        assert_eq!(limits.max_total_bytes, 50_331_648);
    }

    #[test]
    fn codex_alias_is_accepted() {
        let limits = parse_limits(&lookup(&[("CODEX_IMAGE_MAX_COUNT", "4")])).expect("alias");
        assert_eq!(limits.max_images, 4);
    }

    #[test]
    fn non_integer_and_out_of_range_fail_with_the_variable_name() {
        let error = parse_limits(&lookup(&[("NUWAX_IMAGE_MAX_COUNT", "many")]))
            .expect_err("non-integer must fail");
        assert!(error.contains("NUWAX_IMAGE_MAX_COUNT"), "{error}");

        let error = parse_limits(&lookup(&[("NUWAX_IMAGE_ENTRY_MAX_TOTAL_BYTES", "1024")]))
            .expect_err("below the floor must fail");
        assert!(
            error.contains("NUWAX_IMAGE_ENTRY_MAX_TOTAL_BYTES"),
            "{error}"
        );
    }

    #[test]
    fn total_below_entry_limit_fails() {
        let error = parse_limits(&lookup(&[
            ("NUWAX_IMAGE_ENTRY_MAX_BYTES", "33554432"),
            ("NUWAX_IMAGE_ENTRY_MAX_TOTAL_BYTES", "16777216"),
        ]))
        .expect_err("total < entry must fail");
        assert!(error.contains("at least"), "{error}");
    }

    #[test]
    fn custom_total_keeps_images_the_default_would_drop() {
        use crate::TruncationPolicy;
        use codex_protocol::models::FunctionCallOutputContentItem;
        use codex_protocol::models::ImageReference;

        let image = |len: usize| FunctionCallOutputContentItem::InputImage {
            image: ImageReference::Inline {
                image_url: format!("data:image/png;base64,{}", "A".repeat(len)),
            },
            detail: None,
        };
        let ten_mib = 10 * 1024 * 1024;
        let items = vec![image(ten_mib), image(ten_mib), image(ten_mib)];
        let policy = TruncationPolicy::Tokens(10_000);

        let default = super::admit_tool_output_images_with(
            &items,
            policy,
            super::ImageLimits::default(),
        );
        assert!(
            default.dropped >= 1,
            "30 MiB of images must exceed the 24 MiB default total"
        );

        let raised = super::admit_tool_output_images_with(
            &items,
            policy,
            super::ImageLimits {
                max_total_bytes: 48 * 1024 * 1024,
                ..super::ImageLimits::default()
            },
        );
        assert_eq!(
            raised.dropped, 0,
            "raised total must keep images the default drops"
        );
    }

    #[test]
    fn conflicting_aliases_fail_naming_both() {
        let error = parse_limits(&lookup(&[
            ("NUWAX_IMAGE_MAX_COUNT", "8"),
            ("CODEX_IMAGE_MAX_COUNT", "4"),
        ]))
        .expect_err("conflict must fail");
        assert!(error.contains("NUWAX_IMAGE_MAX_COUNT"), "{error}");
        assert!(error.contains("CODEX_IMAGE_MAX_COUNT"), "{error}");
    }
}
