//! The provider table as a PRIORITY LIST (arch §4.3, §4.3.1; config §2.2, §3.2,
//! §7): routing is a greedy first match over `providers` in declaration order,
//! so several rows may own one model and the earliest wins — the `AmbiguousModel`
//! error is retired. Each winner here is paired with the SAME config reordered,
//! because only the flip proves the ORDER decided it and not an accident of the
//! structure. The merge's position laws (a redeclared row hoists; unredeclared
//! rows tail in the lower layer's order) and the one validation greedy-first
//! makes load-bearing (an empty prefix element) live here too.

use crate::tests::config_support::{file, no_env, req, resolve};

use crate::{ConfigError, PartialConfig};

/// A complete row named `name`, claiming `model` however `claim` spells it —
/// so a test states only the fact under scrutiny: which row comes first.
fn row(name: &str, claim: &str) -> String {
    format!(
        "[[provider]]\nname = \"{name}\"\nbase_url = \"https://{name}.test\"\nprotocol = \"openai_chat\"\nauth = \"bearer\"\napi_header = {{ name = \"Authorization\", scheme = \"bearer\" }}\n{claim}\n"
    )
}

/// Route `model` against a file of `rows`, with no `--provider` and no defaults.
fn route(rows: &str, model: &str) -> Result<String, ConfigError> {
    resolve(
        PartialConfig::default(),
        &no_env(),
        file(rows),
        PartialConfig::default(),
        Some(&req(model)),
    )
    .map(|cfg| cfg.provider.name)
}

#[test]
fn the_first_owner_wins_and_reordering_flips_it() {
    // Greedy-first over the priority list, twice over — because the rule never asks
    // WHICH KIND of claim matched, only which row came first (arch §4.3):
    //   * alias-over-prefix: the exact `model_aliases` spelling wins ONLY by being
    //     declared first, not by being more specific — reorder and the prefix takes it.
    //     This is the one-line masquerade of ingress §6: an alias row above `openai`
    //     diverts `gpt-4o` alone while the prefix row keeps every other `gpt-…`.
    //   * prefix-over-prefix: two rows claiming one family is no longer an error at
    //     all (the retired `AmbiguousModel`) — it is the operator's list saying which.
    let alias = row(
        "aliaser",
        "model_aliases = { \"gpt-4o\" = \"claude-sonnet-4-6\" }",
    );
    let prefix = row("prefixer", "model_prefixes = [\"gpt-\"]");
    assert_eq!(
        route(&format!("{alias}{prefix}"), "gpt-4o").unwrap(),
        "aliaser"
    );
    assert_eq!(
        route(&format!("{prefix}{alias}"), "gpt-4o").unwrap(),
        "prefixer",
        "reorder must flip the winner — the ORDER decides, nothing else"
    );

    let a = row("a", "model_prefixes = [\"shared-\"]");
    let b = row("b", "model_prefixes = [\"shared-\"]");
    assert_eq!(route(&format!("{a}{b}"), "shared-7").unwrap(), "a");
    assert_eq!(route(&format!("{b}{a}"), "shared-7").unwrap(), "b");
}

#[test]
fn a_user_row_outranks_a_built_in_defaults_row_claiming_the_same_family() {
    // The merge puts the higher layer's rows first (config §3.2), so a user row
    // claiming `gpt-` beats the built-in `openai` that claims the same family —
    // with the REAL defaults folded in. Under the retired ambiguity rule this
    // config could not route at all without disarming `openai`'s whole family.
    let cfg = resolve(
        PartialConfig::default(),
        &no_env(),
        file(&row("mine", "model_prefixes = [\"gpt-\"]")),
        crate::defaults(),
        Some(&req("gpt-5.4")),
    )
    .unwrap();
    assert_eq!(cfg.provider.name, "mine");

    // The negative: the SAME two rows, order reversed. `--provider` is the one
    // order-insensitive step (config §7 step 1), so naming `openai` reaches the
    // lower-priority row regardless — the mitigation the ruling relies on.
    let named = resolve(
        PartialConfig {
            provider: Some("openai".into()),
            ..Default::default()
        },
        &no_env(),
        file(&row("mine", "model_prefixes = [\"gpt-\"]")),
        crate::defaults(),
        Some(&req("gpt-5.4")),
    )
    .unwrap();
    assert_eq!(named.provider.name, "openai");
}

#[test]
fn redeclaring_a_defaults_row_hoists_it_with_the_claims_it_never_mentioned() {
    // The sharp edge of per-field merge meeting priority (config §3.2), pinned so it
    // cannot regress silently: a user row naming a defaults row with ONLY a
    // `body_defaults` tweak still resolves with the DEFAULTS' `model_prefixes` —
    // fields fall through — while taking the USER FILE's position. So touching one
    // field of a defaults row is also a priority claim for everything that row owns:
    // `openai` sits above `mine` here and takes `gpt-5.4`, though `mine` is the only
    // row in the user file that spells a `gpt-` claim at all.
    let user = format!(
        "[[provider]]\nname = \"openai\"\nbody_defaults = {{ max_tokens = 8192 }}\n{}",
        row("mine", "model_prefixes = [\"gpt-\"]")
    );
    let cfg = resolve(
        PartialConfig::default(),
        &no_env(),
        file(&user),
        crate::defaults(),
        Some(&req("gpt-5.4")),
    )
    .unwrap();
    assert_eq!(cfg.provider.name, "openai");
    assert_eq!(cfg.max_tokens, Some(8192)); // the user's one field
    assert_eq!(cfg.provider.base_url, "https://api.openai.com/v1"); // the defaults' rest

    // Declare the owning row FIRST and the same tweak is harmless — the remedy
    // config §3.2 states ("declare the row that owns a model before any row you
    // redeclare"), and the negative that shows the hoist, not the row, decided it.
    let reordered = format!(
        "{}[[provider]]\nname = \"openai\"\nbody_defaults = {{ max_tokens = 8192 }}\n",
        row("mine", "model_prefixes = [\"gpt-\"]")
    );
    let cfg = resolve(
        PartialConfig::default(),
        &no_env(),
        file(&reordered),
        crate::defaults(),
        Some(&req("gpt-5.4")),
    )
    .unwrap();
    assert_eq!(cfg.provider.name, "mine");
}

#[test]
fn the_merge_tails_unredeclared_rows_in_the_lower_layers_order() {
    // The third position law (config §3.2): hi's rows in hi's order, then LO's rows
    // hi never mentioned, in LO's order. Redeclaring `openai` lifts it out of the
    // defaults' middle to the head; every row the user never named keeps the
    // defaults' own relative order behind it, none dropped, none re-sorted.
    let user = file("[[provider]]\nname = \"openai\"\nbody_defaults = { max_tokens = 8192 }\n");
    let merged = user.or(crate::defaults());
    let names: Vec<&str> = merged.providers.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        [
            "openai",    // hoisted: hi's only row, at hi's position
            "anthropic", // …then lo's unredeclared rows, in lo's order
            "mistral",
            "openai-responses",
            "google",
            "ollama",
            "claude-code",
        ]
    );
}

#[test]
fn an_empty_prefix_element_is_bad_value_but_an_empty_prefix_list_is_fine() {
    // The one validation greedy-first makes load-bearing (config §7). The pair is the
    // point — only the contrast pins the rule:
    //   * `model_prefixes = [""]` — an empty ELEMENT — would own EVERY model
    //     (`"anything".starts_with("")`), and declared early would silently swallow
    //     all routing with no diagnostic. It is a typo, never an authored priority.
    //   * `model_prefixes = []` — the empty LIST, claiming nothing — is how a row
    //     opts out of family routing (`openai-responses` ships it). Legal.
    let err = route(&row("greedy", "model_prefixes = [\"\"]"), "anything").unwrap_err();
    match err {
        ConfigError::BadValue { key, detail } => {
            assert_eq!(key, "model_prefixes");
            assert!(
                detail.contains("greedy"),
                "names the offending row: {detail}"
            );
        }
        other => panic!("expected BadValue, got {other:?}"),
    }
    // It is refused even when routing would never consult it — the row cannot be
    // meant, so it is not honored anywhere.
    let with_named = resolve(
        PartialConfig {
            provider: Some("other".into()),
            ..Default::default()
        },
        &no_env(),
        file(&format!(
            "{}{}",
            row("other", "model_prefixes = [\"x-\"]"),
            row("greedy", "model_prefixes = [\"\"]")
        )),
        PartialConfig::default(),
        Some(&req("x-1")),
    );
    assert!(with_named.is_err());

    // The empty LIST resolves fine; it simply owns nothing, so a named provider is
    // the way in and an unowned model is `NoProvider` as ever.
    let rows = row("quiet", "model_prefixes = []");
    assert_eq!(
        route(&rows, "anything").unwrap_err(),
        ConfigError::NoProvider
    );
    let named = resolve(
        PartialConfig {
            provider: Some("quiet".into()),
            ..Default::default()
        },
        &no_env(),
        file(&rows),
        PartialConfig::default(),
        Some(&req("anything")),
    )
    .unwrap();
    assert_eq!(named.provider.name, "quiet");
}
