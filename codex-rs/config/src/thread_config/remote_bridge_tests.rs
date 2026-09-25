use super::*;
use pretty_assertions::assert_eq;

#[test]
fn remote_bridge_values_are_strict_and_roundtrip_with_identity() {
    for (value, expected) in [
        ("rig", codex_model_provider_info::ChatBridge::Rig),
        ("genai", codex_model_provider_info::ChatBridge::Genai),
        ("native", codex_model_provider_info::ChatBridge::Native),
    ] {
        let provider = proto::ModelProvider {
            id: "custom".into(),
            name: "Custom".into(),
            wire_api: proto::WireApi::Responses as i32,
            experimental_bridge: Some(value.into()),
            ..Default::default()
        };
        let (id, info) = model_provider_from_proto(provider).unwrap();
        assert_eq!(id, "custom");
        assert_eq!(info.provider_id.as_deref(), Some("custom"));
        assert_eq!(info.experimental_bridge, Some(expected));
    }
    let provider = proto::ModelProvider {
        id: "custom".into(),
        wire_api: proto::WireApi::Responses as i32,
        experimental_bridge: Some("Native".into()),
        ..Default::default()
    };
    assert!(model_provider_from_proto(provider).is_err());
}
