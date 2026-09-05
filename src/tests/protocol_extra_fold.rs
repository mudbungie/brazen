//! The shared `extra` fold (`protocol::json::fold_extra`, protocol-dedup D1): the one
//! home of "typed fields win" every `encode` calls. A key the encoder never wrote is
//! inserted whole; a key it wrote is kept — EXCEPT that two objects merge ONE LEVEL, so
//! the typed value wins per second-level key and a passthrough key survives beside it.
//! Without the merge the row's one valve for a NESTING dialect was dropped whole the
//! moment any typed gen scalar was set, which is every agent request (bl-f19d).

use crate::protocol::google_genai::GoogleGenAi;
use crate::protocol::ollama_chat::OllamaChat;
use crate::{CanonicalRequest, Protocol, ProviderCtx};
use serde_json::{json, Value};

fn ctx() -> ProviderCtx<'static> {
    ProviderCtx {
        base_url: "http://localhost:11434",
        model: "m",
        beta_headers: &[],
        exec: None,
    }
}

/// The request body an `ollama_chat` encode produces for `v` (its unmodeled top-level
/// keys land in `req.extra`, exactly as a row's `body_defaults` seeds them).
fn ollama(v: Value) -> Value {
    let req: CanonicalRequest = serde_json::from_value(v).unwrap();
    serde_json::from_slice(&OllamaChat.encode(&req, &ctx()).unwrap().body).unwrap()
}

fn google(v: Value) -> Value {
    let req: CanonicalRequest = serde_json::from_value(v).unwrap();
    serde_json::from_slice(&GoogleGenAi.encode(&req, &ctx()).unwrap().body).unwrap()
}

#[test]
fn a_passthrough_object_merges_into_the_typed_options_leaf_by_leaf() {
    // The bl-f19d case: an agent harness always sets `max_tokens`, so the encoder has
    // already written `options` when the fold runs. The row's `num_ctx` now lands
    // BESIDE the typed `num_predict` instead of being dropped with the whole object,
    // and the two stay DISTINCT fields (the output cap is not the context size).
    let b = ollama(json!({
        "model": "m", "messages": [],
        "max_tokens": 4096,
        "options": {"num_ctx": 32768},
    }));
    assert_eq!(b["options"], json!({"num_predict": 4096, "num_ctx": 32768}));
}

#[test]
fn the_typed_leaf_wins_over_a_same_named_passthrough_leaf() {
    // Merge, not overwrite: `num_predict` is the typed field's, `num_ctx` the row's.
    // A size nobody stated is never fabricated — a bare row emits no `num_ctx` at all.
    let b = ollama(json!({
        "model": "m", "messages": [],
        "max_tokens": 4096,
        "options": {"num_predict": 1, "num_ctx": 8},
    }));
    assert_eq!(b["options"], json!({"num_predict": 4096, "num_ctx": 8}));
    let bare = ollama(json!({"model": "m", "messages": [], "max_tokens": 4096}));
    assert_eq!(bare["options"], json!({"num_predict": 4096}));
}

#[test]
fn a_key_the_encoder_never_wrote_is_inserted_whole() {
    // The vacant slot: no typed `options` (no gen scalar set) and an unmodeled
    // top-level key — both ride to the wire verbatim, the pre-existing behavior.
    let b = ollama(json!({
        "model": "m", "messages": [],
        "options": {"num_ctx": 32768},
        "keep_alive": "10m",
    }));
    assert_eq!(b["options"], json!({"num_ctx": 32768}));
    assert_eq!(b["keep_alive"], json!("10m"));
}

#[test]
fn only_two_objects_merge_a_type_mismatch_keeps_the_typed_value() {
    // Occupied but not both objects: the typed value wins whole, on either side of the
    // mismatch — a scalar passthrough against the typed `options` object, and an object
    // passthrough against the `think` bool the reasoning knob writes.
    let b = ollama(json!({
        "model": "m", "messages": [],
        "max_tokens": 4096,
        "reasoning": "low",
        "options": "nonsense",
        "think": {"nested": true},
    }));
    assert_eq!(b["options"], json!({"num_predict": 4096}));
    assert_eq!(b["think"], json!(true));
}

#[test]
fn the_merge_stops_one_level_down_where_a_namespace_becomes_a_value() {
    // A body map is a namespace of fields; one level down still is (`generationConfig`),
    // and that is where every dialect nests its generation params. Below THAT sits a
    // value the encoder owns whole, so the typed one wins entire rather than blending —
    // `thinkingConfig` here, and the JSON Schema under Anthropic's `output_config.format`
    // (pinned by `anthropic_encode::structured_output_projects_schema_natively_…`).
    let b = google(json!({
        "model": "m", "messages": [],
        "reasoning": "low",
        "generationConfig": {"topK": 40, "thinkingConfig": {"vendorKnob": 1}},
    }));
    assert_eq!(b["generationConfig"]["topK"], json!(40)); // merged in beside the typed keys
    assert_eq!(
        b["generationConfig"]["thinkingConfig"],
        json!({"thinkingBudget": 1024, "includeThoughts": true}) // typed value, whole
    );
}
