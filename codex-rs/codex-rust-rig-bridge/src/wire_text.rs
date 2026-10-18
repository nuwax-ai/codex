//! Prefer Chat's string form for plain text, retaining richer content verbatim.

use serde_json::Value;

pub(super) fn normalize_chat_text(body: &mut Value) -> bool {
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return false;
    };
    let mut changed = false;
    for message in messages {
        let Some(content) = message.get_mut("content") else {
            continue;
        };
        let Some(parts) = content.as_array().filter(|parts| !parts.is_empty()) else {
            continue;
        };
        let text: Option<Vec<_>> = parts
            .iter()
            .map(|part| {
                let part = part.as_object()?;
                if part.len() != 2 || part.get("type")?.as_str()? != "text" {
                    return None;
                }
                part.get("text")?.as_str()
            })
            .collect();
        if let Some(text) = text {
            // Rig can merge old user input, a compaction summary, and the new
            // user input into one array. Some Chat endpoints do not reliably
            // follow its final part; the string form retains the same order.
            *content = Value::String(text.join("\n\n"));
            changed = true;
        }
    }
    changed
}
