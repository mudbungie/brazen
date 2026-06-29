//! `Protocol::models_shape` (the per-dialect discovery DEFAULTS as DATA) + the ONE
//! generic `decode_models` fed those defaults (model-discovery §3, §3.1): a sample
//! list body → the expected ORDERED `Vec<Model>`, the shape each protocol declares,
//! Google's `models/`-prefix strip, the Codex `models`/`slug` override shape, and a
//! malformed/unexpected body → a `Provider` error. Offline fixtures, pure like
//! `decode` — no network. All five impls are exercised so the close gate's 100% line
//! coverage backs the exhaustiveness the trait demands (no dead default).

use crate::protocol::anthropic::AnthropicMessages;
use crate::protocol::google_genai::GoogleGenAi;
use crate::protocol::ollama_chat::OllamaChat;
use crate::protocol::openai::OpenAiChat;
use crate::protocol::openai_responses::OpenAiResponses;
use crate::protocol::{decode_models, ModelsShape};
use crate::{CanonicalError, ErrorKind, Model, Protocol};

/// A `Model` with `default: false` (no dialect flags one today, §3).
fn model(id: &str) -> Model {
    Model {
        id: id.into(),
        default: false,
    }
}

/// Decode a list body through the ONE generic decoder fed the protocol's
/// `models_shape` (model-discovery §3) — the SAME path `fetch_models` uses, never
/// forked, so these fixtures cover the production decode.
fn decode(p: &dyn Protocol, body: &[u8]) -> Result<Vec<Model>, CanonicalError> {
    let s = p.models_shape();
    decode_models(body, s.array_key, s.id_key, s.strip)
}

/// The ids of a decoded list, in order — the assertion the order-preserving contract
/// turns on.
fn ids(p: &dyn Protocol, body: &[u8]) -> Vec<String> {
    let models = decode(p, body).unwrap();
    assert!(models.iter().all(|m| !m.default)); // §3.1: none flag a default today
    models.into_iter().map(|m| m.id).collect()
}

#[test]
fn models_shape_is_the_per_dialect_defaults() {
    // The one home (§3.1): each protocol's DEFAULT discovery shape — the GET path
    // appended to base_url, the list `array_key`/`id_key`, and the leading `strip`.
    assert_eq!(
        OpenAiChat.models_shape(),
        ModelsShape {
            path: "/models",
            array_key: "data",
            id_key: "id",
            strip: ""
        }
    );
    assert_eq!(
        OpenAiResponses.models_shape(),
        ModelsShape {
            path: "/models",
            array_key: "data",
            id_key: "id",
            strip: ""
        }
    );
    assert_eq!(
        AnthropicMessages.models_shape(),
        ModelsShape {
            path: "/v1/models",
            array_key: "data",
            id_key: "id",
            strip: ""
        }
    );
    assert_eq!(
        GoogleGenAi.models_shape(),
        ModelsShape {
            path: "/v1beta/models",
            array_key: "models",
            id_key: "name",
            strip: "models/"
        }
    );
    assert_eq!(
        OllamaChat.models_shape(),
        ModelsShape {
            path: "/api/tags",
            array_key: "models",
            id_key: "name",
            strip: ""
        }
    );
}

#[test]
fn openai_chat_decodes_data_ids_in_wire_order() {
    // `data[].id`, as-is, creation order preserved (§3.1). Non-id fields ignored.
    let body = br#"{"object":"list","data":[
        {"id":"gpt-4o","object":"model","created":1},
        {"id":"gpt-4o-mini","object":"model","created":2},
        {"id":"o1","object":"model","created":3}
    ]}"#;
    assert_eq!(ids(&OpenAiChat, body), ["gpt-4o", "gpt-4o-mini", "o1"]);
    assert_eq!(
        decode(&OpenAiChat, body).unwrap(),
        [model("gpt-4o"), model("gpt-4o-mini"), model("o1")]
    );
}

#[test]
fn openai_responses_decodes_data_ids_like_chat() {
    // Same `data[].id` DEFAULT shape as openai_chat (§3.1) — shared decoder; the Codex
    // override shape is exercised in `the_generic_decoder_reads_codex_models_slug_shape`.
    let body = br#"{"data":[{"id":"gpt-5"},{"id":"o3"}]}"#;
    assert_eq!(ids(&OpenAiResponses, body), ["gpt-5", "o3"]);
}

#[test]
fn the_generic_decoder_reads_codex_models_slug_shape() {
    // The SAME openai_responses protocol, decoded with the row's `[provider.models]`
    // override keys (array_key="models", id_key="slug", §3.1/§3.2): the Codex
    // `{"models":[{"slug":…}]}` body → the ordered slugs. The override keys are ROW
    // data, not a protocol constant — here we prove the generic decoder reads them.
    let body = br#"{"models":[
        {"slug":"gpt-5.6-sol","context_window":1},
        {"slug":"gpt-5.4","context_window":2},
        {"slug":"codex-auto-review"}
    ]}"#;
    assert_eq!(
        decode_models(body, "models", "slug", "")
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect::<Vec<_>>(),
        ["gpt-5.6-sol", "gpt-5.4", "codex-auto-review"]
    );
    // A valid EMPTY Codex list decodes to an empty Vec (the verb returns 0, §2).
    assert!(decode_models(br#"{"models":[]}"#, "models", "slug", "")
        .unwrap()
        .is_empty());
}

#[test]
fn anthropic_decodes_data_ids_newest_first() {
    // `data[].id`, newest-first order preserved verbatim (§3.1).
    let body = br#"{"data":[
        {"type":"model","id":"claude-opus-4-1-20250805"},
        {"type":"model","id":"claude-sonnet-4-5-20250929"},
        {"type":"model","id":"claude-3-5-haiku-20241022"}
    ],"has_more":false}"#;
    assert_eq!(
        ids(&AnthropicMessages, body),
        [
            "claude-opus-4-1-20250805",
            "claude-sonnet-4-5-20250929",
            "claude-3-5-haiku-20241022"
        ]
    );
}

#[test]
fn google_decodes_model_names_stripping_the_models_prefix() {
    // `models[].name`, STRIP leading `models/` so the id is usable in encode's
    // `/v1beta/models/{model}:…` path (§3.1). Order preserved.
    let body = br#"{"models":[
        {"name":"models/gemini-2.5-pro","displayName":"Gemini 2.5 Pro"},
        {"name":"models/gemini-2.5-flash"}
    ]}"#;
    assert_eq!(
        ids(&GoogleGenAi, body),
        ["gemini-2.5-pro", "gemini-2.5-flash"]
    );
}

#[test]
fn google_strips_only_a_leading_models_prefix() {
    // The strip is a leading-prefix op, not a substring delete: a name without the
    // prefix passes through, and an interior `models/` is untouched.
    let body = br#"{"models":[{"name":"gemini-bare"},{"name":"models/a/models/b"}]}"#;
    assert_eq!(ids(&GoogleGenAi, body), ["gemini-bare", "a/models/b"]);
}

#[test]
fn ollama_decodes_model_names_as_is() {
    // `models[].name`, as-is — local tags keep their `:tag` (§3.1). Order preserved.
    let body = br#"{"models":[
        {"name":"llama3:latest","size":1},
        {"name":"qwen2.5-coder:7b","size":2}
    ]}"#;
    assert_eq!(
        ids(&OllamaChat, body),
        ["llama3:latest", "qwen2.5-coder:7b"]
    );
}

#[test]
fn an_empty_list_decodes_to_an_empty_vec() {
    // A well-formed body with no entries is an empty (not error) list — the verb LISTS
    // (exit 0, §2); only the generation `select_model` turns emptiness into Config (78).
    assert!(decode(&OpenAiChat, br#"{"data":[]}"#).unwrap().is_empty());
    assert!(decode(&GoogleGenAi, br#"{"models":[]}"#)
        .unwrap()
        .is_empty());
}

#[test]
fn entries_missing_the_id_field_are_skipped() {
    // A non-string / absent id field projects to nothing rather than a panic or an
    // empty id (the wire never crashes us) — order of the survivors is preserved.
    let body = br#"{"data":[{"id":"keep-1"},{"object":"model"},{"id":42},{"id":"keep-2"}]}"#;
    assert_eq!(ids(&OpenAiChat, body), ["keep-1", "keep-2"]);
}

#[test]
fn malformed_body_is_a_provider_error_for_every_protocol() {
    // §3.1: a body we cannot project (not JSON, or lacking the dialect's list array)
    // is a `Provider` error — the GET drained a 2xx, so an unparseable list is an
    // upstream contract violation, never a silent empty list. Verified per protocol.
    let cases: [(&dyn Protocol, &[u8]); 5] = [
        (&OpenAiChat, b"{not json"),                 // unparseable bytes
        (&OpenAiResponses, br#"{"data":"oops"}"#),   // `data` not an array
        (&AnthropicMessages, br#"{"models":[]}"#),   // right array key for a DIFFERENT dialect
        (&GoogleGenAi, br#"{"data":[{"id":"x"}]}"#), // openai shape, no `models` array
        (&OllamaChat, b"[]"),                        // top-level not an object
    ];
    for (proto, body) in cases {
        let err = decode(proto, body).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Provider { status: 502 });
        assert_eq!(err.exit_code(), 70); // 5xx → 70
        assert!(err.retryable()); // a 5xx provider fault is retryable
        assert!(
            err.message.contains("malformed models list"),
            "diagnostic names the failure: {}",
            err.message
        );
    }
}
