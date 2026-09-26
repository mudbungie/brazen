//! The SEVENTH lifted knob end to end (providers.md §6.3, bl-842b): `--image` — "the
//! reply may include an image". The flag/env/file rungs that fill it, the dump, the
//! row opt-out, the two dialect spellings (Google's output modality, which the Cloud
//! Code envelope inherits; OpenAI Responses' native tool, never duplicated), and the
//! four LOUD rejections on the dialects that return no images. No network.

use std::collections::BTreeMap;

use crate::protocol::anthropic::AnthropicMessages;
use crate::protocol::claude_code::ClaudeCode;
use crate::protocol::google_cloudcode::GoogleCloudCode;
use crate::protocol::google_genai::GoogleGenAi;
use crate::protocol::ollama_chat::OllamaChat;
use crate::protocol::openai::OpenAiChat;
use crate::protocol::openai_responses::OpenAiResponses;
use crate::{
    defaults, dump_config, fill_absent, parse_args, parse_config, partial_from_env,
    strip_unsupported, CanonicalRequest, EnvSnapshot, ErrorKind, PartialConfig, Protocol,
    ProviderCtx, ResolvedConfig,
};
use serde_json::{json, Value};

const CTX: ProviderCtx = ProviderCtx {
    base_url: "https://host",
    model: "m",
    beta_headers: &[],
    exec: Some("claude"),
};

fn req(extra: Value) -> CanonicalRequest {
    let mut v =
        json!({"model": "m", "messages": [{"role": "user", "content": "hi"}], "max_tokens": 64});
    v.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    serde_json::from_value(v).unwrap()
}

fn body(proto: &dyn Protocol, r: &CanonicalRequest) -> Value {
    serde_json::from_slice(&proto.encode(r, &CTX).unwrap().body).unwrap()
}

#[test]
fn every_rung_fills_the_knob() {
    // flag: a bare switch, like `--stream`.
    let flags = parse_args(&["--image".to_string()]).unwrap();
    assert_eq!(flags.config.image, Some(true));
    // env: BRAZEN_IMAGE parses a bool; anything else is a BadValue naming the var.
    let snap = |v: &str| {
        EnvSnapshot(BTreeMap::from([(
            "BRAZEN_IMAGE".to_string(),
            v.to_string(),
        )]))
    };
    assert_eq!(partial_from_env(&snap("true")).unwrap().image, Some(true));
    assert_eq!(partial_from_env(&snap("false")).unwrap().image, Some(false));
    let bad = partial_from_env(&snap("yes please")).unwrap_err();
    assert!(format!("{bad}").contains("BRAZEN_IMAGE"), "{bad}");
    // file: `image = true`, and the fold is the ordinary `Option::or` (flag outranks file).
    let file = parse_config("image = true\n").unwrap();
    assert_eq!(file.image, Some(true));
    let folded = PartialConfig {
        image: Some(false),
        ..Default::default()
    }
    .or(file);
    assert_eq!(folded.image, Some(false));
}

#[test]
fn the_knob_round_trips_through_dump_config() {
    let flags = PartialConfig {
        image: Some(true),
        ..Default::default()
    };
    let out = dump_config(
        flags,
        &EnvSnapshot(BTreeMap::new()),
        PartialConfig::default(),
    )
    .unwrap();
    assert!(out.contains("image = true"), "{out}");
    assert_eq!(parse_config(&out).unwrap().image, Some(true));
}

/// The knob resolved through the production fold, for a row opting out via `keys`.
fn resolved(image: Option<bool>, keys: &str) -> ResolvedConfig {
    let file = parse_config(&format!(
        "[[provider]]\nname = \"row\"\nbase_url = \"u\"\nprotocol = \"openai_responses\"\nauth = \"bearer\"\napi_header = {{ name = \"Authorization\", scheme = \"bearer\" }}\nunsupported_body_keys = [{keys}]\n",
    ))
    .unwrap();
    PartialConfig {
        provider: Some("row".into()),
        image,
        ..Default::default()
    }
    .or(file)
    .or(defaults())
    .into_resolved(Some("m"), None)
    .unwrap()
}

#[test]
fn fill_absent_supplies_the_knob_the_request_wins_and_a_row_can_decline_it() {
    let cfg = resolved(Some(true), "");
    let mut bare = CanonicalRequest::default();
    fill_absent(&mut bare, &cfg);
    assert_eq!(bare.image, Some(true));
    let mut own = req(json!({"image": false}));
    fill_absent(&mut own, &cfg);
    assert_eq!(own.image, Some(false));
    // The opt-out is row DATA: `unsupported_body_keys = ["image"]` clears it.
    let declining = resolved(Some(true), "\"image\"");
    let mut r = CanonicalRequest::default();
    fill_absent(&mut r, &declining);
    strip_unsupported(&mut r, &declining);
    assert_eq!(r.image, None);
}

#[test]
fn google_asks_for_the_image_modality_and_the_cloud_code_envelope_inherits_it() {
    let on = req(json!({"image": true}));
    let modalities = json!(["TEXT", "IMAGE"]);
    assert_eq!(
        body(&GoogleGenAi, &on)["generationConfig"]["responseModalities"],
        modalities
    );
    // The Cloud Code envelope has no code of its own for it: the shared body_map.
    assert_eq!(
        body(&GoogleCloudCode, &on)["request"]["generationConfig"]["responseModalities"],
        modalities
    );
    // The typed knob wins over a raw modality list on the extra valve (one-level merge).
    let raw = req(
        json!({"image": true, "generationConfig": {"responseModalities": ["TEXT"], "seed": 7}}),
    );
    let g = &body(&GoogleGenAi, &raw)["generationConfig"];
    assert_eq!(
        (g["responseModalities"].clone(), g["seed"].clone()),
        (modalities, json!(7))
    );
    // Absent and false are the one absent fact: nothing written.
    for off in [req(json!({})), req(json!({"image": false}))] {
        assert!(body(&GoogleGenAi, &off)
            .get("generationConfig")
            .is_none_or(|g| g.get("responseModalities").is_none()));
    }
}

#[test]
fn responses_appends_the_native_tool_once_and_never_duplicates_a_declared_one() {
    // No other tools: the key still appears, carrying only the image tool.
    assert_eq!(
        body(&OpenAiResponses, &req(json!({"image": true})))["tools"],
        json!([{"type": "image_generation"}])
    );
    // Beside a custom tool: appended after it.
    let with_fn =
        req(json!({"image": true, "tools": [{"name": "f", "input_schema": {"type": "object"}}]}));
    assert_eq!(
        body(&OpenAiResponses, &with_fn)["tools"],
        json!([{"type": "function", "name": "f", "parameters": {"type": "object"}}, {"type": "image_generation"}])
    );
    // A caller-configured image tool stands alone — the knob adds nothing.
    let declared =
        req(json!({"image": true, "tools": [{"type": "image_generation", "size": "1024x1024"}]}));
    assert_eq!(
        body(&OpenAiResponses, &declared)["tools"],
        json!([{"type": "image_generation", "size": "1024x1024"}])
    );
    // Off: no tools key at all.
    assert!(body(&OpenAiResponses, &req(json!({"image": false})))
        .get("tools")
        .is_none());
}

#[test]
fn the_dialects_that_return_no_images_reject_loudly() {
    let on = req(json!({"image": true}));
    let dialects: [(&dyn Protocol, &str); 4] = [
        (&AnthropicMessages, "anthropic_messages"),
        (&OpenAiChat, "openai chat"),
        (&OllamaChat, "ollama_chat"),
        (&ClaudeCode, "claude_code"),
    ];
    for (p, name) in dialects {
        let e = p.encode(&on, &CTX).unwrap_err();
        assert_eq!(
            (e.kind.clone(), e.exit_code()),
            (ErrorKind::ParseInput, 64),
            "{name}"
        );
        assert!(
            e.message.starts_with(name) && e.message.contains("--image"),
            "{}",
            e.message
        );
        // …and `false` is simply absent there too.
        assert!(
            p.encode(&req(json!({"image": false})), &CTX).is_ok(),
            "{name}"
        );
    }
}
