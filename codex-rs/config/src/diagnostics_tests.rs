use super::*;

fn mask(message: &str) -> String {
    mask_type_error_values(message)
}

/// The shape that made the first implementation miss every real error: serde
/// puts the type name between `invalid type: ` and the backtick.
#[test]
fn masks_integer_value_after_type_name() {
    assert_eq!(
        mask("invalid type: integer `12345`, expected a string"),
        "invalid type: integer `<redacted>`, expected a string"
    );
}

#[test]
fn masks_boolean_value() {
    assert_eq!(
        mask("invalid type: boolean `true`, expected a string"),
        "invalid type: boolean `<redacted>`, expected a string"
    );
}

/// Strings — the actual credential type — are rendered with double quotes, not
/// backticks. A pasted key in a numeric field must not be echoed either.
#[test]
fn masks_double_quoted_string_value() {
    assert_eq!(
        mask("invalid type: string \"sk-live-secret\", expected u64"),
        "invalid type: string \"<redacted>\", expected u64"
    );
}

#[test]
fn masks_unknown_variant_value() {
    assert_eq!(
        mask("unknown variant `sk-live-secret`, expected one of `responses`, `chat`, `anthropic`"),
        "unknown variant `<redacted>`, expected one of `responses`, `chat`, `anthropic`"
    );
}

/// The expectation clause uses backticks for its alternatives; those are not
/// user input and must survive so the error stays actionable.
#[test]
fn keeps_enum_expectation_backticks() {
    let masked = mask("invalid type: string \"wrong\", expected one of `low`, `high`, `max`");
    assert_eq!(
        masked,
        "invalid type: string \"<redacted>\", expected one of `low`, `high`, `max`"
    );
    assert!(!masked.contains("wrong"));
}

#[test]
fn masks_string_value_with_escaped_quotes() {
    // Debug escaping renders an inner quote as `\"`; the mask must not stop at
    // the escaped one and leave the rest of the value visible.
    assert_eq!(
        mask("invalid type: string \"sk-\\\"secret-\\\"tail\", expected u64"),
        "invalid type: string \"<redacted>\", expected u64"
    );
}

#[test]
fn leaves_messages_without_values_untouched() {
    for message in [
        "invalid type: map, expected a string",
        "missing field `model_providers`",
        "unknown field `foo`, expected one of `bar`",
        "TOML parse error at line 3, column 9",
    ] {
        assert_eq!(
            mask(message),
            message,
            "message should be untouched: {message}"
        );
    }
}

#[test]
fn masking_is_idempotent() {
    let once = mask("invalid type: integer `12345`, expected a string");
    assert_eq!(mask(&once), once);
}

/// `ConfigLoadError`'s Display is the second surface (io::Error → app-server
/// invalid-config warning) and must mask as well.
#[test]
fn load_error_display_masks_values() {
    let error = ConfigLoadError::new(
        ConfigError::new(
            PathBuf::from("/home/u/.nuwax-harness/config.toml"),
            TextRange {
                start: TextPosition {
                    line: 7,
                    column: 11,
                },
                end: TextPosition {
                    line: 7,
                    column: 16,
                },
            },
            "invalid type: integer `987654`, expected a string",
        ),
        None,
    );
    let rendered = error.to_string();
    assert!(!rendered.contains("987654"), "value leaked: {rendered}");
    assert!(
        rendered.contains("<redacted>"),
        "value not masked: {rendered}"
    );
    assert!(
        rendered.contains("expected a string"),
        "guidance lost: {rendered}"
    );
}
