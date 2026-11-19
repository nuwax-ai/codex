use super::wire_output_cap;

/// Both output wires share this cap so the rule cannot drift between them.
#[test]
fn cap_is_limited_to_half_the_window() {
    assert_eq!(wire_output_cap(Some(384_000), Some(243_200)), Some(121_600));
    assert_eq!(wire_output_cap(Some(550), Some(1_100)), Some(550));
    assert_eq!(wire_output_cap(Some(900), Some(1_100)), Some(550));
}

/// No configured window: keep the request verbatim (catalog caps are only
/// dangerous next to a smaller deployment window).
#[test]
fn cap_is_untouched_without_a_window() {
    assert_eq!(wire_output_cap(Some(384_000), None), Some(384_000));
    assert_eq!(wire_output_cap(None, None), None);
    assert_eq!(wire_output_cap(Some(7), Some(0)), Some(7));
}

/// A missing request value falls back to the window-derived ceiling instead of
/// leaving the provider default unbounded.
#[test]
fn cap_defaults_to_the_ceiling_when_unset() {
    assert_eq!(wire_output_cap(None, Some(1_100)), Some(550));
}
