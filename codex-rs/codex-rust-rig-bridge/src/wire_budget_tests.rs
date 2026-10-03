use super::*;

#[test]
fn unknown_context_still_has_a_wire_limit() {
    assert!(WireBudget::default().check(MAX_WIRE_BYTES).is_ok());
    assert!(WireBudget::default().check(MAX_WIRE_BYTES + 1).is_err());
}

#[test]
fn final_estimate_reserves_the_requested_output() {
    let budget = WireBudget {
        context_window_tokens: Some(100),
        output_tokens: Some(20),
    };
    assert!(budget.check(320).is_ok());
    assert!(budget.check(321).is_err());
    assert!(
        WireBudget {
            context_window_tokens: Some(-1),
            output_tokens: None
        }
        .check(0)
        .is_err()
    );
}

#[test]
fn typed_budget_failure_survives_the_completion_error_chain() {
    let error = WireBudget::default().check(MAX_WIRE_BYTES + 1).unwrap_err();
    assert!(matches!(
        api_error(&error),
        Some(codex_api::ApiError::InvalidRequest { .. })
    ));
    let completion = rig_core::completion::request::CompletionError::RequestError(Box::new(error));
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
        },
        WireBudget {
            context_window_tokens: Some(0),
            output_tokens: None,
        },
        WireBudget {
            context_window_tokens: Some(-1),
            output_tokens: Some(20),
        },
    ] {
        let error = budget.check(0).unwrap_err();
        assert!(matches!(
            api_error(&error),
            Some(codex_api::ApiError::InvalidRequest { .. })
        ));
    }
}
