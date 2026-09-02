//! The FIFTH lifted knob end to end (providers.md §6.2): `ServiceTier`'s two
//! per-family spelling tables, the flag/env/file rungs that fill it, the row
//! opt-out, the three dialect projections and the two documented narrowings, and
//! the ingress inverse — including the values that deliberately stay on the
//! `extra` valve because they have no canonical home. No network.

use std::collections::BTreeMap;

use crate::protocol::anthropic::AnthropicMessages;
use crate::protocol::google_genai::GoogleGenAi;
use crate::protocol::ollama_chat::OllamaChat;
use crate::protocol::openai::OpenAiChat;
use crate::protocol::openai_responses::OpenAiResponses;
use crate::{
    decode_request, defaults, dump_config, fill_absent, parse_args, parse_config, partial_from_env,
    strip_unsupported, CanonicalRequest, EnvSnapshot, IngressId, PartialConfig, Protocol,
    ProviderCtx, ResolvedConfig, ServiceTier,
};
use serde_json::{json, Value};

fn env(pairs: &[(&str, &str)]) -> EnvSnapshot {
    EnvSnapshot(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    )
}

fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| (*a).to_string()).collect()
}

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

fn with_tier(tier: &str) -> CanonicalRequest {
    serde_json::from_value(json!({
        "model": "m", "messages": [], "max_tokens": 64, "service_tier": tier,
    }))
    .unwrap()
}

#[test]
fn the_lane_enum_owns_one_table_per_provider_family() {
    // The canonical spelling (wire + config) is the intent's own word; each dialect
    // family reads its own table off the enum, so "what is `standard` on Anthropic?"
    // has exactly one answer (providers §6.2).
    for (tier, canonical, openai, anthropic) in [
        (ServiceTier::Priority, "priority", "priority", "auto"),
        (
            ServiceTier::Standard,
            "standard",
            "default",
            "standard_only",
        ),
    ] {
        assert_eq!(tier.openai(), openai);
        assert_eq!(tier.anthropic(), anthropic);
        assert_eq!(canonical.parse::<ServiceTier>(), Ok(tier));
        assert_eq!(
            serde_json::to_string(&tier).unwrap(),
            format!("\"{canonical}\"")
        );
        assert_eq!(
            serde_json::from_str::<ServiceTier>(&format!("\"{canonical}\"")).unwrap(),
            tier
        );
    }
    // An unrecognized lane fails FromStr (lifted to a usage/BadValue by the callers).
    assert_eq!("express".parse::<ServiceTier>(), Err(()));
    assert!(!format!("{:?}", ServiceTier::Priority).is_empty());
}

#[test]
fn every_rung_fills_the_knob_and_the_flag_and_env_reject_an_unknown_lane() {
    // flag: `--tier` in both spellings, and a usage error (64) for anything else.
    for form in [vec!["--tier", "priority"], vec!["--tier=priority"]] {
        let flags = parse_args(&argv(&form)).unwrap();
        assert_eq!(flags.config.service_tier, Some(ServiceTier::Priority));
    }
    let err = parse_args(&argv(&["--tier", "express"])).unwrap_err();
    assert!(err.message.contains("priority|standard"), "{err:?}");
    // env: BRAZEN_TIER, the operator's word for the lane.
    let cfg = partial_from_env(&env(&[("BRAZEN_TIER", "standard")])).unwrap();
    assert_eq!(cfg.service_tier, Some(ServiceTier::Standard));
    let bad = partial_from_env(&env(&[("BRAZEN_TIER", "express")])).unwrap_err();
    assert!(format!("{bad}").contains("BRAZEN_TIER"), "{bad}");
    // file: the WIRE spelling is the file key (the crate speaks the wire).
    let file = parse_config("service_tier = \"priority\"\n").unwrap();
    assert_eq!(file.service_tier, Some(ServiceTier::Priority));
    // and the fold is the ordinary `Option::or`: the flag layer outranks the file.
    let folded = PartialConfig {
        service_tier: Some(ServiceTier::Standard),
        ..Default::default()
    }
    .or(file);
    assert_eq!(folded.service_tier, Some(ServiceTier::Standard));
}

#[test]
fn the_knob_round_trips_through_dump_config() {
    // `--dump-config` must show every resolved scalar or it lies about the run
    // (config §6); the dumped key is the file key, so the dump re-parses.
    let flags = PartialConfig {
        service_tier: Some(ServiceTier::Priority),
        ..Default::default()
    };
    let out = dump_config(
        flags,
        &EnvSnapshot(BTreeMap::new()),
        PartialConfig::default(),
    )
    .unwrap();
    assert!(out.contains("service_tier = \"priority\""), "{out}");
    assert_eq!(
        parse_config(&out).unwrap().service_tier,
        Some(ServiceTier::Priority)
    );
}

/// The knob resolved through the production fold, for a row named by `keys` opting out.
fn resolved(tier: Option<ServiceTier>, keys: &str) -> ResolvedConfig {
    let file = parse_config(&format!(
        "[[provider]]\nname = \"row\"\nbase_url = \"u\"\nprotocol = \"openai_chat\"\nauth = \"bearer\"\napi_header = {{ name = \"Authorization\", scheme = \"bearer\" }}\nunsupported_body_keys = [{keys}]\n",
    ))
    .unwrap();
    PartialConfig {
        provider: Some("row".into()),
        service_tier: tier,
        ..Default::default()
    }
    .or(file)
    .or(defaults())
    .into_resolved(Some("m"), None)
    .unwrap()
}

#[test]
fn fill_absent_supplies_the_lane_and_a_row_can_decline_it() {
    // request > config, by composition: a request that names its own lane keeps it.
    let cfg = resolved(Some(ServiceTier::Standard), "");
    assert_eq!(cfg.service_tier, Some(ServiceTier::Standard));
    let mut req = with_tier("priority");
    fill_absent(&mut req, &cfg);
    strip_unsupported(&mut req, &cfg);
    assert_eq!(req.service_tier, Some(ServiceTier::Priority));
    // …and a request that omits it inherits the resolved lane.
    let mut bare = CanonicalRequest::default();
    fill_absent(&mut bare, &cfg);
    assert_eq!(bare.service_tier, Some(ServiceTier::Standard));
    // The opt-out is row DATA, not code: a backend that 400s on the key names the
    // CANONICAL field in `unsupported_body_keys` and the strip clears it whatever
    // its source (config §4.1.1) — without that arm a typed knob is un-declinable.
    let declining = resolved(Some(ServiceTier::Standard), "\"service_tier\"");
    let mut req = with_tier("priority");
    fill_absent(&mut req, &declining);
    strip_unsupported(&mut req, &declining);
    assert_eq!(req.service_tier, None);
}

#[test]
fn the_openai_family_spells_the_lane_the_same_way_on_both_dialects() {
    for proto in [&OpenAiChat as &dyn Protocol, &OpenAiResponses] {
        assert_eq!(
            body(proto, &with_tier("priority"))["service_tier"],
            "priority"
        );
        assert_eq!(
            body(proto, &with_tier("standard"))["service_tier"],
            "default"
        );
        // `None` omits the key entirely — the provider's own default lane (empty set).
        let bare: CanonicalRequest =
            serde_json::from_value(json!({"model":"m","messages":[],"max_tokens":64})).unwrap();
        assert_eq!(body(proto, &bare).get("service_tier"), None);
    }
}

#[test]
fn anthropic_projects_the_priority_intent_onto_auto_and_the_typed_knob_wins() {
    // THE ASYMMETRY (providers §6.2): Anthropic has no request-side priority DEMAND —
    // priority is org provisioning — so the priority intent is `"auto"` (spend it if
    // provisioned, else standard), and `standard` is the explicit `"standard_only"`.
    assert_eq!(
        body(&AnthropicMessages, &with_tier("priority"))["service_tier"],
        "auto"
    );
    assert_eq!(
        body(&AnthropicMessages, &with_tier("standard"))["service_tier"],
        "standard_only"
    );
    // The escape hatch stays the row's `body_defaults` (riding `extra`), and the typed
    // knob is written BEFORE the extra fold, so the two never silently combine.
    let mut req = with_tier("standard");
    req.extra.insert("service_tier".into(), json!("auto"));
    assert_eq!(
        body(&AnthropicMessages, &req)["service_tier"],
        "standard_only"
    );
    // …and with no typed knob the raw escape-hatch value rides through verbatim.
    let mut raw = CanonicalRequest {
        max_tokens: Some(64),
        ..Default::default()
    };
    raw.extra.insert("service_tier".into(), json!("auto"));
    assert_eq!(body(&AnthropicMessages, &raw)["service_tier"], "auto");
}

#[test]
fn the_dialects_with_no_lane_spelling_narrow_it_away() {
    // google_genai / ollama_chat have no `service_tier` wire slot at all, so the knob
    // is a DOCUMENTED narrowing (dropped), exactly like `output`'s narrowings — zero
    // code, and the canonical request is untouched for every other protocol.
    for proto in [&GoogleGenAi as &dyn Protocol, &OllamaChat] {
        let wire = body(proto, &with_tier("priority"));
        assert!(
            !wire.to_string().contains("service_tier"),
            "{proto:p} leaked the lane: {wire}"
        );
    }
}

#[test]
fn ingress_lifts_the_values_with_a_canonical_home_and_valves_the_rest() {
    let dec = |id, v: Value| decode_request(id, v.to_string().as_bytes()).unwrap();
    let base = json!({"model":"m","messages":[{"role":"user","content":"hi"}]});
    let with = |id, tier: Value| {
        let mut v = base.clone();
        v["service_tier"] = tier;
        dec(id, v)
    };
    // openai_chat: the two values the canonical model can hold.
    let req = with(IngressId::OpenAiChat, json!("priority"));
    assert_eq!(req.service_tier, Some(ServiceTier::Priority));
    assert_eq!(req.extra.get("service_tier"), None);
    assert_eq!(
        with(IngressId::OpenAiChat, json!("default")).service_tier,
        Some(ServiceTier::Standard)
    );
    // Every other lane (`auto`/`flex`/`scale`, or a shapeless value) has NO canonical
    // home, so it rides the valve verbatim exactly as it did before the lift: rung 1
    // plus the long-tail valve, never a rung-4 rejection of a request brazen served
    // yesterday — the wire slot exists, and lane ENTITLEMENT is the provider's court
    // (ingress §3, "carry the spec, not the water").
    for lane in [json!("flex"), json!("auto"), json!(7)] {
        let req = with(IngressId::OpenAiChat, lane.clone());
        assert_eq!(req.service_tier, None);
        assert_eq!(req.extra.get("service_tier"), Some(&lane));
    }
    // anthropic_messages: only `standard_only` is an unambiguous lane DEMAND. `auto`
    // is Anthropic's wire DEFAULT and the value `Priority` projects to — the
    // projection is not injective, so lifting it would silently upgrade an ordinary
    // request into a paid lane once re-routed to an OpenAI-family row.
    let req = with(IngressId::AnthropicMessages, json!("standard_only"));
    assert_eq!(req.service_tier, Some(ServiceTier::Standard));
    assert_eq!(req.extra.get("service_tier"), None);
    let req = with(IngressId::AnthropicMessages, json!("auto"));
    assert_eq!(req.service_tier, None);
    assert_eq!(req.extra.get("service_tier"), Some(&json!("auto")));
}
