use super::features_schema;
use codex_features::CapPartialDiagnosticsConfigToml;
use codex_features::FeatureToml;
use codex_features::FeaturesToml;
use pretty_assertions::assert_eq;
use schemars::r#gen::SchemaGenerator;
use serde_json::json;

#[test]
fn cap_diagnostics_schema_matches_both_accepted_config_forms() {
    let mut generator = SchemaGenerator::default();
    let schema = features_schema(&mut generator);
    let expected = generator.subschema_for::<FeatureToml<CapPartialDiagnosticsConfigToml>>();
    let actual = schema
        .into_object()
        .object
        .expect("feature map")
        .properties
        .remove("cap_partial_diagnostics")
        .expect("feature schema");
    assert_eq!(actual, expected);

    for enabled in [false, true] {
        let boolean: FeaturesToml = serde_json::from_value(json!({
            "cap_partial_diagnostics": enabled,
        }))
        .expect("boolean config");
        let structured: FeaturesToml = serde_json::from_value(json!({
            "cap_partial_diagnostics": {"enabled": enabled},
        }))
        .expect("structured config");
        assert_eq!(boolean.entries(), structured.entries());
        assert_eq!(
            boolean.entries().get("cap_partial_diagnostics"),
            Some(&enabled)
        );
    }
}
