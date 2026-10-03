//! Whole-pair filtering and independent full-layout request budgets.

use crate::hosted_replay::MAX_LAYOUT_BYTES_PER_REQUEST;
use crate::hosted_replay::MAX_LAYOUTS_PER_REQUEST;
use crate::hosted_replay::MAX_PAIR_BYTES;
use crate::hosted_replay::MAX_REPLAY_PAIRS_PER_REQUEST;
use crate::hosted_replay::ReplayGroup;
use crate::hosted_replay::UNKNOWN_WIRE_INDEX;
use crate::hosted_replay::valid_call_id;
use crate::hosted_replay::valid_layout;
use crate::hosted_replay::valid_result;
use serde_json::Value;
use serde_json::json;
use std::collections::HashSet;

/// Budgets complete calls/results across response boundaries without moving
/// either half. Layout budgets are independent of pair dedupe and pair caps.
pub(crate) fn sanitize_for_request(mut groups: Vec<ReplayGroup>) -> Vec<ReplayGroup> {
    let mut pairs: Vec<Vec<(usize, usize)>> = Vec::new();
    let mut calls = std::collections::HashMap::<String, usize>::new();
    for (position, group) in groups.iter().enumerate() {
        if group
            .block_sites
            .as_ref()
            .is_some_and(|sites| sites.len() != group.blocks.len())
        {
            continue;
        }
        for (block_index, block) in group.blocks.iter().enumerate() {
            match block.get("type").and_then(Value::as_str) {
                Some("server_tool_use") => {
                    if let Some(id) = valid_call_id(block)
                        && let std::collections::hash_map::Entry::Vacant(entry) =
                            calls.entry(id.to_string())
                    {
                        entry.insert(pairs.len());
                        pairs.push(vec![(position, block_index)]);
                    }
                }
                Some("web_search_tool_result" | "tool_result") => {
                    if let Some(pair) = block
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .and_then(|id| calls.get(id))
                        .map(|index| &mut pairs[*index])
                        && pair.len() == 1
                    {
                        if valid_result(block) {
                            pair.push((position, block_index));
                        } else {
                            pair.clear();
                        }
                    }
                }
                _ => {}
            }
        }
    }
    pairs.retain(|pair| {
        if pair.is_empty() {
            return false;
        }
        let blocks: Vec<_> = pair
            .iter()
            .map(|(position, index)| &groups[*position].blocks[*index])
            .collect();
        let mut positions = HashSet::new();
        let cited_text: Vec<_> = pair
            .iter()
            .filter(|(position, _)| positions.insert(*position))
            .flat_map(|(position, _)| &groups[*position].cited_text)
            .collect();
        serde_json::to_vec(&json!({"blocks":blocks,"cited_text":cited_text}))
            .is_ok_and(|bytes| bytes.len() <= MAX_PAIR_BYTES)
    });
    if pairs.len() > MAX_REPLAY_PAIRS_PER_REQUEST {
        let dropped = pairs.len() - MAX_REPLAY_PAIRS_PER_REQUEST;
        tracing::warn!(
            dropped,
            "web-search request pair cap exceeded; oldest pairs dropped whole"
        );
        pairs.drain(..dropped);
    }
    let kept: HashSet<_> = pairs.into_iter().flatten().collect();
    let mut layout_count = 0usize;
    let mut layout_bytes = 0usize;
    for (position, group) in groups.iter_mut().enumerate() {
        let mut blocks = Vec::new();
        let mut sites = Vec::new();
        for (index, block) in std::mem::take(&mut group.blocks).into_iter().enumerate() {
            if kept.contains(&(position, index)) {
                blocks.push(block);
                if let Some(site) = group
                    .block_sites
                    .as_ref()
                    .and_then(|sites| sites.get(index))
                {
                    sites.push(site.clone());
                }
            }
        }
        group.blocks = blocks;
        if group.block_sites.is_some() {
            group.block_sites = Some(sites);
        }
        let active: HashSet<_> = group
            .block_sites
            .iter()
            .flatten()
            .map(|site| site.layout)
            .collect();
        let mut remap = std::collections::HashMap::new();
        let mut responses = Vec::new();
        let mut layouts = Vec::new();
        for (ordinal, response) in std::mem::take(&mut group.responses).into_iter().enumerate() {
            let Some(old) = u32::try_from(ordinal).ok() else {
                continue;
            };
            if active.contains(&old) {
                remap.insert(old, u32::try_from(responses.len()).unwrap_or(u32::MAX));
                responses.push(response);
                layouts.push(group.layouts.get(ordinal).cloned().unwrap_or_default());
            }
        }
        group.responses = responses;
        group.layouts = layouts;
        if let Some(sites) = &mut group.block_sites {
            for site in sites {
                site.layout = remap.get(&site.layout).copied().unwrap_or(u32::MAX);
            }
        }
        for (ordinal, layout) in group.layouts.iter_mut().enumerate() {
            let Some(response) = group.responses.get(ordinal) else {
                layout.clear();
                continue;
            };
            let owned: HashSet<_> = group
                .block_sites
                .iter()
                .flatten()
                .filter(|site| usize::try_from(site.layout).ok() == Some(ordinal))
                .map(|site| site.wire)
                .filter(|wire| *wire != UNKNOWN_WIRE_INDEX)
                .collect();
            // Filtering removes dropped/duplicate entries, then exact coverage
            // is checked. An imported missing/duplicate reference degrades whole.
            let original_valid = valid_layout(layout, &response.id);
            layout.retain(|entry| {
                entry["kind"] != "pair"
                    || entry
                        .get("index")
                        .and_then(Value::as_u64)
                        .is_some_and(|index| owned.contains(&index))
            });
            let referenced: HashSet<_> = layout
                .iter()
                .filter(|entry| entry["kind"] == "pair")
                .filter_map(|entry| entry.get("index").and_then(Value::as_u64))
                .collect();
            let bytes = serde_json::to_vec(layout).map_or(usize::MAX, |bytes| bytes.len());
            if owned.is_empty()
                || !original_valid
                || referenced != owned
                || layout_count >= MAX_LAYOUTS_PER_REQUEST
                || bytes > MAX_PAIR_BYTES
                || layout_bytes.saturating_add(bytes) > MAX_LAYOUT_BYTES_PER_REQUEST
            {
                layout.clear();
                continue;
            }
            if bytes >= 1_000 {
                tracing::warn!(
                    bytes,
                    "P0 manual review: hosted replay layout can exceed 1k tokens"
                );
            }
            layout_count += 1;
            layout_bytes += bytes;
        }
    }
    // Conversion already merges assistant groups. Preserve legacy callers that
    // construct separate groups, remapping ordinals when merging their identities.
    let mut result: Vec<ReplayGroup> = Vec::new();
    for group in groups.into_iter().filter(|group| !group.blocks.is_empty()) {
        match result.last_mut().filter(|last| last.index == group.index) {
            Some(last) => {
                let base = u32::try_from(last.responses.len()).unwrap_or(u32::MAX);
                last.blocks.extend(group.blocks);
                match (&mut last.block_sites, group.block_sites) {
                    (Some(existing), Some(mut sites)) => {
                        for site in &mut sites {
                            site.layout = site.layout.saturating_add(base);
                        }
                        existing.extend(sites);
                    }
                    _ => last.block_sites = None,
                }
                last.layouts.extend(group.layouts);
                last.responses.extend(group.responses);
                last.cited_text.extend(group.cited_text);
            }
            None => result.push(group),
        }
    }
    result
}
