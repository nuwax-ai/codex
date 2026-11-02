//! Costs distinguish evidence payloads from complete model request estimates.

use super::*;
use crate::Budgeted;
use crate::composition::SectionOutput;
use crate::composition::user_message as message;
use codex_protocol::models::ImageReference;
use pretty_assertions::assert_eq;

#[test]
fn request_token_ranges_keep_boundary_values_in_the_lower_bucket() {
    let tokens = [
        0,
        19_999,
        20_000,
        20_001,
        40_000,
        40_001,
        240_000,
        240_001,
        256_000,
        256_001,
        usize::MAX,
    ];
    assert_eq!(
        tokens.map(request_token_bucket),
        [
            "01_0-20k",
            "01_0-20k",
            "01_0-20k",
            "02_20k-40k",
            "02_20k-40k",
            "03_40k-60k",
            "12_220k-240k",
            "13_240k-256k",
            "13_240k-256k",
            "14_256k-max",
            "14_256k-max"
        ]
    );
}

#[test]
fn section_costs_keep_multimodal_payloads_separate() {
    let context = ComposedContext {
        sections: vec![
            SectionOutput {
                id: "transcript",
                delivery: SectionDelivery::user_content(vec![
                    Budgeted::required(ContentItem::InputText {
                        text: "évidence".to_owned(),
                    }),
                    Budgeted::required(ContentItem::InputImage {
                        image: ImageReference::Inline {
                            image_url: "data:image/png;base64,AAAA".to_owned(),
                        },
                        detail: None,
                    }),
                    Budgeted::required(ContentItem::InputImage {
                        image: ImageReference::File {
                            file_id: "file_123".to_owned(),
                        },
                        detail: None,
                    }),
                ]),
            },
            SectionOutput {
                id: "trusted",
                delivery: SectionDelivery::Message(crate::Budgeted::required(Box::new(
                    message(vec![ContentItem::InputText {
                        text: "verified".to_owned(),
                    }])
                    .into(),
                ))),
            },
        ],
        truncations: Vec::new(),
    };
    assert_eq!(
        context.section_costs().collect::<Vec<_>>(),
        vec![
            (
                "transcript",
                SectionCost {
                    text_bytes: "évidence".len(),
                    image_bytes: "data:image/png;base64,AAAA".len(),
                    image_count: 2
                }
            ),
            (
                "trusted",
                SectionCost {
                    text_bytes: "verified".len(),
                    ..SectionCost::default()
                }
            ),
        ]
    );
}

#[test]
fn request_estimate_reserves_images_independently_of_encoded_size() {
    let image = |payload: &str| {
        message(vec![ContentItem::InputImage {
            image: ImageReference::Inline {
                image_url: format!("data:image/png;base64,{payload}"),
            },
            detail: None,
        }])
    };
    let short = estimate_input_tokens(&image("AAAA"));
    assert_eq!(short, estimate_input_tokens(&image(&"A".repeat(200_000))));
    assert!(short >= IMAGE_TOKEN_RESERVATION);

    let file = message(vec![ContentItem::InputImage {
        image: ImageReference::File {
            file_id: "file_123".to_owned(),
        },
        detail: Some(codex_protocol::models::ImageDetail::Original),
    }]);
    assert!(estimate_input_tokens(&file) >= IMAGE_TOKEN_RESERVATION);
}

#[test]
fn section_estimate_bounds_the_delivered_message() {
    // Account for both JSON escaping layers and UTF-8-safe chunk boundaries.
    for format in [crate::TranscriptFormat::Line, crate::TranscriptFormat::Json] {
        for (text, max_count) in [
            ("x".to_owned(), 40),
            ("quoted \"text\"".to_owned(), 40),
            ("é🙂\n\0\\".repeat(20_000), 1),
        ] {
            let mut record = crate::TranscriptRecord::new(
                &crate::ConversationTranscriptEntryKind::Assistant,
                /*index*/ 7,
                text,
                /*retained_source_order*/ None,
                format,
                /*suffix*/ "",
            );
            // Composition changes framing after preparation has populated the cache.
            record.frame_for_sync("\n");
            let expected = ContentItem::InputText {
                text: record.to_string(),
            };
            assert_eq!(record.rendered().tokens, content_tokens(&expected));
            assert_eq!(record.clone().into_content_item(), expected);
            // Individually rounded costs must still leave room for separators.
            for count in 1..=max_count {
                let context = ComposedContext {
                    sections: vec![SectionOutput {
                        id: "transcript",
                        delivery: SectionDelivery::UserContent(vec![
                            Budgeted::required(
                                record.clone().into()
                            );
                            count
                        ]),
                    }],
                    truncations: Vec::new(),
                };
                assert_eq!(
                    context.section_costs().collect::<Vec<_>>(),
                    vec![(
                        "transcript",
                        SectionCost {
                            text_bytes: record.to_string().len() * count,
                            ..SectionCost::default()
                        }
                    )]
                );
                let estimate = context.estimated_tokens();
                assert!(estimate >= estimate_input_tokens(&context.into_messages()[0]));
            }
        }
    }
}

// Fork (nuwax-codex): the fallback clamp must honor an explicitly configured
// window (Spec §6.1) — an operator who set the real window owns the best
// information about the model.
mod effective_input_token_limit_tests {
    use crate::budget::effective_input_token_limit;
    use codex_protocol::openai_models::ModelInfo;
    use serde_json::json;

    fn model_info(window: Option<i64>, fallback: bool, percent: i64) -> ModelInfo {
        let mut value = json!({
            "slug": "test-model",
            "display_name": "Test Model",
            "supported_reasoning_levels": [],
            "shell_type": "unified_exec",
            "visibility": "none",
            "supported_in_api": true,
            "priority": 99, "experimental_supported_tools": [], "support_verbosity": false,
            "additional_speed_tiers": [],
            "service_tiers": [],
            "default_reasoning_summary": "auto",
            "web_search_tool_type": "text",
            "truncation_policy": {"mode": "bytes", "limit": 10000},
        });
        if let Some(window) = window {
            value["context_window"] = json!(window);
            value["max_context_window"] = json!(window);
        }
        value["effective_context_window_percent"] = json!(percent);
        let mut model: ModelInfo =
            serde_json::from_value(value).expect("a ModelInfo from partial JSON");
        // Internal-only marker: skipped by serde, set directly.
        model.used_fallback_model_metadata = fallback;
        model
    }

    #[test]
    fn a_fallback_model_without_a_configured_window_is_clamped() {
        let model = model_info(Some(1_000_000), /*fallback*/ true, 100);
        assert_eq!(
            effective_input_token_limit(&model, /*configured*/ None),
            128_000
        );
    }

    #[test]
    fn a_fallback_model_with_an_explicit_window_uses_it_in_full() {
        let model = model_info(Some(1_000_000), /*fallback*/ true, 95);
        assert_eq!(
            effective_input_token_limit(&model, Some(1_000_000)),
            950_000
        );
    }

    #[test]
    fn a_cataloged_model_is_never_clamped_by_the_fallback_guard() {
        let model = model_info(Some(400_000), /*fallback*/ false, 95);
        assert_eq!(
            effective_input_token_limit(&model, /*configured*/ None),
            380_000
        );
    }
}
