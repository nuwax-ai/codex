//! Flat-name resolution index for the Chat-Completions (`wire_api = "chat"`)
//! tool-call path.
//!
//! This is a fork-only addition kept in a dedicated module so it does not
//! interleave with upstream `registry.rs` logic — only minimal one-line hooks
//! live in `registry.rs`, which eases future merges with official codex.
//!
//! Background: codex models every tool (including MCP) as a Responses-API
//! `namespace` tool. When a provider uses `wire_api = "chat"`,
//! `codex-rust-genai-bridge` flattens each namespaced tool
//! (`ToolName::namespaced("mcp__memory", "create_entities")`) into a single
//! function name (`mcp__memory__create_entities`) because Chat Completions has
//! no namespace concept. The model then calls back with that flat string and
//! `namespace: None`. [`FlatNameIndex`] maps the flat string back to the
//! canonical namespaced `ToolName` so the registry can dispatch to the right
//! handler.
//!
//! Only namespaced tools are indexed, so plain tools are never shadowed. The
//! Responses-API path is unaffected: it supplies `namespace: Some(...)` and
//! resolves via exact-key lookup in the registry before this index is consulted.

use std::collections::HashMap;

use codex_tools::ToolName;
use codex_tools::code_mode_name_for_tool_name;

/// Maps flattened tool names back to their canonical namespaced `ToolName`.
pub(crate) struct FlatNameIndex {
    flat_by_name: HashMap<String, ToolName>,
}

impl Default for FlatNameIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl FlatNameIndex {
    pub(crate) fn new() -> Self {
        Self {
            flat_by_name: HashMap::new(),
        }
    }

    /// Records a tool under its flattened name. Only namespaced tools are
    /// indexed; plain tools are a no-op so they can never be shadowed.
    pub(crate) fn insert(&mut self, tool_name: &ToolName) {
        if tool_name.namespace.is_some() {
            let flat = code_mode_name_for_tool_name(tool_name);
            self.flat_by_name
                .entry(flat)
                .or_insert_with(|| tool_name.clone());
        }
    }

    /// Drops the flat-name entry for a tool. No-op for plain tools.
    pub(crate) fn remove(&mut self, tool_name: &ToolName) {
        if tool_name.namespace.is_some() {
            let flat = code_mode_name_for_tool_name(tool_name);
            self.flat_by_name.remove(&flat);
        }
    }

    /// Resolves a flat name (as returned by a Chat-Completions model, with
    /// `namespace: None`) back to the canonical namespaced `ToolName`.
    pub(crate) fn resolve(&self, flat_name: &str) -> Option<&ToolName> {
        self.flat_by_name.get(flat_name)
    }
}
