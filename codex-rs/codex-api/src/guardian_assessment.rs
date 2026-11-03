//! Decodes action-bound Guardian metadata without granting permission to execute.
//! Missing or malformed assessments remain unavailable; consumers must validate the
//! response, completed action, and current authorization before using a low-risk result.

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

/// One final serving assessment emitted immediately before the matching output item is done.
/// It is carried outside model-visible response items and is not subsequently revised.
///
/// Consumers must validate the response, completed action, and current authorization
/// before using a low-risk result. This assessment does not grant permission to execute.
// Fork (nuwax-codex): serde derives added — the fork serializes `ResponseEvent`
// (persistence/replay), which carries this type in one variant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardianAssessment {
    pub response_id: String,
    pub item_id: String,
    /// False only permits considering the action for the Guardian fast path.
    pub requires_review: bool,
}

pub(crate) fn parse_guardian_assessment(
    response_id: Option<&str>,
    metadata: Option<&Value>,
) -> Option<GuardianAssessment> {
    let response_id = response_id?;
    let guardian = metadata?.get("guardian")?;
    let item_id = guardian.get("item_id")?.as_str()?;
    let requires_review = guardian.get("requires_review")?.as_bool()?;
    if response_id.is_empty() || item_id.is_empty() {
        return None;
    }
    Some(GuardianAssessment {
        response_id: response_id.to_owned(),
        item_id: item_id.to_owned(),
        requires_review,
    })
}
