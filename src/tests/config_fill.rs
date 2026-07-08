//! `fill_absent` (config §4, §4.1): the request-absent fields a resolved config
//! supplies, and the ones a request keeps. The embedded `defaults.toml` validity
//! and the resolved gen defaults live in `config_defaults`.

use crate::{defaults, fill_absent, CanonicalRequest, Content, PartialConfig, ResolvedConfig};
use serde_json::json;

fn resolved(flags: PartialConfig, model: &str) -> ResolvedConfig {
    // The production composition (run/mod.rs): fold then route by request model.
    flags
        .or(PartialConfig::default())
        .or(defaults())
        .into_resolved(Some(model).filter(|m| !m.is_empty()))
        .unwrap()
}

fn select(provider: &str) -> PartialConfig {
    PartialConfig {
        provider: Some(provider.into()),
        ..Default::default()
    }
}

#[test]
fn fill_absent_fills_only_what_the_request_omits() {
    let cfg = resolved(
        PartialConfig {
            temperature: Some(0.3),
            top_p: Some(0.8),
            ..select("anthropic")
        },
        "claude-x",
    );
    let mut req = CanonicalRequest::default(); // empty model, all gen fields None
    fill_absent(&mut req, &cfg);
    assert_eq!(req.model, "claude-x"); // empty -> filled
    assert_eq!(req.max_tokens, Some(4096)); // row body_default folded into cfg.max_tokens
    assert_eq!(req.temperature, Some(0.3));
    assert_eq!(req.top_p, Some(0.8));
}

#[test]
fn fill_absent_propagates_stream_from_config_but_a_request_setting_wins() {
    // `--stream`/`BRAZEN_STREAM`/file resolve into `cfg.stream` and seed a request
    // that omits it — the regression: previously the flag never reached the wire,
    // so every live SSE 2xx decoded to `premature upstream EOF` (config §4).
    let cfg = resolved(
        PartialConfig {
            stream: Some(true),
            ..select("anthropic")
        },
        "m",
    );
    assert_eq!(cfg.stream, Some(true)); // carried by into_resolved

    let mut omitted = CanonicalRequest::default(); // stream: None
    fill_absent(&mut omitted, &cfg);
    assert_eq!(omitted.stream, Some(true)); // absent -> filled from config

    // A request that sets `stream` wins as-is, even against a streaming config — and
    // an explicit `false` is HONORED, never reverted to the global default (config §4.2).
    let mut present = CanonicalRequest {
        stream: Some(false),
        ..Default::default()
    };
    fill_absent(&mut present, &cfg);
    assert_eq!(present.stream, Some(false)); // present -> untouched

    // Neither request nor config set it: brazen's stream-native global default `true`
    // is the lowest operand of the fold (config §4.2), so the wire streams by default.
    let mut bare = CanonicalRequest::default();
    fill_absent(&mut bare, &resolved(select("anthropic"), "m"));
    assert_eq!(bare.stream, Some(true));
}

#[test]
fn fill_absent_supplies_the_config_system_prompt_when_the_request_omits_it() {
    let cfg = resolved(
        PartialConfig {
            system: Some(vec![Content::Text("be terse".into())]),
            ..select("anthropic")
        },
        "m",
    );
    assert_eq!(cfg.system, Some(vec![Content::Text("be terse".into())])); // carried by into_resolved
    let mut req = CanonicalRequest::default(); // no system
    fill_absent(&mut req, &cfg);
    assert_eq!(req.system, Some(vec![Content::Text("be terse".into())])); // absent -> filled
}

#[test]
fn fill_absent_leaves_a_request_system_prompt_untouched() {
    let cfg = resolved(
        PartialConfig {
            system: Some(vec![Content::Text("config".into())]),
            ..select("anthropic")
        },
        "m",
    );
    let mut req = CanonicalRequest {
        system: Some(vec![Content::Text("request".into())]),
        ..Default::default()
    };
    fill_absent(&mut req, &cfg);
    assert_eq!(req.system, Some(vec![Content::Text("request".into())])); // present -> untouched
}

#[test]
fn fill_absent_leaves_request_present_fields_untouched() {
    let cfg = resolved(
        PartialConfig {
            max_tokens: Some(50),
            temperature: Some(0.3),
            top_p: Some(0.8),
            ..select("anthropic")
        },
        "claude-x",
    );
    let mut req = CanonicalRequest {
        model: "mine".into(),
        max_tokens: Some(7),
        temperature: Some(0.9),
        top_p: Some(0.1),
        ..Default::default()
    };
    fill_absent(&mut req, &cfg);
    assert_eq!(req.model, "mine"); // present -> untouched
    assert_eq!(req.max_tokens, Some(7));
    assert_eq!(req.temperature, Some(0.9));
    assert_eq!(req.top_p, Some(0.1));
}

#[test]
fn fill_absent_seeds_config_passthrough_into_req_extra() {
    // A row's non-gen `body_defaults` becomes `cfg.extra`; `fill_absent` seeds it into
    // `req.extra` BENEATH the request's own keys (config §4.1) — the live encode seam.
    let file = crate::parse_config(
        "[[provider]]\nname = \"p\"\nbase_url = \"u\"\nprotocol = \"openai_chat\"\nauth = \"bearer\"\napi_header = { name = \"Authorization\", scheme = \"bearer\" }\nbody_defaults = { store = false, seed = 7 }\n",
    )
    .unwrap();
    let cfg = PartialConfig {
        provider: Some("p".into()),
        ..Default::default()
    }
    .or(file)
    .or(defaults())
    .into_resolved(Some("m"))
    .unwrap();
    assert_eq!(cfg.extra.get("store"), Some(&json!(false))); // non-gen passthrough resolved
                                                             // The request brings its own `store` (wins); `seed` is absent (config seeds it).
    let mut req: CanonicalRequest =
        serde_json::from_value(json!({"model":"m","messages":[],"store":true})).unwrap();
    fill_absent(&mut req, &cfg);
    assert_eq!(req.extra.get("store"), Some(&json!(true))); // request's own key wins
    assert_eq!(req.extra.get("seed"), Some(&json!(7))); // config fills the gap
}
