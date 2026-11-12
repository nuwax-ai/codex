//! Fork (nuwax-codex): request-level inline-image budget.
//!
//! The per-output image limits bound one tool result; nothing bounded the whole
//! request. A session that captured a few full-page screenshots re-sends every
//! one of them on every turn, so the first ceiling that noticed was the
//! gateway's body limit — reported as a 413 and answered with a blind
//! shrink-and-retry that could not see which images were responsible.
//!
//! This pass runs on the prepared request items: while the request's inline
//! image payload is over budget, the largest image is re-encoded smaller. The
//! model keeps the evidence (a downscaled screenshot is still readable); only
//! pixels it never needed are dropped, and the gateway limit stops deciding.
//!
//! It works on the request's copy of the history, so the fitted bytes are not
//! written back: a session that stays over budget re-encodes its largest image
//! once per request. The per-image budget is what keeps that from being the
//! normal case — with it, eight screenshots sit well under this ceiling.

use codex_history::ResponseItemEnvelope;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::ImageReference;
use codex_protocol::models::ResponseItem;

/// Total inline image payload one request may carry.
///
/// A 32 MiB gateway body limit is common (the NuwaX proxy is one), and the
/// request also carries the system prompt, tool schemas, and conversation text.
/// 20 MiB leaves room for those without guessing per provider.
pub(crate) const DEFAULT_REQUEST_IMAGE_BYTES: usize = 20 * 1024 * 1024;

/// Operator override, in bytes. Zero or an unparsable value falls back to the
/// default; the variable is read per request because it is process-wide config.
pub(crate) const REQUEST_IMAGE_BYTES_ENV: &str = "NUWAX_REQUEST_IMAGE_MAX_BYTES";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RequestImageBudgetOutcome {
    /// Inline image bytes still in the request after fitting.
    pub(crate) total_bytes: usize,
    /// Images that were re-encoded at least once.
    pub(crate) rewritten: usize,
    /// Budget the pass was given.
    pub(crate) budget: usize,
}

impl RequestImageBudgetOutcome {
    pub(crate) fn is_empty(&self) -> bool {
        self.rewritten == 0
    }
}

/// The budget for one request, from the environment or the default.
pub(crate) fn request_image_budget() -> usize {
    std::env::var(REQUEST_IMAGE_BYTES_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|bytes| *bytes > 0)
        .unwrap_or(DEFAULT_REQUEST_IMAGE_BYTES)
}

/// Re-encodes the largest inline images until the request fits `budget`.
///
/// Selection of *which* image gives up bytes lives in
/// `codex_utils_image::fit_data_urls_to_total_budget`; this module only knows
/// where inline images sit inside conversation items.
pub(crate) fn fit_request_images_to_budget(
    items: &mut [ResponseItem],
    budget: usize,
) -> RequestImageBudgetOutcome {
    let mut refs: Vec<&mut ResponseItem> = items.iter_mut().collect();
    fit_to_budget(&mut refs, budget)
}

/// Same pass over the annotated history envelopes a request is built from.
pub(crate) fn fit_request_envelopes_to_budget(
    envelopes: &mut [ResponseItemEnvelope],
    budget: usize,
) -> RequestImageBudgetOutcome {
    let mut refs: Vec<&mut ResponseItem> = envelopes
        .iter_mut()
        .map(|envelope| &mut envelope.item)
        .collect();
    fit_to_budget(&mut refs, budget)
}

fn fit_to_budget(items: &mut [&mut ResponseItem], budget: usize) -> RequestImageBudgetOutcome {
    // Locate every inline image first: the borrow checker cannot hand out
    // several `&mut` into the same slice at once, and the fit needs to choose
    // between all of them.
    let mut slots: Vec<(usize, usize)> = Vec::new();
    let mut total = 0usize;
    for (item_index, item) in items.iter().enumerate() {
        let mut slot = 0;
        while let Some(url) = inline_image_ref(item, slot) {
            total += url.len();
            slots.push((item_index, slot));
            slot += 1;
        }
    }
    if total <= budget {
        return RequestImageBudgetOutcome {
            total_bytes: total,
            rewritten: 0,
            budget,
        };
    }

    // Same traversal as above, so `urls[i]` belongs to `slots[i]`; building
    // both from one scan is what keeps a write-back from landing on the wrong
    // image.
    let mut urls: Vec<String> = Vec::with_capacity(slots.len());
    for (item_index, slot) in &slots {
        match inline_image_ref(items[*item_index], *slot) {
            Some(url) => urls.push(url.to_owned()),
            None => {
                return RequestImageBudgetOutcome {
                    total_bytes: total,
                    rewritten: 0,
                    budget,
                };
            }
        }
    }
    let fitted = codex_utils_image::fit_data_urls_to_total_budget(&mut urls, budget);

    let mut rewritten = 0;
    for (index, (item_index, slot)) in slots.iter().enumerate() {
        let Some(url) = urls.get(index) else {
            break;
        };
        let Some(entry) = items.get_mut(*item_index) else {
            continue;
        };
        if let Some(current) = inline_image_mut(&mut **entry, *slot)
            && current != url
        {
            *current = url.clone();
            rewritten += 1;
        }
    }
    RequestImageBudgetOutcome {
        total_bytes: fitted.total_bytes,
        rewritten,
        budget,
    }
}

/// The `slot`-th inline image in `item`, counted the same way as the pass above.
fn inline_image_ref(item: &ResponseItem, slot: usize) -> Option<&str> {
    let mut seen = 0;
    match item {
        ResponseItem::Message { content, .. } => {
            for entry in content {
                if let ContentItem::InputImage { image, .. } = entry
                    && let ImageReference::Inline { image_url } = image
                {
                    if seen == slot {
                        return Some(image_url);
                    }
                    seen += 1;
                }
            }
            None
        }
        ResponseItem::FunctionCallOutput { output, .. }
        | ResponseItem::CustomToolCallOutput { output, .. } => {
            for entry in output.content_items()? {
                if let FunctionCallOutputContentItem::InputImage { image, .. } = entry
                    && let ImageReference::Inline { image_url } = image
                {
                    if seen == slot {
                        return Some(image_url);
                    }
                    seen += 1;
                }
            }
            None
        }
        _ => None,
    }
}

fn inline_image_mut(item: &mut ResponseItem, slot: usize) -> Option<&mut String> {
    let mut seen = 0;
    match item {
        ResponseItem::Message { content, .. } => {
            for entry in content {
                if let ContentItem::InputImage { image, .. } = entry
                    && let ImageReference::Inline { image_url } = image
                {
                    if seen == slot {
                        return Some(image_url);
                    }
                    seen += 1;
                }
            }
            None
        }
        ResponseItem::FunctionCallOutput { output, .. }
        | ResponseItem::CustomToolCallOutput { output, .. } => {
            for entry in output.content_items_mut()? {
                if let FunctionCallOutputContentItem::InputImage { image, .. } = entry
                    && let ImageReference::Inline { image_url } = image
                {
                    if seen == slot {
                        return Some(image_url);
                    }
                    seen += 1;
                }
            }
            None
        }
        _ => None,
    }
}

#[cfg(test)]
#[path = "image_request_budget_tests.rs"]
mod tests;
