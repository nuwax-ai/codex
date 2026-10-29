//! The Anthropic wire's reasoning control, routed through rig's typed
//! `GenerationOptions` channel instead of a raw body injection.
//!
//! Why the typed channel matters: rig answers `Reasoning` from the model's
//! catalog entry (`ReasoningSupport::Listed { levels, can_disable, .. }`), and
//! an option the model cannot take fails the request build — the default
//! policy is [`OnUnsupported::Error`], "never silently dropped". So an effort
//! level the model does not take, or turning thinking off on a model that
//! cannot, is refused *before* anything is sent. Injecting
//! `output_config.effort` into the serialized body instead skips that check
//! entirely: the gateway may accept an unknown level and silently apply a
//! different one, which is how `medium` reached GLM-5.3-Flash (a model whose
//! real tiers are `low`/`high`/`max`) and came back as low-effort output.
//!
//! The tier list is ours to declare, not rig's catalog's: a fork provider's
//! model is spelled however its gateway spells it, so `ProviderInfo` carries
//! the levels explicitly (see `reasoning_effort_levels`). Without them the
//! wire stays permissive — rig's default for a model it does not list.
//!
//! [`OnUnsupported::Error`]: rig_core::completion::OnUnsupported::Error

use std::sync::Arc;

use codex_api::ApiError;
use codex_protocol::openai_models::ReasoningEffort;
use rig_core::catalog::ModelSpec;
use rig_core::catalog::ReasoningSupport;
use rig_core::completion::Effort;
use rig_core::completion::Reasoning;
use rig_core::providers::registry::ProviderId;
use serde::de::Deserialize;

/// Codex's effort vocabulary onto rig's portable levels. `Ultra` has no rig
/// level (Anthropic's scale stops at `max`), so it clamps with a warning
/// rather than inventing a wire word; `None`, `Persistent` and `Custom` carry
/// no level and map to `Reasoning::Off`, which rig answers per model.
pub(crate) fn rig_effort(effort: &ReasoningEffort) -> Option<Effort> {
    match effort {
        ReasoningEffort::Minimal | ReasoningEffort::Low => Some(Effort::Low),
        ReasoningEffort::Medium => Some(Effort::Medium),
        ReasoningEffort::High => Some(Effort::High),
        ReasoningEffort::XHigh => Some(Effort::XHigh),
        ReasoningEffort::Max | ReasoningEffort::Ultra => {
            if matches!(effort, ReasoningEffort::Ultra) {
                tracing::warn!(
                    "reasoning effort `ultra` has no Anthropic level above `max`; clamping to `max`"
                );
            }
            Some(Effort::Max)
        }
        ReasoningEffort::None | ReasoningEffort::Persistent | ReasoningEffort::Custom(_) => None,
    }
}

/// The wire's `Reasoning` for a codex effort: a level when the effort names
/// one, otherwise `Off` (no level to send). No effort configured says nothing
/// and lets the model default.
pub(crate) fn rig_reasoning(effort: Option<&ReasoningEffort>) -> Option<Reasoning> {
    effort.map(|effort| match rig_effort(effort) {
        Some(level) => Reasoning::Effort(level),
        None => Reasoning::Off,
    })
}

/// The tiers `words` names, as rig's effort levels, in the order given and
/// without repeats. `None` — the field unset — is an empty list, which leaves
/// the wire permissive.
///
/// `source` is where the list came from — `NUWAX_REASONING_EFFORT_LEVELS` or
/// `model_providers.<id>.reasoning_effort_levels` — so a rejected word says
/// which knob to fix. Each word is read through rig's own `Effort`
/// deserializer, the same one rig reads a catalog row's effort values with
/// (`rig-core/src/catalog/row.rs`), so the accepted vocabulary is rig's and
/// cannot drift from it: a word rig does not spell fails here, naming rig's
/// own list, instead of reaching the gateway.
///
/// A list that is set but names no tier is refused rather than returned: it
/// would refuse every effort, which is indistinguishable in effect from a typo
/// and nothing like leaving the field unset.
pub(crate) fn parse_levels(
    source: &str,
    words: Option<&[String]>,
) -> Result<Vec<Effort>, ApiError> {
    let Some(words) = words else {
        return Ok(Vec::new());
    };
    let mut levels: Vec<Effort> = Vec::with_capacity(words.len());
    for word in words {
        let level = rig_level(source, word)?;
        if !levels.contains(&level) {
            levels.push(level);
        }
    }
    if levels.is_empty() {
        return Err(ApiError::InvalidRequest {
            message: format!(
                "{source} declares no effort tier, which would refuse every effort; \
                 list the tiers the model takes, such as `low,high,max`, or unset the field \
                 to leave the wire permissive"
            ),
        });
    }
    Ok(levels)
}

/// Refuses an effort the declared tiers do not cover, before the request is
/// built.
///
/// This is the Chat wire's gate. Chat cannot take rig's typed channel — rig's
/// `Rewrite::OutputCapRename` reads the same `reasons(facts, model)` answer
/// that the typed channel turns on, and renames `max_tokens` to the
/// OpenAI-only `max_completion_tokens`, which a compatible gateway ignores, so
/// the configured output cap would silently stop applying. rig couples the two
/// on purpose ("this same wire reaches compatible servers"), and an unbound
/// wire is what keeps the rename scoped. So the tier list is checked here
/// instead, in rig's vocabulary: the list is our configuration, so the rule
/// that governs it is ours to state — only the words are rig's.
///
/// `declared` empty means nothing is declared, which stays permissive.
pub(crate) fn check_declared(
    source: &str,
    declared: &[Effort],
    requested: Option<&ReasoningEffort>,
) -> Result<(), ApiError> {
    if declared.is_empty() {
        return Ok(());
    }
    let Some(effort) = requested else {
        return Ok(());
    };
    match rig_effort(effort) {
        Some(level) if declared.contains(&level) => Ok(()),
        Some(level) => Err(ApiError::InvalidRequest {
            message: format!(
                "{source} do not include `{}`; this model takes {}",
                level.as_str(),
                tier_list(declared)
            ),
        }),
        // `none`, `persistent` and an unknown custom name carry no level. On
        // the Anthropic wire rig answers this from `can_disable`; here it is
        // the same answer stated once, since these gateways always think.
        None => Err(ApiError::InvalidRequest {
            message: format!(
                "{source} say this gateway's models always think, so reasoning cannot be \
                 turned off; this model takes {}",
                tier_list(declared)
            ),
        }),
    }
}

fn tier_list(declared: &[Effort]) -> String {
    if declared.is_empty() {
        return "no effort tier".to_owned();
    }
    declared
        .iter()
        .map(|level| format!("`{}`", level.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn rig_level(source: &str, word: &str) -> Result<Effort, ApiError> {
    // Answered apart from the deserializer: rig reads a `none` in a vendor's
    // effort list as "thinking can be turned off", a different axis from the
    // tiers themselves, and the gateways this bridge serves are
    // always-thinking. Silently dropping the word would leave the operator
    // believing they configured something.
    if matches!(word, "none" | "off") {
        return Err(ApiError::InvalidRequest {
            message: format!(
                "{source} lists `{word}`, which is not an effort tier: it says whether \
                 thinking can be turned off, and these gateways are always-thinking. \
                 List the tiers the model takes, such as `low,high,max`."
            ),
        });
    }
    use serde::de::IntoDeserializer;
    let word_de: serde::de::value::StrDeserializer<'_, serde::de::value::Error> =
        word.into_deserializer();
    Effort::deserialize(word_de).map_err(|error| ApiError::InvalidRequest {
        message: format!("{source} lists an effort tier rig does not spell, `{word}`: {error}"),
    })
}

/// A `ModelSpec` for `model` as the `vendor` wire speaks it, declaring the
/// tiers `levels` names. An empty `levels` means no tiers are declared, so rig
/// answers as it does for a model it does not list. `None` only if the vendor
/// name is refused, which a literal cannot be.
///
/// The spec starts from rig's own catalog row for the model when it has one:
/// the row decides the wire's shape — `compat.adaptive_thinking` (whether an
/// effort travels with a `thinking` sibling), `binds_context`, the output
/// limit — and a fork gateway that serves a real Claude or GPT id must keep
/// those facts. A gateway's own model has no row and starts from "nothing
/// known", whose `adaptive_thinking` is false: `output_config.effort` alone,
/// which is the shape these gateways take.
///
/// Binding matters even with no tiers declared. Left unbound, rig answers from
/// its catalog, and for an Anthropic-dialect model it does not list that means
/// the newest models' shape — `thinking: {"type": "adaptive"}` beside the
/// effort — which is not what a GLM or MiMo endpoint takes. So the same
/// request would change shape depending on whether the operator declared
/// tiers, which is precisely the "configured one thing, sent another" split
/// this channel exists to remove.
///
/// `can_disable` is `false` when tiers are declared: the gateways this bridge
/// serves are always-thinking models, so a request that turns reasoning off is
/// refused here rather than 400'd (or `1210`'d) downstream. The provider id is
/// *declared* rather than a catalog vendor, so the bound spec answers under
/// any vendor a wire asks about — ours is a custom base URL, not rig's row.
pub(crate) fn effort_spec(vendor: &str, model: &str, levels: &[Effort]) -> Option<Arc<ModelSpec>> {
    let declared = ProviderId::declare("nuwax").ok()?;
    let catalog_vendor = ProviderId::catalog(vendor)?;
    let mut spec = rig_core::catalog::Catalog::builtin()
        .get(catalog_vendor, model)
        .map(|resolved| resolved.shared())
        .unwrap_or_else(|| Arc::new(ModelSpec::new(declared.clone(), model)));
    // `make_mut` copies a row the catalog shares, so rig's own entry is never
    // written through.
    let spec = Arc::make_mut(&mut spec);
    spec.provider = declared;
    spec.reasoning = if levels.is_empty() {
        ReasoningSupport::Unknown { default: None }
    } else {
        ReasoningSupport::Listed {
            levels: levels.to_vec(),
            budget: None,
            can_disable: false,
            default: None,
        }
    };
    Some(Arc::new(spec.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn levels(words: &[&str]) -> Result<Vec<Effort>, ApiError> {
        let words: Vec<String> = words.iter().map(|word| (*word).to_owned()).collect();
        parse_levels("NUWAX_REASONING_EFFORT_LEVELS", Some(&words))
    }

    /// The field unset, which must stay permissive rather than reading as
    /// "takes no tier".
    fn unset() -> Result<Vec<Effort>, ApiError> {
        parse_levels("NUWAX_REASONING_EFFORT_LEVELS", None)
    }

    fn message_of(error: &ApiError) -> String {
        match error {
            ApiError::InvalidRequest { message } => message.clone(),
            other => panic!("expected an invalid-request error, got {other:?}"),
        }
    }

    fn declared(words: &[&str]) -> Arc<ModelSpec> {
        let parsed = levels(words).expect("declared tiers");
        effort_spec("anthropic", "GLM-5.3-Flash", &parsed).expect("a declared vendor")
    }

    /// A gateway model rig does not list, with no tiers declared: the spec must
    /// stay permissive about which level it takes, while still saying the
    /// gateway's own wire shape (no `thinking` sibling beside the effort).
    fn undeclared() -> Arc<ModelSpec> {
        let empty = unset().expect("an unset field is permissive");
        effort_spec("anthropic", "GLM-5.3-Flash", &empty).expect("a declared vendor")
    }

    #[test]
    fn codex_effort_maps_onto_rig_levels() {
        assert_eq!(rig_effort(&ReasoningEffort::Low), Some(Effort::Low));
        assert_eq!(rig_effort(&ReasoningEffort::Medium), Some(Effort::Medium));
        assert_eq!(rig_effort(&ReasoningEffort::High), Some(Effort::High));
        assert_eq!(rig_effort(&ReasoningEffort::XHigh), Some(Effort::XHigh));
        assert_eq!(rig_effort(&ReasoningEffort::Max), Some(Effort::Max));
        // Ultra clamps: Anthropic's scale stops at max.
        assert_eq!(rig_effort(&ReasoningEffort::Ultra), Some(Effort::Max));
        // No level to send.
        assert_eq!(rig_effort(&ReasoningEffort::None), None);
        assert_eq!(rig_effort(&ReasoningEffort::Persistent), None);
    }

    #[test]
    fn reasoning_is_off_only_when_an_effort_names_no_level() {
        assert!(matches!(
            rig_reasoning(Some(&ReasoningEffort::High)),
            Some(Reasoning::Effort(Effort::High))
        ));
        assert!(matches!(
            rig_reasoning(Some(&ReasoningEffort::None)),
            Some(Reasoning::Off)
        ));
        // Unset stays unset: the model's own default applies.
        assert!(rig_reasoning(None).is_none());
    }

    #[test]
    fn a_declared_spec_answers_under_any_vendor() {
        let spec = declared(&["low", "high", "max"]);
        let facts = rig_core::catalog::ModelFacts::new(spec);
        // The wire asks under its own vendor ("anthropic"); a declared
        // provider's bound spec must still answer.
        let resolved = facts.for_model("anthropic", "GLM-5.3-Flash");
        assert!(resolved.is_some(), "bound spec must answer for its model");
        assert_eq!(
            resolved
                .and_then(|spec| spec.reasoning.levels())
                .map(<[Effort]>::len),
            Some(3)
        );
    }

    /// The regression this pins: an unbound spec makes rig answer from its
    /// catalog, and for an Anthropic-dialect model it does not list that is
    /// the newest models' shape — `thinking: {"type":"adaptive"}` beside the
    /// effort. A GLM or MiMo endpoint takes `output_config.effort` alone, so
    /// the gateway's own shape must survive whether or not tiers are declared.
    #[test]
    fn a_gateway_model_keeps_its_wire_shape_with_or_without_declared_tiers() {
        for spec in [declared(&["low", "high", "max"]), undeclared()] {
            assert!(
                !spec.compat.adaptive_thinking,
                "a gateway model sends no `thinking` sibling: {}",
                spec.reference()
            );
        }
    }

    /// A model rig does list keeps that row's facts, so a fork gateway serving
    /// a real Claude id is not downgraded to "nothing known" by binding.
    #[test]
    fn a_model_rig_lists_keeps_its_catalog_row() {
        // Claude Opus 4.8 is a listed Anthropic row; its adaptive thinking is
        // what makes rig pair the effort with `thinking: {"type":"adaptive"}`.
        let listed = effort_spec("anthropic", "claude-opus-4-8", &[])
            .expect("a listed model still gets a bound spec");
        assert!(
            listed.compat.adaptive_thinking,
            "the catalog row's compat must survive binding: {}",
            listed.reference()
        );
        // The bound spec answers under any vendor, declared or not.
        let facts = rig_core::catalog::ModelFacts::new(listed);
        assert!(
            facts.for_model("anthropic", "claude-opus-4-8").is_some(),
            "the bound row must still answer"
        );
    }

    /// Binding a spec must not cost the request its images: rig reads a bound
    /// spec's modalities to decide whether to drop image blocks. The Anthropic
    /// dialect passes no naming rule, so images are accepted whatever a spec
    /// says — asserted here so a future rig change that consults it fails
    /// loudly instead of stripping every attachment.
    #[test]
    fn a_bound_spec_does_not_claim_a_gateway_model_is_text_only() {
        // rig exposes no modalities builder and `ModelSpec::new` starts
        // text-only, so this is a statement about what rig reads rather than
        // something the bridge sets. The wire-level proof is
        // `declaring_effort_tiers_does_not_drop_images` in the wire suite.
        let spec = undeclared();
        assert!(
            !spec.input.image,
            "the bare spec is text-only; the Anthropic wire must ignore that"
        );
    }

    #[test]
    fn the_tier_words_are_read_through_rigs_own_vocabulary() {
        assert_eq!(
            levels(&["low", "high", "max"]).expect("GLM-5.3-Flash's real tiers"),
            vec![Effort::Low, Effort::High, Effort::Max]
        );
        // Repeated words collapse rather than widening the declared list.
        assert_eq!(
            levels(&["high", "high"]).expect("a repeated tier"),
            vec![Effort::High]
        );
    }

    #[test]
    fn a_tier_rig_does_not_spell_is_refused_naming_the_variable_and_rigs_words() {
        let error = levels(&["low", "hih", "max"]).expect_err("a typo must not pass");
        let message = message_of(&error);
        assert!(
            message.contains("NUWAX_REASONING_EFFORT_LEVELS"),
            "the knob to fix must be named: {message}"
        );
        assert!(
            message.contains("`hih`"),
            "the offending word must be named: {message}"
        );
        // rig's own deserializer lists what it does spell, so the accepted
        // vocabulary is never a copy kept in step by hand.
        assert!(
            message.contains("`max`") && message.contains("`xhigh`"),
            "rig's accepted words must be shown: {message}"
        );
    }

    #[test]
    fn none_is_not_a_tier_and_says_which_axis_it_belongs_to() {
        for word in ["none", "off"] {
            let error = levels(&["low", word]).expect_err("`none` is not a tier");
            let message = message_of(&error);
            assert!(
                message.contains("turned off") && message.contains("always-thinking"),
                "the message must name the axis it belongs to: {message}"
            );
        }
    }

    #[test]
    fn a_tier_the_model_lacks_is_refused_before_the_request_is_built() {
        // The whole point of the typed channel: rig's own gate, driven by the
        // declared tiers, refuses a level the model does not take.
        let facts = rig_core::catalog::ModelFacts::new(declared(&["low", "high", "max"]));
        let spec = facts
            .for_model("anthropic", "GLM-5.3-Flash")
            .expect("the bound spec answers");

        assert_eq!(
            spec.reasoning
                .refusal(&Reasoning::Effort(Effort::Medium))
                .as_deref(),
            Some("the model has no `medium` effort level")
        );
        assert_eq!(
            spec.reasoning.refusal(&Reasoning::Off).as_deref(),
            Some("reasoning cannot be turned off on this model")
        );
        for taken in [Effort::Low, Effort::High, Effort::Max] {
            assert_eq!(
                spec.reasoning.refusal(&Reasoning::Effort(taken)),
                None,
                "`{}` is a real tier and must be sent",
                taken.as_str()
            );
        }
    }

    /// The Chat wire's gate. rig's Chat dialect couples "may I validate this
    /// effort" to "rename the output cap", so the tier rule is stated here
    /// instead — in rig's vocabulary, before anything is built.
    #[test]
    fn the_chat_gate_refuses_a_tier_the_model_lacks() {
        let declared = levels(&["low", "high", "max"]).expect("GLM-5.3-Flash's tiers");
        for taken in [
            ReasoningEffort::Low,
            ReasoningEffort::High,
            ReasoningEffort::Max,
        ] {
            check_declared("NUWAX_REASONING_EFFORT_LEVELS", &declared, Some(&taken))
                .expect("a declared tier is sent");
        }
        let error = check_declared(
            "NUWAX_REASONING_EFFORT_LEVELS",
            &declared,
            Some(&ReasoningEffort::Medium),
        )
        .expect_err("medium is not a GLM-5.3-Flash tier");
        let message = message_of(&error);
        assert!(
            message.contains("`medium`") && message.contains("`max`"),
            "the missing tier and the ones the model takes must both be named: {message}"
        );
        // `ultra` clamps to `max`, which IS declared, so it is accepted.
        check_declared(
            "NUWAX_REASONING_EFFORT_LEVELS",
            &declared,
            Some(&ReasoningEffort::Ultra),
        )
        .expect("ultra clamps onto a declared tier");
    }

    #[test]
    fn the_chat_gate_refuses_turning_reasoning_off() {
        let declared = levels(&["low", "high", "max"]).expect("GLM-5.3-Flash's tiers");
        for effort in [
            ReasoningEffort::None,
            ReasoningEffort::Persistent,
            ReasoningEffort::Custom("deep".into()),
        ] {
            let error = check_declared("NUWAX_REASONING_EFFORT_LEVELS", &declared, Some(&effort))
                .expect_err("these gateways always think");
            assert!(
                message_of(&error).contains("always think"),
                "{effort:?} must be refused saying why: {}",
                message_of(&error)
            );
        }
    }

    /// Nothing declared means nothing to check — the wire stays permissive,
    /// exactly as before any of this existed.
    #[test]
    fn the_chat_gate_is_permissive_when_nothing_is_declared() {
        let undeclared = unset().expect("an unset field is permissive");
        for effort in [
            ReasoningEffort::None,
            ReasoningEffort::Medium,
            ReasoningEffort::Ultra,
        ] {
            check_declared("NUWAX_REASONING_EFFORT_LEVELS", &undeclared, Some(&effort))
                .expect("an undeclared provider checks nothing");
        }
    }

    #[test]
    fn a_list_set_but_naming_no_tier_is_refused_rather_than_permissive() {
        let error = parse_levels(
            "NUWAX_REASONING_EFFORT_LEVELS",
            Some(&["none".to_owned(), "off".to_owned()]),
        )
        .expect_err("a list of non-tiers must not pass");
        assert!(
            message_of(&error).contains("`none`"),
            "the offending word must be named: {}",
            message_of(&error)
        );
        // An empty list is set-but-empty, which is not the same as unset.
        let error = parse_levels("NUWAX_REASONING_EFFORT_LEVELS", Some(&[]))
            .expect_err("an empty list must not read as permissive");
        assert!(
            message_of(&error).contains("declares no effort tier"),
            "the message must say what an empty list would mean: {}",
            message_of(&error)
        );
    }
}
