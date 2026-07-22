//! The one schema, four instances (config §2): `Option` fields, the array-of-
//! tables ⇄ keyed-map deserialize seam, and the associative `or` fold.

use crate::{AuthId, Content, OutMode, PartialConfig, ProtocolId};
use serde_json::json;

fn parse(s: &str) -> PartialConfig {
    crate::parse_config(s).unwrap()
}

#[test]
fn out_mode_parses_known_spellings_and_rejects_others() {
    assert_eq!(OutMode::parse("text"), Some(OutMode::Text));
    assert_eq!(OutMode::parse("ndjson"), Some(OutMode::Ndjson));
    assert_eq!(OutMode::parse("raw"), Some(OutMode::Raw));
    assert_eq!(OutMode::parse("xml"), None);
    // Copy + Eq + Debug.
    let m = OutMode::Text;
    assert_eq!(m, m);
    assert!(!format!("{m:?}").is_empty());
}

#[test]
fn deserializes_scalar_fields() {
    let cfg = parse(
        "provider = \"anthropic\"\nmodel = \"sonnet\"\napi_key = \"sk\"\noutput = \"ndjson\"\nthinking = true\nmax_tokens = 1000\ntemperature = 0.5\ntop_p = 0.9\nstream = true\ntimeout = 90\n",
    );
    assert_eq!(cfg.provider.as_deref(), Some("anthropic"));
    assert_eq!(cfg.model.as_deref(), Some("sonnet"));
    assert!(cfg.api_key.is_some());
    assert_eq!(cfg.output, Some(OutMode::Ndjson));
    assert_eq!(cfg.thinking, Some(true));
    assert_eq!(cfg.max_tokens, Some(1000));
    assert_eq!(cfg.temperature, Some(0.5));
    assert_eq!(cfg.top_p, Some(0.9));
    assert_eq!(cfg.stream, Some(true));
    assert_eq!(cfg.timeout, Some(90));
    assert!(cfg.providers.is_empty());
    // Clone + Debug + PartialEq.
    assert_eq!(cfg.clone(), cfg);
    assert!(!format!("{cfg:?}").is_empty());
}

#[test]
fn deserializes_provider_rows_into_the_keyed_map() {
    let cfg = parse(
        "[[provider]]\nname = \"anthropic\"\nbase_url = \"https://api.anthropic.com\"\nprotocol = \"anthropic_messages\"\nauth = \"api_key\"\napi_header = { name = \"x-api-key\", scheme = \"raw\" }\nbeta_headers = [[\"anthropic-version\", \"2023-06-01\"]]\ngeneration_query = [[\"beta\", \"true\"]]\nmodel_aliases = { sonnet = \"claude-3-5-sonnet\" }\nbody_defaults = { max_tokens = 4096 }\n",
    );
    let row = cfg.row("anthropic").unwrap();
    assert_eq!(row.base_url.as_deref(), Some("https://api.anthropic.com"));
    assert_eq!(row.protocol, Some(ProtocolId::AnthropicMessages));
    assert_eq!(row.auth, Some(AuthId::ApiKey));
    assert_eq!(row.api_header.as_ref().unwrap().name, "x-api-key");
    assert_eq!(row.beta_headers.as_ref().unwrap().len(), 1);
    assert_eq!(
        row.generation_query.as_deref(),
        Some(&[("beta".to_string(), "true".to_string())][..])
    );
    assert_eq!(
        row.model_aliases.as_ref().unwrap().get("sonnet").unwrap(),
        "claude-3-5-sonnet"
    );
    assert_eq!(row.body_defaults.get("max_tokens"), Some(&json!(4096)));
    assert_eq!(row.clone(), *row);
}

#[test]
fn deserializes_the_system_prompt_as_a_content_vec() {
    // A config-file `system` decodes through the canonical `Content` repr; each
    // bare array string is a `Content::Text`.
    let cfg = parse("system = [\"You are helpful\"]\n");
    assert_eq!(
        cfg.system,
        Some(vec![Content::Text("You are helpful".into())])
    );
}

#[test]
fn or_folds_the_system_prompt_like_any_scalar() {
    let hi = parse("system = [\"hi\"]\n");
    let lo = parse("system = [\"lo\"]\n");
    // hi present -> hi wins.
    assert_eq!(
        hi.or(lo.clone()).system,
        Some(vec![Content::Text("hi".into())])
    );
    // hi None -> defers to lo.
    assert_eq!(
        PartialConfig::default().or(lo).system,
        Some(vec![Content::Text("lo".into())])
    );
}

#[test]
fn or_folds_the_transport_timeout_like_any_scalar() {
    // The one silence budget folds like any scalar: hi present wins, hi None defers.
    let hi = parse("timeout = 5\n");
    let lo = parse("timeout = 99\n");
    assert_eq!(hi.clone().or(lo.clone()).timeout, Some(5)); // hi wins
    assert_eq!(PartialConfig::default().or(lo).timeout, Some(99)); // hi None -> lo
}

#[test]
fn unmodeled_top_level_keys_land_in_the_extra_valve() {
    let cfg = parse("safe_prompt = true\nrandom_seed = 7\n");
    assert_eq!(cfg.extra.get("safe_prompt"), Some(&json!(true)));
    assert_eq!(cfg.extra.get("random_seed"), Some(&json!(7)));
}

#[test]
fn a_duplicate_provider_name_is_rejected() {
    let err = crate::parse_config(
        "[[provider]]\nname = \"x\"\nbase_url = \"a\"\n[[provider]]\nname = \"x\"\nbase_url = \"b\"\n",
    )
    .unwrap_err();
    assert!(format!("{err}").contains("duplicate provider name"));
}

#[test]
fn a_typo_in_a_provider_row_is_rejected() {
    // `deny_unknown_fields` on the row turns a misspelled key into a parse error.
    let err = crate::parse_config("[[provider]]\nname = \"x\"\nbas_url = \"a\"\n").unwrap_err();
    assert!(!format!("{err}").is_empty());
}

#[test]
fn a_misplaced_top_level_key_under_oauth_is_rejected() {
    // `deny_unknown_fields` on `OAuthConfig` catches a TOP-LEVEL row key (here
    // `unsupported_body_keys`) mistakenly typed inside `[provider.oauth]`. Without
    // the deny it vanished silently and the strip never fired (bl-9649, bl-2869).
    let err = crate::parse_config(
        "[[provider]]\nname = \"x\"\n[provider.oauth]\nauthorize_url = \"a\"\ntoken_url = \"t\"\nclient_id = \"c\"\nunsupported_body_keys = [\"max_tokens\"]\n",
    )
    .unwrap_err();
    assert!(!format!("{err}").is_empty());
}

#[test]
fn a_typo_in_the_oauth_redirect_block_is_rejected() {
    // `deny_unknown_fields` on `RedirectSpec` makes a misspelled nested key error.
    let err = crate::parse_config(
        "[[provider]]\nname = \"x\"\n[provider.oauth]\nauthorize_url = \"a\"\ntoken_url = \"t\"\nclient_id = \"c\"\nredirect = { prt = 1455 }\n",
    )
    .unwrap_err();
    assert!(!format!("{err}").is_empty());
}

#[test]
fn a_non_table_top_level_is_a_type_error() {
    // Drives the visitor's `expecting` message.
    let err = serde_json::from_value::<PartialConfig>(json!(5)).unwrap_err();
    assert!(format!("{err}").contains("brazen config table"));
}

#[test]
fn or_lets_the_higher_layer_win_and_none_defer() {
    let hi = parse("model = \"hi\"\ntemperature = 0.1\n");
    let lo = parse("model = \"lo\"\ntop_p = 0.9\nmax_tokens = 50\nthinking = true\n");
    let merged = hi.or(lo);
    assert_eq!(merged.model.as_deref(), Some("hi")); // hi wins
    assert_eq!(merged.temperature, Some(0.1)); // only hi
    assert_eq!(merged.top_p, Some(0.9)); // hi None -> defers to lo
    assert_eq!(merged.max_tokens, Some(50)); // only lo
    assert_eq!(merged.thinking, Some(true)); // hi None -> defers to lo
}

#[test]
fn or_merges_the_provider_table_per_key_per_field() {
    // A higher layer patching one field leaves the rest deferring (config §3.2), and
    // `body_defaults` itself merges per-key under `or_map` (config §3.2, §4.1).
    let hi = parse("[[provider]]\nname = \"anthropic\"\nbody_defaults = { max_tokens = 8192 }\n");
    let lo = parse(
        "[[provider]]\nname = \"anthropic\"\nbase_url = \"u\"\nbody_defaults = { max_tokens = 4096, store = false }\n[[provider]]\nname = \"openai\"\nbase_url = \"o\"\n",
    );
    let merged = hi.or(lo);
    let anthropic = merged.row("anthropic").unwrap();
    assert_eq!(
        anthropic.body_defaults.get("max_tokens"),
        Some(&json!(8192))
    ); // hi key wins
    assert_eq!(anthropic.body_defaults.get("store"), Some(&json!(false))); // lo-only key survives
    assert_eq!(anthropic.base_url.as_deref(), Some("u")); // hi None -> defers
                                                          // A key only in the lower layer passes through untouched.
    assert_eq!(merged.row("openai").unwrap().base_url.as_deref(), Some("o"));
}

#[test]
fn deserializes_and_folds_model_prefixes() {
    // Whole-list row fields replace rather than merge. `generation_query` is the
    // generation-URL sibling of `model_prefixes` and obeys the same Option fold.
    let hi_query =
        parse("[[provider]]\nname = \"anthropic\"\ngeneration_query = [[\"hi\", \"a\"]]\n");
    let lo_query =
        parse("[[provider]]\nname = \"anthropic\"\ngeneration_query = [[\"lo\", \"b\"]]\n");
    assert_eq!(
        hi_query
            .or(lo_query)
            .row("anthropic")
            .unwrap()
            .generation_query,
        Some(vec![("hi".into(), "a".into())])
    );

    // The routing-ownership field parses as a string list and folds whole-Option,
    // like `model_aliases` — a higher layer's list replaces the lower's (arch §4.3).
    let row = parse("[[provider]]\nname = \"anthropic\"\nmodel_prefixes = [\"claude-\"]\n");
    assert_eq!(
        row.row("anthropic").unwrap().model_prefixes.as_deref(),
        Some(&["claude-".to_string()][..])
    );
    let hi = parse("[[provider]]\nname = \"anthropic\"\nmodel_prefixes = [\"hi-\"]\n");
    let lo = parse("[[provider]]\nname = \"anthropic\"\nmodel_prefixes = [\"lo-\"]\n");
    assert_eq!(
        hi.or(lo.clone()).row("anthropic").unwrap().model_prefixes,
        Some(vec!["hi-".to_string()]) // hi present wins whole
    );
    assert_eq!(
        PartialConfig::default()
            .or(lo)
            .row("anthropic")
            .unwrap()
            .model_prefixes,
        Some(vec!["lo-".to_string()]) // hi None -> defers to lo
    );
}

#[test]
fn or_lets_the_higher_extra_key_win() {
    let hi = parse("knob = \"hi\"\n");
    let lo = parse("knob = \"lo\"\nother = 1\n");
    let merged = hi.or(lo);
    assert_eq!(merged.extra.get("knob"), Some(&json!("hi")));
    assert_eq!(merged.extra.get("other"), Some(&json!(1)));
}
