//! The provider-reported model METADATA projection (model-discovery §3): context window
//! (input token limit), max output tokens (output limit), and display name — lifted
//! through each protocol's `models_shape` keys, Option-shaped and CARRIED, never
//! fabricated (the Usage zero-vs-unknown principle). Google is the richest source (all
//! three), Anthropic serves only `display_name`, OpenAI/Ollama none (the empty-set rule);
//! a row's `[provider.models]` may NAME a metadata key to lift a fact its list serves
//! under a non-default key (the Codex `context_window`). Shares the `model` / `decode` /
//! `bare_keys` helpers with the sibling `model_discovery_decode`. Offline, pure.

use crate::protocol::{
    anthropic::AnthropicMessages, google_genai::GoogleGenAi, openai::OpenAiChat,
};
use crate::protocol::{decode_models, ModelKeys};
use crate::tests::model_discovery_decode::{bare_keys, decode, model};
use crate::Model;

#[test]
fn a_row_named_metadata_key_lifts_the_codex_context_window() {
    // The SAME openai_responses protocol, decoded with the row's `[provider.models]`
    // override keys (array_key="models", id_key="slug", §3.1/§3.2): the Codex
    // `{"models":[{"slug":…}]}` body → the ordered slugs. The override keys are ROW data,
    // not a protocol constant — here we prove the generic decoder reads them, AND that a
    // row-NAMED metadata key (`context_key = "context_window"`, §3.2) lifts the Codex
    // list's own `context_window` into `Model.context_window` (Some), while the slug entry
    // that omits it stays `None` — carried, never fabricated (§3).
    let keys = ModelKeys {
        context_key: "context_window",
        ..bare_keys("models", "slug", "")
    };
    let body = br#"{"models":[
        {"slug":"gpt-5.6-sol","context_window":400000},
        {"slug":"gpt-5.4","context_window":272000},
        {"slug":"codex-auto-review"}
    ]}"#;
    let got = decode_models(body, &keys).unwrap();
    assert_eq!(
        got.iter().map(|m| m.id.clone()).collect::<Vec<_>>(),
        ["gpt-5.6-sol", "gpt-5.4", "codex-auto-review"]
    );
    assert_eq!(
        got.iter().map(|m| m.context_window).collect::<Vec<_>>(),
        [Some(400000), Some(272000), None]
    );
    // A valid EMPTY Codex list decodes to an empty Vec (the verb returns 0, §2).
    assert!(decode_models(br#"{"models":[]}"#, &keys)
        .unwrap()
        .is_empty());
}

#[test]
fn metadata_is_projected_per_dialect_carried_never_fabricated() {
    // Google is the richest source: `inputTokenLimit`/`outputTokenLimit`/`displayName`
    // all land through its own `models_shape` keys (the SAME path `fetch_models` uses); a
    // Google entry missing a limit leaves that field `None` (carried, not fabricated).
    let g = br#"{"models":[
        {"name":"models/gemini-2.5-pro","displayName":"Gemini 2.5 Pro",
         "inputTokenLimit":1048576,"outputTokenLimit":65536},
        {"name":"models/gemini-embed"}
    ]}"#;
    assert_eq!(
        decode(&GoogleGenAi, g).unwrap(),
        [
            Model {
                id: "gemini-2.5-pro".into(),
                default: false,
                context_window: Some(1048576),
                max_output_tokens: Some(65536),
                display_name: Some("Gemini 2.5 Pro".into()),
            },
            model("gemini-embed"), // no metadata served ⇒ every field None
        ]
    );
    // Anthropic serves `display_name` but NO token limits — the label is carried, the
    // limits stay `None` (§3, empty-set rule; a Model NOT equal to the bare `model()`).
    let a =
        br#"{"data":[{"type":"model","id":"claude-opus-4-1","display_name":"Claude Opus 4.1"}]}"#;
    assert_eq!(
        decode(&AnthropicMessages, a).unwrap(),
        [Model {
            id: "claude-opus-4-1".into(),
            default: false,
            context_window: None,
            max_output_tokens: None,
            display_name: Some("Claude Opus 4.1".into()),
        }]
    );
    // OpenAI serves neither — even a body that happens to carry a `display_name` field is
    // ignored, because the dialect's `display_name_key` is `""` (not that key).
    let o = br#"{"data":[{"id":"gpt-5","display_name":"ignored"}]}"#;
    assert_eq!(decode(&OpenAiChat, o).unwrap(), [model("gpt-5")]);
}
