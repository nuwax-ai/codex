use super::*;

#[test]
fn unknown_context_still_has_a_wire_limit() {
    assert!(
        WireBudget::default()
            .check_body(MAX_WIRE_BYTES, None)
            .is_ok()
    );
    assert!(
        WireBudget::default()
            .check_body(MAX_WIRE_BYTES + 1, None)
            .is_err()
    );
}

#[test]
fn final_estimate_reserves_the_requested_output() {
    let budget = WireBudget {
        context_window_tokens: Some(100),
        output_tokens: Some(20),
        vision_floor_tokens: 0,
    };
    assert!(budget.check_body(320, None).is_ok());
    assert!(budget.check_body(321, None).is_err());
    assert!(
        WireBudget {
            context_window_tokens: Some(-1),
            output_tokens: None,
            vision_floor_tokens: 0,
        }
        .check_body(0, None)
        .is_err()
    );
}

#[test]
fn typed_budget_failure_survives_the_completion_error_chain() {
    let error = WireBudget::default()
        .check_body(MAX_WIRE_BYTES + 1, None)
        .unwrap_err();
    assert!(matches!(
        api_error(&error),
        Some(codex_api::ApiError::InvalidRequest { .. })
    ));
    let completion = rig_core::error::ProviderError::Http(std::sync::Arc::new(error));
    assert!(matches!(
        api_error(&completion),
        Some(codex_api::ApiError::InvalidRequest { .. })
    ));
}

#[test]
fn impossible_output_reservation_is_a_configuration_error_before_network_or_compaction() {
    for budget in [
        WireBudget {
            context_window_tokens: Some(100),
            output_tokens: Some(100),
            vision_floor_tokens: 0,
        },
        WireBudget {
            context_window_tokens: Some(0),
            output_tokens: None,
            vision_floor_tokens: 0,
        },
        WireBudget {
            context_window_tokens: Some(-1),
            output_tokens: Some(20),
            vision_floor_tokens: 0,
        },
    ] {
        let error = budget.check_body(0, None).unwrap_err();
        assert!(matches!(
            api_error(&error),
            Some(codex_api::ApiError::InvalidRequest { .. })
        ));
    }
}

#[test]
fn vision_floor_raises_the_estimate_without_ever_adding_or_lowering() {
    // 400 text bytes + a 6000-byte inline payload: the walk sees 100 text
    // tokens + 1844 vision; the floor carries the original's 5120 patches.
    let costs = crate::wire_images::WireImageCosts {
        inline_payload_bytes: 6_000,
        vision_tokens: 1_844,
    };
    let wire_bytes = 400 + 6_000;
    let floored = WireBudget {
        context_window_tokens: Some(2_100),
        output_tokens: Some(64),
        vision_floor_tokens: 5_120,
    };
    // Without the floor: 100 + 1844 + 64 = 2008 ≤ 2100 would pass; the
    // floor must raise the charge to 100 + 5120 + 64 > 2100 and reject.
    let rejected = floored.check_body(wire_bytes, Some(costs)).unwrap_err();
    assert!(matches!(
        api_error(&rejected),
        Some(codex_api::ApiError::ContextWindowExceeded)
    ));
    // With room for the floor the same request dispatches unchanged.
    assert!(
        WireBudget {
            context_window_tokens: Some(6_000),
            output_tokens: Some(64),
            vision_floor_tokens: 5_120,
        }
        .check_body(wire_bytes, Some(costs))
        .is_ok()
    );
    // A floor below the walk never lowers the walk's own charge.
    assert!(
        WireBudget {
            context_window_tokens: Some(2_100),
            output_tokens: Some(64),
            vision_floor_tokens: 1,
        }
        .check_body(wire_bytes, Some(costs))
        .is_ok()
    );
}
