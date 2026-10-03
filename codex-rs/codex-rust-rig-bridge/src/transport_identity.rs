//! Response and segment ownership for v3 hosted replay. Text equality validates
//! an already named owner; it never selects a message or crosses a segment.

use crate::hosted_replay::ReplayGroup;
use crate::hosted_replay::ResponseSite;
use crate::hosted_replay::UNKNOWN_WIRE_INDEX;
use serde_json::Value;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;

pub(crate) fn splice_identity(content: &mut Vec<Value>, group: &ReplayGroup) {
    let Some(sites) = group
        .block_sites
        .as_ref()
        .filter(|sites| sites.len() == group.blocks.len())
    else {
        tracing::warn!("misaligned hosted identity; retaining history and replaying pairs only");
        content.extend(group.blocks.iter().cloned());
        return;
    };
    let mut consumed = HashSet::new();
    let mut splices = BTreeMap::<usize, Vec<Value>>::new();
    let mut handled = HashSet::new();
    for (ordinal, response) in group.responses.iter().enumerate() {
        let mut pairs: Vec<_> = sites
            .iter()
            .enumerate()
            .filter(|(_, site)| usize::try_from(site.layout).ok() == Some(ordinal))
            .collect();
        if pairs.is_empty() {
            continue;
        }
        pairs.sort_by_key(|(_, site)| (site.wire != UNKNOWN_WIRE_INDEX, site.wire));
        handled.extend(pairs.iter().map(|(position, _)| *position));
        let fallback: Vec<_> = pairs
            .iter()
            .map(|(position, _)| group.blocks[*position].clone())
            .collect();
        let start = response
            .segments
            .first()
            .map_or(response.anchor, |site| site.start);
        let end = response
            .segments
            .last()
            .map_or(response.anchor, |site| site.start.saturating_add(site.len));
        let layout = group
            .layouts
            .get(ordinal)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let rebuild = rebuild_response(content, group, response, ordinal, layout)
            .filter(|_| (start..end).all(|position| !consumed.contains(&position)));
        if let Some(rebuild) = rebuild {
            consumed.extend(start..end);
            splices.entry(start).or_default().extend(rebuild);
        } else {
            // Missing carrier/owner or edited history: pairs retain this
            // response's explicit boundary. No earlier text is searched.
            tracing::warn!(response_id = %response.id,
                "hosted layout cannot validate its named segments; replaying response-scoped pairs only");
            splices
                .entry(end.min(content.len()))
                .or_default()
                .extend(fallback);
        }
    }
    // Legacy v1/v2 have no response identity. Their citations are never used.
    let legacy: Vec<_> = group
        .blocks
        .iter()
        .enumerate()
        .filter(|(position, _)| !handled.contains(position))
        .map(|(_, block)| block.clone())
        .collect();
    splices.entry(content.len()).or_default().extend(legacy);
    let mut rebuilt = Vec::new();
    for position in 0..=content.len() {
        if let Some(blocks) = splices.remove(&position) {
            rebuilt.extend(blocks);
        }
        if position < content.len() && !consumed.contains(&position) {
            rebuilt.push(content[position].clone());
        }
    }
    *content = rebuilt;
}

fn rebuild_response(
    content: &[Value],
    group: &ReplayGroup,
    response: &ResponseSite,
    ordinal: usize,
    layout: &[Value],
) -> Option<Vec<Value>> {
    if layout.is_empty() || !crate::hosted_replay::valid_layout(layout, &response.id) {
        return None;
    }
    let mut segments = HashMap::new();
    let mut expected_start = response
        .segments
        .first()
        .map_or(response.anchor, |site| site.start);
    for site in &response.segments {
        if site.start != expected_start
            || site.len == 0
            || crate::hosted_replay::segment_response(&site.id) != Some(response.id.as_str())
            || segments
                .insert(
                    site.id.as_str(),
                    content.get(site.start..site.start.checked_add(site.len)?)?,
                )
                .is_some()
        {
            return None;
        }
        expected_start = site.start.checked_add(site.len)?;
    }
    let sites = group.block_sites.as_ref()?;
    let mut pair_positions = HashMap::new();
    for (position, site) in sites.iter().enumerate() {
        if usize::try_from(site.layout).ok() == Some(ordinal)
            && site.wire != UNKNOWN_WIRE_INDEX
            && pair_positions.insert(site.wire, position).is_some()
        {
            return None;
        }
    }
    let mut pair_coverage = HashSet::new();
    let mut seen_owners = Vec::new();
    let mut text = HashMap::<&str, String>::new();
    let mut parts = HashMap::<&str, Vec<usize>>::new();
    let mut rebuild = Vec::new();
    for entry in layout {
        match entry.get("kind").and_then(Value::as_str) {
            Some("pair") => {
                let wire = entry.get("index")?.as_u64()?;
                let position = *pair_positions.get(&wire)?;
                if !pair_coverage.insert(wire) {
                    return None;
                }
                rebuild.push(group.blocks.get(position)?.clone());
            }
            Some(kind @ ("text" | "cited" | "segment")) => {
                let owner = entry.get("owner")?.as_str()?;
                let original = segments.get(owner)?;
                if seen_owners.last().copied() != Some(owner) {
                    seen_owners.push(owner);
                }
                if kind == "segment" {
                    let part = usize::try_from(entry.get("part")?.as_u64()?).ok()?;
                    parts.entry(owner).or_default().push(part);
                    let block = original.get(part)?;
                    if !matches!(
                        block["type"].as_str(),
                        Some("thinking" | "redacted_thinking" | "tool_use")
                    ) {
                        return None;
                    }
                    rebuild.push(block.clone());
                } else {
                    let block = entry.get("block")?;
                    text.entry(owner)
                        .or_default()
                        .push_str(block.get("text")?.as_str()?);
                    rebuild.push(block.clone());
                }
            }
            _ => return None,
        }
    }
    // Full coverage after filtering: every accepted pair exactly once, every
    // named segment exactly once and in its persisted order. No cross-boundary
    // concatenation, duplicate reasoning parts, or omitted tool calls.
    if pair_coverage.len() != pair_positions.len()
        || seen_owners
            != response
                .segments
                .iter()
                .map(|site| site.id.as_str())
                .collect::<Vec<_>>()
    {
        return None;
    }
    for (owner, original) in segments {
        if let Some(expected) = text.remove(owner) {
            if parts.contains_key(owner) || original.iter().any(|block| block["type"] != "text") {
                return None;
            }
            let actual: Option<String> = original
                .iter()
                .map(|block| block.get("text")?.as_str())
                .collect();
            if actual.as_deref() != Some(expected.as_str()) {
                return None;
            }
        } else if parts.remove(owner)? != (0..original.len()).collect::<Vec<_>>() {
            return None;
        }
    }
    Some(rebuild)
}

#[cfg(test)]
#[path = "transport_identity_tests.rs"]
mod tests;
