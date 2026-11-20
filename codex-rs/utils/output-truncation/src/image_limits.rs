use crate::TruncationPolicy;
use crate::approx_bytes_for_tokens;
use codex_protocol::models::FunctionCallOutputContentItem;
use std::ffi::OsString;
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

/// Visual tokens actually available to image items in one tool result: the
/// per-output cap minus control headroom (9,744 of 10,000).
///
/// Shrink decisions must compare against this number, not the raw cap: the
/// original-image estimator caps at `ORIGINAL_PATCH_CEILING` (10,000), so a
/// `> 10_000` test never fires and a large-but-legal original image would skip
/// shrinking and then be dropped by the admission's 9,744-token budget.
pub(crate) fn effective_visual_budget_tokens() -> usize {
    bounded_image_output_policy(TruncationPolicy::Tokens(usize::MAX)).token_budget()
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
        // Match the marker by its stable shape, not by regenerating it with the
        // current limits: history written under one configuration must stay
        // recognizable after the operator changes the environment (the numbers
        // in the marker describe the limits that produced it).
        _ if is_image_omission_marker(suffix) => Some((OmissionKind::Image, count)),
        _ => None,
    }
}

/// Stable head of the image omission marker (after `[omitted N `).
const IMAGE_OMISSION_MARKER_PREFIX: &str = "image(s): over the per-output image limits (";
/// Stable tail of the image omission marker.
const IMAGE_OMISSION_MARKER_SUFFIX: &str = ") ...]";

/// Whether `suffix` is the body of an engine-generated image omission marker.
///
/// The numbers describe the limits that produced the marker, so they are not
/// compared against the current configuration; the field *shape* is validated
/// instead, so ordinary tool text that merely starts and ends like the marker
/// is not mistaken for one.
fn is_image_omission_marker(suffix: &str) -> bool {
    let Some(body) = suffix.strip_prefix(IMAGE_OMISSION_MARKER_PREFIX) else {
        return false;
    };
    let Some(body) = body.strip_suffix(IMAGE_OMISSION_MARKER_SUFFIX) else {
        return false;
    };
    let Some((limits, tail)) = body.split_once("; ") else {
        return false;
    };
    let parts: Vec<&str> = limits.split(", ").collect();
    let [count, per_image, total, reference, visual] = parts.as_slice() else {
        return false;
    };
    let Some(count) = count.strip_prefix("max ") else {
        return false;
    };
    number_segment(count, " images")
        && number_segment(per_image, " MiB each")
        && number_segment(total, " MiB total")
        && number_segment(reference, " KiB reference/header")
        && number_segment(visual, " visual tokens")
        && tail
            .strip_prefix("unknown original costs ")
            .and_then(|rest| rest.strip_suffix(" tokens plus control headroom"))
            .is_some_and(|value| value.parse::<u64>().is_ok())
}

fn number_segment(segment: &str, suffix: &str) -> bool {
    segment
        .strip_suffix(suffix)
        .is_some_and(|value| !value.is_empty() && value.parse::<u64>().is_ok())
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

/// Default entrance bounds for images inside ONE tool result (screenshots, MCP
/// `image` blocks, Code Mode `image()`): they complement — never replace — the
/// wire byte limit, the output reserve, and the decode-safety pixel cap in
/// `codex-utils-image`.
///
/// These constants are the built-in defaults; deployments tune them through
/// [`current`] (the `NUWAX_IMAGE_*` environment variables, see [`ImageLimits`]).
/// The defaults deliberately bound the model-visible cost of a single tool
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
const MIN_ENTRY_IMAGE_BYTES: usize = 1024 * 1024;
const MAX_ENTRY_IMAGE_BYTES: usize = 64 * 1024 * 1024;
const MIN_ENTRY_TOTAL_BYTES: usize = 8 * 1024 * 1024;
const MAX_ENTRY_TOTAL_BYTES: usize = 128 * 1024 * 1024;

static IMAGE_LIMITS: OnceLock<Result<ImageLimits, String>> = OnceLock::new();

/// Limits for this process, initialised from the environment on first use.
///
/// An invalid environment value keeps the built-in defaults here; call
/// [`env_error`] at startup to turn it into a hard failure naming the variable.
pub fn current() -> ImageLimits {
    match IMAGE_LIMITS.get_or_init(|| parse_limits(&|name| std::env::var_os(name))) {
        Ok(limits) => *limits,
        Err(_) => ImageLimits::default(),
    }
}

/// The environment-parsing error, if any. Startup paths call this to fail fast.
pub fn env_error() -> Option<&'static str> {
    match IMAGE_LIMITS.get_or_init(|| parse_limits(&|name| std::env::var_os(name))) {
        Ok(_) => None,
        Err(error) => Some(error.as_str()),
    }
}

fn parse_limits(get: &dyn Fn(&str) -> Option<OsString>) -> Result<ImageLimits, String> {
    let mut limits = ImageLimits::default();
    let mut entry_name = ENV_ENTRY_MAX_BYTES.0;
    let mut total_name = ENV_ENTRY_MAX_TOTAL_BYTES.0;
    if let Some((raw, name)) = read_setting(get, ENV_MAX_COUNT)? {
        limits.max_images =
            parse_bounded(&raw, name, "image count", MIN_IMAGE_COUNT, MAX_IMAGE_COUNT)?;
    }
    if let Some((raw, name)) = read_setting(get, ENV_ENTRY_MAX_BYTES)? {
        limits.max_image_bytes = parse_bounded(
            &raw,
            name,
            "byte count",
            MIN_ENTRY_IMAGE_BYTES,
            MAX_ENTRY_IMAGE_BYTES,
        )?;
        entry_name = name;
    }
    if let Some((raw, name)) = read_setting(get, ENV_ENTRY_MAX_TOTAL_BYTES)? {
        limits.max_total_bytes = parse_bounded(
            &raw,
            name,
            "byte count",
            MIN_ENTRY_TOTAL_BYTES,
            MAX_ENTRY_TOTAL_BYTES,
        )?;
        total_name = name;
    }
    if limits.max_total_bytes < limits.max_image_bytes {
        return Err(format!(
            "{total_name} ({}) must be at least {entry_name} ({})",
            limits.max_total_bytes, limits.max_image_bytes,
        ));
    }
    Ok(limits)
}

fn read_setting(
    get: &dyn Fn(&str) -> Option<OsString>,
    names: (&'static str, &'static str),
) -> Result<Option<(String, &'static str)>, String> {
    let (nuwax_name, codex_name) = names;
    let nuwax = read_env_text(get, nuwax_name)?;
    let codex = read_env_text(get, codex_name)?;
    match (nuwax, codex) {
        (Some(nuwax_value), Some(codex_value)) if nuwax_value.trim() != codex_value.trim() => Err(
            format!("{nuwax_name} ({nuwax_value:?}) and {codex_name} ({codex_value:?}) disagree"),
        ),
        (Some(value), _) => Ok(Some((value, nuwax_name))),
        (None, Some(value)) => Ok(Some((value, codex_name))),
        (None, None) => Ok(None),
    }
}

/// Reads one variable as UTF-8 text, treating only blank values as unset.
///
/// `std::env::var` maps a non-Unicode value to the same `None` as an unset
/// variable, which silently skips fail-fast validation; read the raw `OsString`
/// and reject it with the variable named instead.
fn read_env_text(
    get: &dyn Fn(&str) -> Option<OsString>,
    name: &str,
) -> Result<Option<String>, String> {
    let Some(raw) = get(name) else {
        return Ok(None);
    };
    let text = raw
        .into_string()
        .map_err(|raw| format!("Invalid {name}: value must be UTF-8 text; got {raw:?}"))?;
    Ok((!text.trim().is_empty()).then_some(text))
}

fn parse_bounded(
    raw: &str,
    name: &str,
    unit: &str,
    min: usize,
    max: usize,
) -> Result<usize, String> {
    let invalid = || {
        format!("Invalid {name} value {raw:?}; expected an integer {unit} between {min} and {max}")
    };
    let parsed: i64 = raw.trim().parse().map_err(|_| invalid())?;
    let value = usize::try_from(parsed).map_err(|_| invalid())?;
    if !(min..=max).contains(&value) {
        return Err(invalid());
    }
    Ok(value)
}

/// Conservative visual cost, without decoding untrusted image bytes.
fn charged_visual_tokens(
    image: &codex_protocol::models::ImageReference,
    detail: Option<codex_protocol::models::ImageDetail>,
    limits: ImageLimits,
) -> usize {
    match detail {
        Some(codex_protocol::models::ImageDetail::Original) => match image {
            codex_protocol::models::ImageReference::Inline { image_url } => {
                // Use the configured entry bound so a legal image above the
                // built-in 16 MiB estimate bound still gets a real patch count
                // instead of the conservative 10,000-token ceiling.
                codex_utils_image::estimate_original_image_tokens_with_limit(
                    image_url,
                    limits.max_image_bytes,
                )
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
        let cost = charged_visual_tokens(image, *detail, limits);
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
    use super::omitted_images_marker_with;
    use super::parse_limits;
    use codex_protocol::models::FunctionCallOutputContentItem;
    use std::collections::HashMap;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<std::ffi::OsString> {
        let map: HashMap<String, std::ffi::OsString> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), std::ffi::OsString::from(*value)))
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

        let default =
            super::admit_tool_output_images_with(&items, policy, super::ImageLimits::default());
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
    fn relationship_error_names_the_spelling_that_was_set() {
        let error = parse_limits(&lookup(&[
            ("CODEX_IMAGE_ENTRY_MAX_BYTES", "33554432"),
            ("CODEX_IMAGE_ENTRY_MAX_TOTAL_BYTES", "16777216"),
        ]))
        .expect_err("total < entry must fail");
        assert!(
            error.contains("CODEX_IMAGE_ENTRY_MAX_TOTAL_BYTES"),
            "{error}"
        );
        assert!(error.contains("CODEX_IMAGE_ENTRY_MAX_BYTES"), "{error}");
        assert!(
            !error.contains("NUWAX_IMAGE_ENTRY_MAX_TOTAL_BYTES"),
            "{error}"
        );
    }

    #[test]
    fn omission_marker_is_recognized_after_limits_change() {
        // Written under one configuration, parsed under another: the numbers in
        // the marker describe the old limits, but the shape must stay stable.
        let written = omitted_images_marker_with(3, ImageLimits::default());
        let FunctionCallOutputContentItem::InputText { text } = written else {
            panic!("marker must be text");
        };
        assert!(matches!(
            super::omission_marker(&text),
            Some((super::OmissionKind::Image, 3))
        ));

        let other = ImageLimits {
            max_images: 16,
            max_image_bytes: 48 * 1024 * 1024,
            max_total_bytes: 96 * 1024 * 1024,
        };
        let rewritten = omitted_images_marker_with(7, other);
        let FunctionCallOutputContentItem::InputText { text } = rewritten else {
            panic!("marker must be text");
        };
        assert!(matches!(
            super::omission_marker(&text),
            Some((super::OmissionKind::Image, 7))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn effective_visual_budget_excludes_control_headroom() {
        // Admission charges images against 10,000 minus 256 control tokens;
        // shrink must use the same number or the comparison can never fire.
        assert_eq!(
            super::effective_visual_budget_tokens(),
            super::MAX_TOOL_OUTPUT_VISIBLE_TOKENS - 256
        );
        assert_eq!(super::effective_visual_budget_tokens(), 9_744);
    }

    #[test]
    fn marker_shape_rejects_lookalike_tool_text() {
        // Same prefix and suffix, unrelated payload: must stay ordinary text.
        let lookalike =
            "[omitted 1 image(s): over the per-output image limits (report: disk is full) ...]";
        assert!(super::omission_marker(lookalike).is_none());

        // A real marker still parses after the strict body check.
        let FunctionCallOutputContentItem::InputText { text } =
            omitted_images_marker_with(2, ImageLimits::default())
        else {
            panic!("marker must be text");
        };
        assert!(matches!(
            super::omission_marker(&text),
            Some((super::OmissionKind::Image, 2))
        ));
    }

    #[test]
    fn non_utf8_environment_value_fails_instead_of_being_ignored() {
        use std::os::unix::ffi::OsStringExt;

        let get = |name: &str| {
            (name == "NUWAX_IMAGE_MAX_COUNT").then(|| std::ffi::OsString::from_vec(vec![0xff]))
        };
        let error = parse_limits(&get).expect_err("non-UTF-8 must fail");
        assert!(error.contains("NUWAX_IMAGE_MAX_COUNT"), "{error}");
        assert!(error.contains("UTF-8"), "{error}");
    }

    /// A legal PNG larger than the built-in 16 MiB estimate bound: header-only
    /// inspection must still produce a real patch count once the entry bound is
    /// raised, instead of charging the 10,000-token ceiling and dropping it.
    #[test]
    fn original_image_above_16_mib_keeps_a_real_visual_estimate() {
        use crate::TruncationPolicy;
        use base64::Engine;
        use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
        use codex_protocol::models::FunctionCallOutputContentItem;
        use codex_protocol::models::ImageDetail;
        use codex_protocol::models::ImageReference;

        fn png_chunk(kind: [u8; 4], data: &[u8]) -> Vec<u8> {
            let mut out = Vec::with_capacity(data.len() + 12);
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            out.extend_from_slice(&kind);
            out.extend_from_slice(data);
            out.extend_from_slice(&crc32(&kind, data).to_be_bytes());
            out
        }

        fn crc32(kind: &[u8], data: &[u8]) -> u32 {
            let mut crc = 0xFFFF_FFFFu32;
            for byte in kind.iter().chain(data.iter()) {
                crc ^= u32::from(*byte);
                for _ in 0..8 {
                    let mask = (crc & 1).wrapping_neg();
                    crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
                }
            }
            !crc
        }

        let width = 2400u32;
        let height = 1750u32;
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);

        let mut png = Vec::new();
        png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        png.extend_from_slice(&png_chunk(*b"IHDR", &ihdr));
        // Header inspection never inflates IDAT; its size only makes the data URL
        // exceed the built-in 16 MiB estimate bound.
        png.extend_from_slice(&png_chunk(*b"IDAT", &vec![0u8; 13_000_000]));
        png.extend_from_slice(&png_chunk(*b"IEND", &[]));

        let encoded = BASE64_STANDARD.encode(&png);
        assert!(encoded.len() > 16 * 1024 * 1024);
        let image_url = format!("data:image/png;base64,{encoded}");
        assert!(image_url.len() <= 24 * 1024 * 1024);

        let items = vec![FunctionCallOutputContentItem::InputImage {
            image: ImageReference::Inline { image_url },
            detail: Some(ImageDetail::Original),
        }];
        let policy = TruncationPolicy::Tokens(10_000);

        // Default entry bound (16 MiB) refuses the payload outright.
        let default =
            super::admit_tool_output_images_with(&items, policy, super::ImageLimits::default());
        assert_eq!(default.dropped, 1);

        // Raised entry bound admits it and charges the real patch count (75×55).
        let raised = super::admit_tool_output_images_with(
            &items,
            policy,
            super::ImageLimits {
                max_image_bytes: 24 * 1024 * 1024,
                max_total_bytes: 24 * 1024 * 1024,
                ..super::ImageLimits::default()
            },
        );
        assert_eq!(raised.dropped, 0);
        assert_eq!(raised.visual_tokens, 75 * 55);
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
