//! End-to-end `bz --list-providers` (config §6.1): the EFFECTIVE provider table —
//! the merged fold WITH the defaults operand `--dump-config` drops — in `providers`
//! order, each row's `credential` computed offline. The error paths live in
//! `list_providers_errors`; the shared harness in `list_providers_support`.

use std::collections::BTreeMap;

use crate::testing::MemoryCredStore;
use crate::tests::list_providers_support::{floor, go, row};
use crate::tests::run_support::temp;
use crate::{Cred, EnvSnapshot, Secret};

/// The listing shows the BUILT-IN rows — the whole reason it exists (config §6.1):
/// `--dump-config` drops the defaults operand and so can never print them.
#[test]
fn lists_the_effective_table_the_dump_cannot_show() {
    let out = floor(&MemoryCredStore::new());
    assert_eq!(out.code, 0);
    assert_eq!(out.stderr, "");
    let names: Vec<&str> = out
        .stdout
        .lines()
        .map(|l| l.split_whitespace().next().unwrap_or_default())
        .collect();
    // `providers` order IS routing priority (arch §4.3.1): the head is the row a
    // bare `bz "q"` reaches. Asserted as the whole ordered list, not a set.
    assert_eq!(
        names,
        [
            "anthropic",
            "openai",
            "mistral",
            "openai-responses",
            "google",
            "ollama",
            "claude-code",
            // The one built-in oauth2 row (auth §10.5), last so it moves no routing.
            "openai-chatgpt",
        ]
    );
    // The dump of the same (empty) config prints none of them (config §6, bl-d67a).
    let dump = crate::dump_config(
        crate::PartialConfig::default(),
        &EnvSnapshot(BTreeMap::new()),
        crate::PartialConfig::default(),
    )
    .unwrap();
    assert!(!dump.contains("anthropic"), "{dump}");
}

/// The six facts, spelled as the config file spells them (the serde rename, §6.1),
/// space-padded to the widest value in each column — one line per row, no header.
/// `tuning` and `device` sit before `credential`, the one column whose value can
/// carry a space.
#[test]
fn renders_padded_columns_in_config_spelling() {
    let out = floor(&MemoryCredStore::new());
    // Widths come from the widest value: `openai-responses` (16), `google_generative_ai`
    // (20), `api_key` (7). Asserted literally so the alignment contract is pinned.
    assert_eq!(
        out.stdout.lines().next().unwrap_or_default(),
        "anthropic         anthropic_messages    api_key  effort,priority  tools,multi_turn  -      missing"
    );
    assert_eq!(
        row(&out.stdout, "google"),
        // Google has a thinkingConfig but no lane field → `effort` alone (providers §6.2).
        [
            "google",
            "google_generative_ai",
            "api_key",
            "effort",
            "tools,multi_turn",
            "-",
            "missing"
        ]
    );
    // A keyless row reads no credential at all (auth §3.1) — never "missing".
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
    // The one built-in row that serves a HEADLESS sign-in names its flow STYLE
    // (auth §10.8); every other row can only be signed in through `--browser`.
    assert_eq!(row(&out.stdout, "openai-chatgpt")[5], "codex");
}

/// `credential` is `resolved_secret`'s answer minus the network (config §6.1): a
/// stored cred on one row leaves every other row's answer untouched.
#[test]
fn stored_credential_is_per_row() {
    let store = MemoryCredStore::with(
        "openai",
        Cred::Bearer {
            token: Secret::new("t"),
        },
    );
    let out = floor(&store);
    assert_eq!(row(&out.stdout, "openai")[6], "stored");
    assert_eq!(row(&out.stdout, "mistral")[6], "missing");
}

/// A store MISS falling through to the row's `ambient` block (auth §5.5) is reported
/// as `ambient`, not `stored` — the provenance the fetch preserves.
#[test]
fn ambient_discovery_is_reported_as_ambient() {
    let store = MemoryCredStore::with_ambient(Cred::ApiKey {
        key: Secret::new("k"),
    });
    let out = floor(&store);
    // `anthropic` is the one floor row with an `ambient` block (ANTHROPIC_API_KEY).
    assert_eq!(row(&out.stdout, "anthropic")[6], "ambient");
    assert_eq!(row(&out.stdout, "openai")[6], "missing");
}

/// `--api-key`/`BRAZEN_API_KEY` is provider-AGNOSTIC (config §3.4): it shadows the
/// store on every KEYED row — and on none of the others.
#[test]
fn the_inline_key_shows_on_every_keyed_row() {
    let out = go(
        &["--list-providers", "--api-key", "sk"],
        &[("BRAZEN_CONFIG", "/nope.toml")],
        &MemoryCredStore::new(),
    );
    assert_eq!(row(&out.stdout, "anthropic")[6], "inline");
    assert_eq!(row(&out.stdout, "openai")[6], "inline");
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
}

const OAUTH_ROW: &str = r#"
[[provider]]
name = "sso"
base_url = "https://example.test"
protocol = "openai_responses"
auth = "oauth2"
api_header = { name = "Authorization", scheme = "bearer" }
oauth = { authorize_url = "https://a", token_url = "https://t", client_id = "c" }
"#;

/// An `oauth2` row's `Auth` impl never reads `inline_key` (auth §3.1), so the column
/// must not claim it would — and a user row precedes the built-in floor (config §3.2).
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
        out.stdout
            .lines()
            .next()
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>(),
        [
            "sso",
            "openai_responses",
            "oauth2",
            "effort,priority",
            "tools,multi_turn",
            "-",
            "missing"
        ]
    );
    assert_eq!(row(&out.stdout, "anthropic")[6], "inline");
}

/// The object form is the resolved `OutMode`, not the `--json` flag alone — the same
/// fold `--list-models` reads (model-discovery §2).
#[test]
fn ndjson_output_emits_the_providers_object() {
    for (argv, env) in [
        (
            vec!["--list-providers", "--json"],
            vec![("BRAZEN_CONFIG", "/nope.toml")],
        ),
        (
            vec!["--list-providers"],
            vec![("BRAZEN_CONFIG", "/nope.toml"), ("BRAZEN_OUTPUT", "ndjson")],
        ),
    ] {
        let out = go(&argv, &env, &MemoryCredStore::new());
        assert_eq!(out.code, 0);
        let v: serde_json::Value = serde_json::from_str(out.stdout.trim()).unwrap();
        let rows = v["providers"].as_array().unwrap();
        assert_eq!(rows.len(), 8);
        assert_eq!(rows[0]["name"], "anthropic");
        assert_eq!(rows[0]["protocol"], "anthropic_messages");
        assert_eq!(rows[0]["auth"], "api_key");
        assert_eq!(rows[0]["credential"], "missing");
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
        let ollama = rows.iter().find(|r| r["name"] == "ollama").unwrap();
        assert_eq!(ollama["effort"], true);
        assert_eq!(ollama["priority"], false); // no lane field on a local runner
    }
}
