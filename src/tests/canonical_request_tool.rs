//! The `Tool` wire pair (§3.1, CR-4): the hand-rolled serde keyed on the PRESENCE
//! of the `type` key — no `type` decodes `Custom` byte-compatibly with the
//! pre-enum wire, a `type` decodes `Provider` whose `config` captures every
//! remaining key verbatim (the open-set carry).

use crate::Tool;
use serde_json::json;

fn rt(t: &Tool) -> Tool {
    serde_json::from_str(&serde_json::to_string(t).unwrap()).unwrap()
}

#[test]
fn tool_custom_roundtrips_with_and_without_description() {
    // No `type` key on the wire → Custom; the re-serialize is byte-stable and
    // omits an absent description (the pre-enum wire bytes exactly).
    let with = Tool::Custom {
        name: "search".into(),
        description: Some("web search".into()),
        input_schema: json!({"type": "object"}),
    };
    assert_eq!(rt(&with), with);
    let bare_wire = r#"{"name":"x","input_schema":{"type":"object"}}"#;
    let bare: Tool = serde_json::from_str(bare_wire).unwrap();
    assert_eq!(
        bare,
        Tool::Custom {
            name: "x".into(),
            description: None,
            input_schema: json!({"type": "object"}),
        }
    );
    assert_eq!(serde_json::to_string(&bare).unwrap(), bare_wire);
    // Custom preserves the pre-enum strictness: input_schema is required.
    assert!(serde_json::from_str::<Tool>(r#"{"name":"x"}"#).is_err());
}

#[test]
fn tool_provider_carries_type_name_and_every_config_key() {
    // A `type` key on the wire → Provider; config captures EVERY remaining key,
    // so unknown provider config (max_uses, user_location, …) survives verbatim.
    let t: Tool = serde_json::from_str(
        r#"{"type":"web_search_20250305","name":"web_search","max_uses":5,
            "user_location":{"type":"approximate","city":"NYC"}}"#,
    )
    .unwrap();
    let Tool::Provider { kind, name, config } = &t else {
        panic!("expected Provider, got {t:?}");
    };
    assert_eq!(kind, "web_search_20250305");
    assert_eq!(name, "web_search");
    assert_eq!(config.get("max_uses"), Some(&json!(5)));
    assert_eq!(
        config.get("user_location"),
        Some(&json!({"type": "approximate", "city": "NYC"}))
    );
    assert_eq!(config.len(), 2); // type/name never land in config
    assert_eq!(rt(&t), t);
    // Re-serializes to {"type","name",...config} — NO input_schema/description.
    let v = serde_json::to_value(&t).unwrap();
    assert_eq!(
        v,
        json!({"type": "web_search_20250305", "name": "web_search", "max_uses": 5,
               "user_location": {"type": "approximate", "city": "NYC"}})
    );
    assert!(v.get("input_schema").is_none());
    assert!(v.get("description").is_none());
}
