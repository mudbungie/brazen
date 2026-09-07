//! The SIXTH lifted knob end to end (providers.md §6.3): `cache_key`'s one
//! OpenAI-family spelling, the three dialects that narrow it away, the `extra`
//! precedence, and the ingress inverse — including the shapeless value that
//! deliberately stays on the valve because it has no canonical home. No network.

use crate::protocol::anthropic::AnthropicMessages;
use crate::protocol::google_genai::GoogleGenAi;
use crate::protocol::ollama_chat::OllamaChat;
use crate::protocol::openai::OpenAiChat;
use crate::protocol::openai_responses::OpenAiResponses;
use crate::{
    decode_request, defaults, parse_config, strip_unsupported, CanonicalRequest, IngressId,
    PartialConfig, Protocol, ProviderCtx,
};
use serde_json::{json, Value};

/// The wire body a dialect encodes for `req` (model/base_url are fixed and inert here).
fn body(proto: &dyn Protocol, req: &CanonicalRequest) -> Value {
    let ctx = ProviderCtx {
        base_url: "https://host",
        model: "m",
        beta_headers: &[],
        exec: None,
    };
    serde_json::from_slice(&proto.encode(req, &ctx).unwrap().body).unwrap()
}

fn with_key(key: Option<&str>) -> CanonicalRequest {
    CanonicalRequest {
        model: "m".into(),
        max_tokens: Some(64),
        cache_key: key.map(ToOwned::to_owned),
        ..Default::default()
    }
}

#[test]
fn the_openai_family_spells_the_branch_key_the_same_way_on_both_dialects() {
    // OpenAI's automatic prefix cache routes by `prompt_cache_key`; a growing,
    // stateless, fully-resent input without one lands on whichever replica takes it
    // and reads back zero cached tokens (bl-8b47).
    for proto in [&OpenAiChat as &dyn Protocol, &OpenAiResponses] {
        assert_eq!(
            body(proto, &with_key(Some("branch-7")))["prompt_cache_key"],
            "branch-7"
        );
        // `None` omits the key entirely: absent means unsent, and the wire is
        // byte-for-byte what it was before the field existed (the empty-set path).
        assert_eq!(body(proto, &with_key(None)).get("prompt_cache_key"), None);
    }
}

#[test]
fn the_typed_key_wins_over_the_escape_hatch_and_the_hatch_rides_through_alone() {
    // Written BEFORE the `extra` fold, so a row's `body_defaults.prompt_cache_key`
    // and the typed field never silently combine (§2.1.1).
    for proto in [&OpenAiChat as &dyn Protocol, &OpenAiResponses] {
        let mut req = with_key(Some("typed"));
        req.extra.insert("prompt_cache_key".into(), json!("raw"));
        assert_eq!(body(proto, &req)["prompt_cache_key"], "typed");

        let mut raw = with_key(None);
        raw.extra.insert("prompt_cache_key".into(), json!("raw"));
        assert_eq!(body(proto, &raw)["prompt_cache_key"], "raw");
    }
}

#[test]
fn the_dialects_that_place_or_lack_the_cache_narrow_it_away() {
    // Anthropic needs no equivalent — its encoder PLACES `cache_control` marks itself
    // from the request's shape (anthropic-messages.md §2.10) — and google_genai /
    // ollama_chat have no slot at all. All three drop it: a documented narrowing,
    // zero code, and the canonical request is untouched for every other protocol.
    for proto in [
        &AnthropicMessages as &dyn Protocol,
        &GoogleGenAi,
        &OllamaChat,
    ] {
        let wire = body(proto, &with_key(Some("branch-7")));
        assert!(
            !wire.to_string().contains("branch-7"),
            "{proto:p} leaked the branch key: {wire}"
        );
    }
}

#[test]
fn ingress_lifts_the_string_and_valves_what_has_no_canonical_home() {
    let base = json!({"model":"m","messages":[{"role":"user","content":"hi"}]});
    let with = |key: Value| {
        let mut v = base.clone();
        v["prompt_cache_key"] = key;
        decode_request(IngressId::OpenAiChat, v.to_string().as_bytes()).unwrap()
    };
    // Lifted: a masqueraded request re-routed to Anthropic must not carry an
    // OpenAI-only top-level key to a dialect that 400s on it.
    let req = with(json!("branch-7"));
    assert_eq!(req.cache_key.as_deref(), Some("branch-7"));
    assert_eq!(req.extra.get("prompt_cache_key"), None);
    // A shapeless value has no canonical home, so it rides the valve verbatim rather
    // than becoming a rung-4 rejection of a request the wire slot accepts (ingress §3).
    let req = with(json!(7));
    assert_eq!(req.cache_key, None);
    assert_eq!(req.extra.get("prompt_cache_key"), Some(&json!(7)));
    // Absent stays absent.
    let req = decode_request(IngressId::OpenAiChat, base.to_string().as_bytes()).unwrap();
    assert_eq!(req.cache_key, None);
}

#[test]
fn a_row_that_rejects_the_key_declines_it_as_data() {
    // The opt-out is row DATA, not code: a backend that 400s on `prompt_cache_key`
    // names the CANONICAL field in `unsupported_body_keys` and the strip clears the
    // typed field whatever its source (config §4.1.1). Without that arm the key would
    // fall through to `req.extra.remove`, which cannot reach a typed field — the row
    // would name a strip that never happened.
    let row = |keys: &str| {
        let file = parse_config(&format!(
            "[[provider]]\nname = \"row\"\nbase_url = \"u\"\nprotocol = \"openai_chat\"\nauth = \"bearer\"\napi_header = {{ name = \"Authorization\", scheme = \"bearer\" }}\nunsupported_body_keys = [{keys}]\n",
        ))
        .unwrap();
        PartialConfig {
            provider: Some("row".into()),
            ..Default::default()
        }
        .or(file)
        .or(defaults())
        .into_resolved(Some("m"), None)
        .unwrap()
    };
    let mut kept = with_key(Some("branch-7"));
    strip_unsupported(&mut kept, &row(""));
    assert_eq!(kept.cache_key.as_deref(), Some("branch-7"));

    let mut dropped = with_key(Some("branch-7"));
    strip_unsupported(&mut dropped, &row("\"cache_key\""));
    assert_eq!(dropped.cache_key, None);
}
