//! The `[ingress]` table (ingress §6, §7; config §2, §3, §6): parse +
//! deny_unknown_fields, the per-field `or` fold, `--dump-config` redaction and
//! round-trip, and `resolve_ingress` — defaults, the unknown-adaptation check,
//! and the non-loopback-without-token refuse-to-start rule.

use crate::{
    dump_config, parse_config, ConfigError, EnvSnapshot, IngressConfig, LossyMode, PartialConfig,
    PartialIngress,
};

fn parse(s: &str) -> PartialConfig {
    parse_config(s).unwrap()
}

const FULL_TABLE: &str = "[ingress]\ndialect = \"openai_chat\"\nlisten = \"0.0.0.0:4891\"\ntoken = \"tok-secret\"\nlossy = \"reject\"\nlossy_overrides = { thinking_replay = \"adapt\" }\n";

#[test]
fn deserializes_the_ingress_table() {
    let cfg = parse(FULL_TABLE);
    let ing: &PartialIngress = cfg.ingress.as_ref().unwrap();
    assert_eq!(ing.dialect.as_deref(), Some("openai_chat"));
    assert_eq!(ing.listen.as_deref(), Some("0.0.0.0:4891"));
    assert_eq!(
        ing.token.as_ref().map(crate::Secret::expose),
        Some("tok-secret")
    );
    assert_eq!(ing.lossy, Some(LossyMode::Reject));
    assert_eq!(
        ing.lossy_overrides.get("thinking_replay"),
        Some(&LossyMode::Adapt)
    );
    // Decoded by the named arm, never the `extra` valve (config §2.3).
    assert!(cfg.extra.is_empty());
    // Clone + Debug + PartialEq.
    assert_eq!(ing.clone(), *ing);
    assert!(!format!("{ing:?}").is_empty());
}

#[test]
fn a_typo_in_the_ingress_table_is_a_malformed_file() {
    // `deny_unknown_fields` like a provider row (ingress §6, config §2.3).
    let err = parse_config("[ingress]\ndialct = \"openai_chat\"\n").unwrap_err();
    assert!(matches!(err, ConfigError::MalformedFile { .. }), "{err:?}");
    // An unknown lossy SPELLING is a parse error too (the closed enum).
    let err = parse_config("[ingress]\nlossy = \"adaqt\"\n").unwrap_err();
    assert!(matches!(err, ConfigError::MalformedFile { .. }), "{err:?}");
}

#[test]
fn or_folds_the_table_per_field_and_overrides_per_key() {
    let hi = parse(
        "[ingress]\ndialect = \"openai_chat\"\nlossy_overrides = { thinking_replay = \"reject\" }\n",
    );
    let lo = parse(
        "[ingress]\ndialect = \"anthropic_messages\"\nlisten = \"127.0.0.1:9000\"\nlossy = \"reject\"\nlossy_overrides = { thinking_replay = \"adapt\", document_url_drop = \"reject\" }\n",
    );
    let merged = hi.or(lo).ingress.unwrap();
    assert_eq!(merged.dialect.as_deref(), Some("openai_chat")); // hi wins
    assert_eq!(merged.listen.as_deref(), Some("127.0.0.1:9000")); // hi None -> lo
    assert_eq!(merged.lossy, Some(LossyMode::Reject)); // only lo
                                                       // Per-key merge, like body_defaults (config §3.2): hi key wins, lo-only survives.
    assert_eq!(
        merged.lossy_overrides.get("thinking_replay"),
        Some(&LossyMode::Reject)
    );
    assert_eq!(
        merged.lossy_overrides.get("document_url_drop"),
        Some(&LossyMode::Reject)
    );
}

#[test]
fn a_missing_table_is_the_fold_identity() {
    let some = parse("[ingress]\ndialect = \"openai_chat\"\n");
    // Table in one layer passes through from either side; in neither, stays None.
    let kept = some.clone().or(PartialConfig::default()).ingress.unwrap();
    assert_eq!(kept.dialect.as_deref(), Some("openai_chat"));
    let kept = PartialConfig::default().or(some).ingress.unwrap();
    assert_eq!(kept.dialect.as_deref(), Some("openai_chat"));
    assert_eq!(
        PartialConfig::default()
            .or(PartialConfig::default())
            .ingress,
        None
    );
}

#[test]
fn dump_redacts_the_token_and_round_trips() {
    let out = dump_config(
        parse(FULL_TABLE),
        &EnvSnapshot::default(),
        PartialConfig::default(),
    )
    .unwrap();
    assert!(out.contains("[ingress]"), "{out}");
    assert!(out.contains("token = \"<redacted>\""), "{out}");
    assert!(!out.contains("tok-secret"), "{out}");
    // The dump re-parses to the same merged partial, token now the inert
    // sentinel — the round-trip law (config §6, §2.2).
    let reparsed = parse(&out);
    let mut redacted = parse(FULL_TABLE);
    redacted.ingress.as_mut().unwrap().token = Some(crate::Secret::new("<redacted>"));
    assert_eq!(reparsed, redacted);
}

#[test]
fn dump_keeps_a_tokenless_table_sparse() {
    // Absent fields stay absent (no token/listen keys invented), and the
    // no-token redact path leaves the table untouched.
    let out = dump_config(
        parse("[ingress]\ndialect = \"openai_chat\"\n"),
        &EnvSnapshot::default(),
        PartialConfig::default(),
    )
    .unwrap();
    assert!(out.contains("dialect = \"openai_chat\""), "{out}");
    assert!(!out.contains("token"), "{out}");
    assert!(!out.contains("listen"), "{out}");
    assert!(!out.contains("lossy"), "{out}");
    // And the mirror image — a dialect-less table dumps without inventing one
    // (requiredness is resolution's, never the dump's).
    let out = dump_config(
        parse("[ingress]\nlisten = \"127.0.0.1:9000\"\n"),
        &EnvSnapshot::default(),
        PartialConfig::default(),
    )
    .unwrap();
    assert!(out.contains("listen = \"127.0.0.1:9000\""), "{out}");
    assert!(!out.contains("dialect"), "{out}");
}

#[test]
fn resolves_loopback_defaults_with_no_token() {
    // The zero-knob table: dialect alone. Defaults are RESOLUTION's (ingress
    // §6): loopback listen, lossy = adapt — and loopback needs no token.
    let ing: IngressConfig = parse("[ingress]\ndialect = \"openai_chat\"\n")
        .resolve_ingress()
        .unwrap();
    assert_eq!(ing.dialect, "openai_chat");
    assert_eq!(ing.listen.to_string(), "127.0.0.1:4891");
    assert!(ing.listen.ip().is_loopback());
    assert_eq!(ing.token, None);
    assert_eq!(ing.lossy, LossyMode::Adapt);
    assert!(ing.lossy_overrides.is_empty());
    // Clone + Debug + PartialEq.
    assert_eq!(ing.clone(), ing);
    assert!(!format!("{ing:?}").is_empty());
}

#[test]
fn resolves_a_full_table() {
    let ing = parse(FULL_TABLE).resolve_ingress().unwrap();
    assert_eq!(ing.listen.to_string(), "0.0.0.0:4891");
    assert_eq!(
        ing.token.as_ref().map(crate::Secret::expose),
        Some("tok-secret")
    );
    assert_eq!(ing.lossy, LossyMode::Reject);
    // The per-case policy QUERY (ingress §4): the override wins for its name;
    // an un-overridden name falls to the global default.
    assert_eq!(ing.lossy_for("thinking_replay"), LossyMode::Adapt);
    assert_eq!(ing.lossy_for("document_url_drop"), LossyMode::Reject);
}

#[test]
fn no_table_names_the_missing_table() {
    // `--serve` with no `[ingress]` table is Config/78 naming it (ingress §6).
    let err = PartialConfig::default().resolve_ingress().unwrap_err();
    assert!(matches!(err, ConfigError::Ingress { .. }), "{err:?}");
    let msg = format!("{err}");
    assert!(msg.contains("[ingress]"), "{msg}");
    assert!(msg.contains("--serve"), "{msg}");
}

#[test]
fn a_dialect_less_table_cannot_serve() {
    // Required only to serve — parse accepted it; resolution refuses (ingress §2, §6).
    let err = parse("[ingress]\nlisten = \"127.0.0.1:9000\"\n")
        .resolve_ingress()
        .unwrap_err();
    assert!(format!("{err}").contains("dialect"), "{err}");
}

#[test]
fn an_unknown_adaptation_name_is_an_error() {
    // A typo'd override must never silently leave the default in force
    // (ingress §4): the unknown name is surfaced with the known vocabulary.
    let err = parse(
        "[ingress]\ndialect = \"openai_chat\"\nlossy_overrides = { thinking_reply = \"reject\" }\n",
    )
    .resolve_ingress()
    .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.starts_with("ingress:"), "{msg}");
    assert!(msg.contains("thinking_reply"), "{msg}");
    assert!(msg.contains("thinking_replay, document_url_drop"), "{msg}");
}

#[test]
fn an_unparseable_listen_is_an_error() {
    // The bind address is a numeric `ip:port`; a hostname cannot be proven
    // loopback without IO, so it is rejected at resolution, not at bind.
    let err = parse("[ingress]\ndialect = \"openai_chat\"\nlisten = \"localhost:4891\"\n")
        .resolve_ingress()
        .unwrap_err();
    assert!(format!("{err}").contains("localhost:4891"), "{err}");
}

#[test]
fn non_loopback_without_token_refuses_to_start() {
    // The ingress §7 security posture: a routable listener wired to the
    // operator's credentials must be a deliberate, authenticated act.
    let err = parse("[ingress]\ndialect = \"openai_chat\"\nlisten = \"0.0.0.0:4891\"\n")
        .resolve_ingress()
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("non-loopback"), "{msg}");
    assert!(msg.contains("token"), "{msg}");
}

#[test]
fn non_loopback_with_token_and_v6_loopback_start() {
    // A token authenticates the routable bind (ingress §7)…
    let ing =
        parse("[ingress]\ndialect = \"openai_chat\"\nlisten = \"0.0.0.0:4891\"\ntoken = \"t\"\n")
            .resolve_ingress()
            .unwrap();
    assert!(!ing.listen.ip().is_loopback());
    // …and IPv6 loopback is loopback — no token demanded.
    let ing = parse("[ingress]\ndialect = \"openai_chat\"\nlisten = \"[::1]:4891\"\n")
        .resolve_ingress()
        .unwrap();
    assert!(ing.listen.ip().is_loopback());
    assert_eq!(ing.token, None);
}
