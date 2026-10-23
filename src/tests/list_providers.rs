//! End-to-end `bz --list-providers` (config §6.1): the routing TABLE — the rows the
//! operator named plus the catalog presets a credential resolves for (config §3.2) —
//! in `providers` order, each row's `credential` computed offline. The error paths
//! live in `list_providers_errors`; the shared harness in `list_providers_support`.

use std::collections::BTreeMap;

use crate::testing::MemoryCredStore;
use crate::tests::list_providers_support::{floor, go, row};
use crate::tests::run_support::temp;
use crate::{Cred, EnvSnapshot, Secret};

/// A store holding the one built-in oauth2 preset's sign-in, so it is in the table.
fn chatgpt_store() -> MemoryCredStore {
    MemoryCredStore::with(
        "openai-chatgpt",
        Cred::OAuth2 {
            access_token: Secret::new("at"),
            refresh_token: Secret::new("rt"),
            expires_at: 4_000_000_000,
            scope: None,
            account_id: None,
        },
    )
}

fn names(stdout: &str) -> Vec<&str> {
    stdout
        .lines()
        .map(|l| l.split_whitespace().next().unwrap_or_default())
        .collect()
}

/// With nothing held and nothing named, the table is EMPTY (config §3.2): a preset is
/// catalog, not table, until an act of the operator's reaches it — and the empty table
/// is the loop over zero rows, exit 0, not a branch. The dump of the same config shows
/// no preset either, for its own reason (config §6).
#[test]
fn an_empty_store_lists_no_preset() {
    let out = floor(&MemoryCredStore::new());
    assert_eq!(out.code, 0);
    assert_eq!(out.stdout, "");
    assert_eq!(out.stderr, "");
    let dump = crate::dump_config(
        crate::PartialConfig::default(),
        &EnvSnapshot(BTreeMap::new()),
        crate::PartialConfig::default(),
    )
    .unwrap();
    assert!(!dump.contains("anthropic"), "{dump}");
}

/// A credential is the act: a stored cred puts ITS preset in the table and no other;
/// the provider-agnostic inline key puts EVERY keyed preset in, in catalog order — the
/// head is the row a bare `bz --api-key K "q"` reaches. Keyless presets (`ollama`,
/// `claude-code`) and the oauth2 preset (whose impl never reads the inline key) stay
/// catalog.
#[test]
fn a_credential_puts_its_preset_in_the_table_in_catalog_order() {
    let stored = floor(&MemoryCredStore::with(
        "openai",
        Cred::Bearer {
            token: Secret::new("t"),
        },
    ));
    assert_eq!(names(&stored.stdout), ["openai"]);
    assert_eq!(row(&stored.stdout, "openai")[6], "stored");
    let inline = go(
        &["--list-providers", "--api-key", "sk"],
        &[("BRAZEN_CONFIG", "/nope.toml")],
        &MemoryCredStore::new(),
    );
    assert_eq!(
        names(&inline.stdout),
        ["anthropic", "openai", "mistral", "openai-responses"]
    );
    assert_eq!(row(&inline.stdout, "anthropic")[6], "inline");
    assert_eq!(row(&inline.stdout, "openai")[6], "inline");
}

/// NAMING a preset is the other act: one line, `[[provider]] name = "ollama"`, and
/// the catalog fills the body. A keyless row reads no credential at all (auth §3.1) —
/// never "missing"; a keyed row named but unfunded says `missing`, which is the one
/// place that word can now appear (bl-526d: a consumer no longer filters it).
#[test]
fn naming_a_preset_puts_it_in_the_table_with_the_catalog_body() {
    let cfg = temp("[[provider]]\nname = \"ollama\"\n[[provider]]\nname = \"anthropic\"\n");
    let out = go(
        &["--list-providers"],
        &[("BRAZEN_CONFIG", cfg.0.to_str().unwrap())],
        &MemoryCredStore::new(),
    );
    assert_eq!(names(&out.stdout), ["ollama", "anthropic"]);
    assert_eq!(
        row(&out.stdout, "ollama"),
        [
            "ollama",
            "ollama_chat",
            "none",
            "effort",
            "tools,multi_turn",
            "-",
            "not",
            "required"
        ]
    );
    assert_eq!(row(&out.stdout, "anthropic")[6], "missing");
}

/// The seven facts, spelled as the config file spells them (the serde rename, §6.1),
/// space-padded to the widest value in each column — one line per row, no header.
/// `tuning` and `device` sit before `credential`, the one column whose value can
/// carry a space.
#[test]
fn renders_padded_columns_in_config_spelling() {
    let out = go(
        &["--list-providers", "--api-key", "sk"],
        &[("BRAZEN_CONFIG", "/nope.toml")],
        &chatgpt_store(),
    );
    // Widths come from the widest value: `openai-responses` (16), `anthropic_messages`
    // (18), `api_key` (7), `codex` (5). Asserted literally so the alignment is pinned.
    assert_eq!(
        out.stdout.lines().next().unwrap_or_default(),
        "anthropic         anthropic_messages  api_key  effort,priority        tools,multi_turn  -      inline"
    );
    // The one built-in row that serves a HEADLESS sign-in names its flow STYLE
    // (auth §10.8); every other row can only be signed in through `--browser`.
    assert_eq!(row(&out.stdout, "openai-chatgpt")[5], "codex");
    assert_eq!(row(&out.stdout, "openai-chatgpt")[6], "stored");
}

/// A store MISS falling through to the row's `ambient` block (auth §5.5) is reported
/// as `ambient`, not `stored` — the provenance the fetch preserves — and it is an act
/// like any other: the discovered row is in the table. The double answers EVERY
/// ambient spec, so each preset carrying a block lists; the two with none do not.
#[test]
fn ambient_discovery_is_reported_as_ambient() {
    let store = MemoryCredStore::with_ambient(Cred::ApiKey {
        key: Secret::new("k"),
    });
    let out = floor(&store);
    assert_eq!(row(&out.stdout, "anthropic")[6], "ambient");
    assert_eq!(row(&out.stdout, "openai")[6], "ambient");
    assert!(row(&out.stdout, "ollama").is_empty());
    assert!(row(&out.stdout, "claude-code").is_empty());
}

const OAUTH_ROW: &str = r#"
[[provider]]
name = "sso"
base_url = "https://example.test"
protocol = "openai_responses"
auth = "oauth2"
oauth = { authorize_url = "https://a", token_url = "https://t", client_id = "c" }
"#;

/// An `oauth2` row's `Auth` impl never reads `inline_key` (auth §3.1), so the column
/// must not claim it would — and a user row precedes the built-in floor (config §3.2).
/// The row names no `api_header`: a bearer-token row completes with the one header it
/// can mean (auth §3.1), so the listing still shows it whole.
#[test]
fn an_oauth_row_ignores_the_inline_key() {
    let cfg = temp(OAUTH_ROW);
    let out = go(
        &["--list-providers", "--api-key", "sk"],
        &[("BRAZEN_CONFIG", cfg.0.to_str().unwrap())],
        &MemoryCredStore::new(),
    );
    // A user row is declared BEFORE the built-in floor (config §3.2), so it heads the
    // priority order — and its oauth2 auth ignores the inline key entirely.
    assert_eq!(
        row(&out.stdout, "sso"),
        [
            "sso",
            "openai_responses",
            "oauth2",
            "effort,priority,image",
            "tools,multi_turn",
            "-",
            "missing"
        ]
    );
    assert_eq!(names(&out.stdout)[0], "sso");
    assert_eq!(row(&out.stdout, "anthropic")[6], "inline");
}

/// The object form is the resolved `OutMode`, not the `--json` flag alone — the same
/// fold `--list-models` reads (model-discovery §2).
#[test]
fn ndjson_output_emits_the_providers_object() {
    for (argv, env) in [
        (
            vec!["--list-providers", "--json", "--api-key", "sk"],
            vec![("BRAZEN_CONFIG", "/nope.toml")],
        ),
        (
            vec!["--list-providers", "--api-key", "sk"],
            vec![("BRAZEN_CONFIG", "/nope.toml"), ("BRAZEN_OUTPUT", "ndjson")],
        ),
    ] {
        let out = go(&argv, &env, &chatgpt_store());
        assert_eq!(out.code, 0);
        let v: serde_json::Value = serde_json::from_str(out.stdout.trim()).unwrap();
        let rows = v["providers"].as_array().unwrap();
        // The four keyed presets the inline key activates + the stored oauth2 one.
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0]["name"], "anthropic");
        assert_eq!(rows[0]["protocol"], "anthropic_messages");
        assert_eq!(rows[0]["auth"], "api_key");
        assert_eq!(rows[0]["credential"], "inline");
        // No `device` block on this row ⇒ no headless sign-in, carried as `null`.
        assert_eq!(rows[0]["device"], serde_json::Value::Null);
        // The one row that has one carries the STYLE, not a bool — the value a
        // consumer above brazen branches its own login spawn on (auth §10.8).
        let chatgpt = rows.iter().find(|r| r["name"] == "openai-chatgpt").unwrap();
        assert_eq!(chatgpt["device"], "codex");
        // The tuning pair rides the object as two BOOLEANS — the machine shape of the
        // text `tuning` column, computed from the same Row (config §6.1). This is the
        // read a consumer above brazen uses to delete its own protocol table.
        assert_eq!(rows[0]["effort"], true);
        assert_eq!(rows[0]["priority"], true);
        let responses = rows
            .iter()
            .find(|r| r["name"] == "openai-responses")
            .unwrap();
        assert_eq!(responses["image"], true);
    }
}
