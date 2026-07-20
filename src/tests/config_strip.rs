//! `strip_unsupported` — the per-row request-body strip (config §4.1.1), the
//! inverse of `body_defaults`. The Codex backend 400s on `temperature`/`top_p`/
//! `max_output_tokens`; a row lists the CANONICAL fields and brazen drops each from
//! the request after `fill_absent`, so the encoder never emits them (bl-d54a).

use crate::{
    defaults, fill_absent, strip_unsupported, CanonicalRequest, PartialConfig, ResolvedConfig,
};
use serde_json::json;

/// A row carrying `unsupported_body_keys`, resolved through the production fold so
/// the list reaches `cfg.provider` exactly as `serve` sees it.
fn row_with_unsupported(keys: &str) -> ResolvedConfig {
    let file = crate::parse_config(&format!(
        "[[provider]]\nname = \"codex\"\nbase_url = \"u\"\nprotocol = \"openai_responses\"\nauth = \"bearer\"\napi_header = {{ name = \"Authorization\", scheme = \"bearer\" }}\nunsupported_body_keys = [{keys}]\n",
    ))
    .unwrap();
    PartialConfig {
        provider: Some("codex".into()),
        ..Default::default()
    }
    .or(file)
    .or(defaults())
    .into_resolved(Some("gpt-5.4"), None)
    .unwrap()
}

#[test]
fn strip_unsupported_drops_each_listed_field_whatever_its_source() {
    // The Codex backend 400s on temperature/top_p/max_output_tokens (bl-d54a, bl-73d8).
    // The row names CANONICAL fields; the gen trio clears the typed fields, a non-gen
    // key clears the `extra` valve — run AFTER fill_absent, so even an EXPLICIT request
    // value (not just a config default) is dropped.
    let cfg = row_with_unsupported(
        "\"max_tokens\", \"temperature\", \"top_p\", \"reasoning\", \"output\", \"frequency_penalty\"",
    );
    assert_eq!(
        cfg.provider.unsupported_body_keys,
        vec![
            "max_tokens".to_string(),
            "temperature".into(),
            "top_p".into(),
            "reasoning".into(),
            "output".into(),
            "frequency_penalty".into()
        ]
    );
    let mut req: CanonicalRequest = serde_json::from_value(json!({
        "model": "gpt-5.4",
        "messages": [],
        "max_tokens": 256,
        "temperature": 0.5,
        "top_p": 0.9,
        "reasoning": "high",
        "output": {"type": "json"},
        "frequency_penalty": 0.2,
    }))
    .unwrap();
    fill_absent(&mut req, &cfg);
    strip_unsupported(&mut req, &cfg);
    assert_eq!(req.max_tokens, None); // typed gen field cleared
    assert_eq!(req.temperature, None);
    assert_eq!(req.top_p, None);
    assert_eq!(req.reasoning, None); // the lifted reasoning knob cleared (config §4.1.1)
    assert_eq!(req.output, None); // the lifted structured-output knob cleared (config §4.1.1)
    assert_eq!(req.extra.get("frequency_penalty"), None); // non-gen key cleared from `extra`
}

#[test]
fn strip_unsupported_is_a_no_op_for_a_row_that_pins_nothing() {
    // The standard path: no `unsupported_body_keys` → the loop never runs, every field
    // survives. Severability — the behavior is exactly the row datum, nothing more.
    let cfg = PartialConfig {
        provider: Some("anthropic".into()),
        ..Default::default()
    }
    .or(defaults())
    .into_resolved(Some("m"), None)
    .unwrap();
    assert!(cfg.provider.unsupported_body_keys.is_empty());
    let mut req = CanonicalRequest {
        max_tokens: Some(7),
        temperature: Some(0.9),
        top_p: Some(0.1),
        ..Default::default()
    };
    fill_absent(&mut req, &cfg);
    strip_unsupported(&mut req, &cfg);
    assert_eq!(req.max_tokens, Some(7)); // untouched
    assert_eq!(req.temperature, Some(0.9));
    assert_eq!(req.top_p, Some(0.1));
}
