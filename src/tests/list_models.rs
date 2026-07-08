//! End-to-end `bz --list-models` control flag (model-discovery §2): provider resolution (the
//! same `into_resolved(None)` query), the one models GET (auth + the row's required
//! `anthropic-version` header), the two output shapes (`--json`/`BRAZEN_OUTPUT=ndjson`
//! object, default text), the no-provider DEFAULT to the first row, and the cache write.
//! The error paths live in `list_models_errors`. `MockTransport`; offline. The shared
//! harness lives in `list_models_support`.

use std::collections::BTreeMap;

use crate::testing::{MemoryCredStore, MockTransport};
use crate::tests::list_models_support::{go, go_cache, go_env, ANT, MODELS};
use crate::{Cred, EnvSnapshot, Method, Model, Secret};

#[test]
fn text_prints_ids_one_per_line_in_provider_order() {
    let tx = MockTransport::ok(vec![MODELS]);
    let o = go(ANT, &tx, &MemoryCredStore::new());
    assert_eq!(o.code, 0);
    assert_eq!(
        o.stdout,
        "claude-opus-4-1-20250805\nclaude-sonnet-4-5-20250929\n"
    );
    assert!(o.stderr.is_empty());
    // The GET targets {base_url}{models_path}, carries auth + the required version.
    let sent = tx.requests();
    assert_eq!(sent[0].method, Method::Get);
    assert_eq!(sent[0].url, "https://api.anthropic.com/v1/models");
    assert_eq!(sent[0].header("x-api-key"), Some("sk"));
    assert_eq!(sent[0].header("anthropic-version"), Some("2023-06-01"));
}

#[test]
fn json_emits_the_models_object() {
    let tx = MockTransport::ok(vec![MODELS]);
    let o = go(
        &[
            "--list-models",
            "--provider",
            "anthropic",
            "--json",
            "--api-key",
            "sk",
        ],
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 0);
    let v: serde_json::Value = serde_json::from_str(&o.stdout).unwrap();
    assert_eq!(v["models"][0]["id"], "claude-opus-4-1-20250805");
    assert_eq!(v["models"][0]["default"], false);
    assert_eq!(v["models"][1]["id"], "claude-sonnet-4-5-20250929");
}

#[test]
fn the_verb_writes_the_decoded_list_to_the_cache() {
    // The SOLE cache write (model-discovery §5): after a successful decode the verb
    // `put`s the decoded list under the provider name — exactly the order/ids the GET
    // returned, the list the generation path later reads. Best-effort, exit unchanged.
    let tx = MockTransport::ok(vec![MODELS]);
    let (o, cache) = go_cache(ANT, &tx, &MemoryCredStore::new());
    assert_eq!(o.code, 0);
    let puts = cache.puts();
    assert_eq!(puts.len(), 1, "exactly one cache write");
    assert_eq!(puts[0].0, "anthropic", "keyed by the provider row name");
    assert_eq!(
        puts[0].1,
        vec![
            Model {
                id: "claude-opus-4-1-20250805".into(),
                default: false,
            },
            Model {
                id: "claude-sonnet-4-5-20250929".into(),
                default: false,
            },
        ],
        "the decoded list, in provider order"
    );
}

#[test]
fn brazen_output_ndjson_emits_the_models_object_with_no_flag() {
    // The output shape is the RESOLVED `OutMode` (flag/env/file), the same fact the data
    // plane folds — not the `--json` flag alone. `BRAZEN_OUTPUT=ndjson` with NO `--json`
    // selects the `{"models":[…]}` object, exactly as the flag does. (The env spelling is
    // `ndjson`; `OutMode::parse` rejects `json` — `--json` is the flag-only alias.)
    let env = EnvSnapshot(BTreeMap::from([(
        "BRAZEN_OUTPUT".to_string(),
        "ndjson".to_string(),
    )]));
    let tx = MockTransport::ok(vec![MODELS]);
    let o = go_env(ANT, &env, &tx, &MemoryCredStore::new());
    assert_eq!(o.code, 0);
    let v: serde_json::Value = serde_json::from_str(&o.stdout).unwrap();
    assert_eq!(v["models"][0]["id"], "claude-opus-4-1-20250805");
    assert_eq!(v["models"][1]["id"], "claude-sonnet-4-5-20250929");
}

#[test]
fn unflagged_ids_carry_no_suffix() {
    // No dialect flags a default today (§3.1), so a real listing has no ` (default)`
    // suffix — the bare ids, one per line. The suffix branch itself (a provider that
    // DOES flag one) is unit-tested on `print_models` in the module (no integration
    // body can set `default:true`).
    let body = br#"{"data":[{"id":"a"},{"id":"b"}]}"#;
    let tx = MockTransport::ok(vec![body]);
    let o = go(
        &["--list-models", "--provider", "openai", "--api-key", "sk"],
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "a\nb\n");
}

#[test]
fn no_provider_lists_the_first_provider() {
    // `bz --list-models` with NO `--provider`: discovery shares the data plane's
    // first-provider default (`into_resolved(None)` → the first row, anthropic by
    // name), so it lists the DEFAULT provider's models — the GET hits anthropic.
    let tx = MockTransport::ok(vec![MODELS]);
    let o = go(
        &["--list-models", "--api-key", "sk"],
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(
        o.stdout,
        "claude-opus-4-1-20250805\nclaude-sonnet-4-5-20250929\n"
    );
    assert_eq!(tx.requests()[0].url, "https://api.anthropic.com/v1/models");
}

#[test]
fn a_stored_credential_is_used_for_the_get() {
    let store = MemoryCredStore::with(
        "anthropic",
        Cred::ApiKey {
            key: Secret::new("sk-store"),
        },
    );
    let tx = MockTransport::ok(vec![MODELS]);
    let o = go(&["--list-models", "--provider", "anthropic"], &tx, &store);
    assert_eq!(o.code, 0);
    assert_eq!(tx.requests()[0].header("x-api-key"), Some("sk-store"));
}

#[test]
fn an_empty_list_prints_nothing_at_0() {
    // The verb LISTS, it does not select: a well-formed EMPTY body is a successful
    // empty listing (0). The empty-cache→Config(78) contract is `select_model`'s, on the
    // generation path (`run_cache`) — not the verb's. The empty list is surfaced HONESTLY
    // on stderr (model-discovery §2/§3.2: a version-gated `[provider.models].query` can
    // silently return empty — never a silent void) but the exit stays 0.
    let tx = MockTransport::ok(vec![br#"{"data":[]}"#]);
    let o = go(ANT, &tx, &MemoryCredStore::new());
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "");
    assert!(o.stderr.contains("no models returned for `anthropic`"));
}
