//! The routing TABLE over the catalog (config §3.2, bl-6feb): a merged row is routable
//! when the operator NAMED it or a CREDENTIAL resolves for it; a catalog preset nobody
//! touched is reachable by explicit `--provider` and nothing else. Also the
//! bearer-header default a `bearer`/`oauth2` row completes with (auth §3.1). The
//! listing's view of the same predicate lives in `list_providers`.

use crate::testing::MemoryCredStore;
use crate::tests::config_support::{file, no_env, req, resolve_table};
use crate::{defaults, AuthId, ConfigError, Cred, HeaderScheme, PartialConfig, Secret};

fn route(
    flags: PartialConfig,
    file_toml: &str,
    model: Option<&str>,
    store: &dyn crate::CredStore,
) -> Result<String, ConfigError> {
    let r = model.map(req);
    resolve_table(
        flags,
        &no_env(),
        file(file_toml),
        defaults(),
        r.as_ref(),
        None,
        store,
    )
    .map(|cfg| cfg.provider.name)
}

#[test]
fn an_untouched_preset_is_catalog_not_table() {
    // The REAL catalog, an empty store, nothing named: the zero-config default has no
    // head and a prefix the `anthropic` preset owns finds no table row — `NoProvider`
    // both ways, never a 77 on a row the operator never chose.
    let empty = MemoryCredStore::new();
    assert_eq!(
        route(PartialConfig::default(), "", None, &empty).unwrap_err(),
        ConfigError::NoProvider
    );
    assert_eq!(
        route(
            PartialConfig::default(),
            "",
            Some("claude-haiku-4-5-20251001"),
            &empty
        )
        .unwrap_err(),
        ConfigError::NoProvider
    );
    // The error names the acts that fill the table.
    let msg = ConfigError::NoProvider.to_string();
    assert!(
        msg.contains("bz --login") && msg.contains("[[provider]]"),
        "{msg}"
    );
    // By NAME the catalog is still read whole: naming the row is the act.
    let picked = PartialConfig {
        provider: Some("anthropic".into()),
        ..Default::default()
    };
    assert_eq!(route(picked, "", None, &empty).unwrap(), "anthropic");
}

#[test]
fn a_credential_puts_the_preset_in_the_table() {
    // Stored (a `bz --login` / a hand-stored key): that preset, and only that preset.
    let stored = MemoryCredStore::with(
        "mistral",
        Cred::Bearer {
            token: Secret::new("t"),
        },
    );
    assert_eq!(
        route(PartialConfig::default(), "", None, &stored).unwrap(),
        "mistral"
    );
    assert_eq!(
        route(PartialConfig::default(), "", Some("gpt-5.4"), &stored).unwrap_err(),
        ConfigError::NoProvider
    );
    // Ambient (the vendor env var another tool already set): the double answers every
    // `ambient` block, so the first keyed preset carrying one heads the table.
    let ambient = MemoryCredStore::with_ambient(Cred::ApiKey {
        key: Secret::new("k"),
    });
    assert_eq!(
        route(PartialConfig::default(), "", None, &ambient).unwrap(),
        "anthropic"
    );
    assert_eq!(
        route(PartialConfig::default(), "", Some("gpt-5.4"), &ambient).unwrap(),
        "openai"
    );
}

#[test]
fn a_keyless_preset_enters_only_by_name() {
    // `auth = "none"` reads no credential, so no credential can be the act: with an
    // inline key EVERY keyed preset is in the table but `ollama` is not…
    let inline = PartialConfig {
        api_key: Some(Secret::new("k")),
        ..Default::default()
    };
    let empty = MemoryCredStore::new();
    assert_eq!(
        route(inline.clone(), "", None, &empty).unwrap(),
        "anthropic"
    );
    assert_eq!(
        route(inline, "", Some("llama3.2"), &empty).unwrap_err(),
        ConfigError::NoProvider
    );
    // …and ONE line in the file — the name — puts it in, body from the catalog, at the
    // head (a user row outranks the presets, config §3.2).
    let cfg = resolve_table(
        PartialConfig::default(),
        &no_env(),
        file("[[provider]]\nname = \"ollama\"\n"),
        defaults(),
        None,
        None,
        &empty,
    )
    .unwrap();
    assert_eq!(cfg.provider.name, "ollama");
    assert_eq!(cfg.provider.auth, AuthId::None);
    assert_eq!(cfg.provider.base_url, "http://localhost:11434");
}

#[test]
fn the_fold_clears_preset_on_any_row_a_layer_names() {
    // The provenance bit is set in exactly one place (`defaults()`) and ANDed by the
    // fold: a user row — even a bare name — is not a preset; an unnamed one still is.
    let d = defaults();
    assert!(d.providers.iter().all(|(_, row)| row.preset));
    let merged = file("[[provider]]\nname = \"openai\"\n").or(defaults());
    assert!(!merged.row("openai").unwrap().preset);
    assert!(merged.row("anthropic").unwrap().preset);
    // A parsed row never carries the bit, and a dump never emits it (`#[serde(skip)]`).
    assert!(
        !file("[[provider]]\nname = \"x\"\n")
            .row("x")
            .unwrap()
            .preset
    );
    let dumped = toml::to_string(&merged.row("openai").unwrap()).unwrap();
    assert!(!dumped.contains("preset"), "{dumped}");
}

#[test]
fn a_bearer_or_oauth2_row_completes_with_the_authorization_header() {
    // The one header those ids can mean (RFC 6750, auth §3.1) — so the catalog states
    // the fact once and a user's bearer row needs no `api_header` line.
    let cfg = resolve_table(
        PartialConfig {
            provider: Some("openai".into()),
            ..Default::default()
        },
        &no_env(),
        PartialConfig::default(),
        defaults(),
        None,
        None,
        &MemoryCredStore::new(),
    )
    .unwrap();
    let hdr = cfg.provider.api_header.unwrap();
    assert_eq!(hdr.name, "Authorization");
    assert_eq!(hdr.scheme, HeaderScheme::Bearer);
    let user = "[[provider]]\nname = \"sso\"\nbase_url = \"u\"\nprotocol = \"openai_responses\"\nauth = \"oauth2\"\noauth = { authorize_url = \"https://a\", token_url = \"https://t\", client_id = \"c\" }\n";
    let cfg = resolve_table(
        PartialConfig {
            provider: Some("sso".into()),
            ..Default::default()
        },
        &no_env(),
        file(user),
        PartialConfig::default(),
        None,
        None,
        &MemoryCredStore::new(),
    )
    .unwrap();
    assert_eq!(
        cfg.provider.api_header.unwrap().scheme,
        HeaderScheme::Bearer
    );
}
