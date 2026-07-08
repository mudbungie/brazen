//! End-to-end `bz --list-models` against a row with a `[provider.models]` override
//! (model-discovery §3.2): the ChatGPT-SSO Codex backend, served by the SAME
//! `openai_responses` protocol as the standard api-key row but with a divergent
//! discovery endpoint — a `?client_version` query the GET must carry (Gap A) and a
//! `{"models":[{"slug":…}]}` body the standard `data`/`id` keys cannot read (Gap B).
//! The override is user-authored config (no embedded `defaults.toml` row carries one),
//! injected here via a temp file + `--config`. `MockTransport`; offline.

use crate::protocol::{ModelKeys, ModelsShape};
use crate::run::models_req;
use crate::testing::{MemoryCredStore, MockTransport};
use crate::tests::list_models_support::go;
use crate::tests::run_support::temp;
use crate::{Method, ModelsOverride};

/// The openai_responses DEFAULT shape (model-discovery §3.1) — what a Codex row
/// overrides via `[provider.models]` (§3.2). The pure `models_req` table tests below
/// drive it directly (the integration tests above exercise the same helper through
/// `fetch_models`). No metadata keys (openai_responses serves none), so the metadata
/// override paths below start from `""`.
const DEF: ModelsShape = ModelsShape {
    path: "/models",
    keys: ModelKeys {
        array_key: "data",
        id_key: "id",
        strip: "",
        context_key: "",
        max_output_key: "",
        display_name_key: "",
    },
};

#[test]
fn no_override_is_the_plain_protocol_default() {
    // Absent `[provider.models]` (§3.2): `{base_url}{path}`, no `?`, protocol keys —
    // including the metadata keys, which stay at the protocol default (`""` here).
    let r = models_req(DEF, None, "https://api.openai.com/v1");
    assert_eq!(r.url, "https://api.openai.com/v1/models");
    assert_eq!(
        (r.keys.array_key, r.keys.id_key, r.keys.strip),
        ("data", "id", "")
    );
    assert_eq!(r.keys.context_key, "");
    assert_eq!(r.keys.max_output_key, "");
    assert_eq!(r.keys.display_name_key, "");
}

#[test]
fn a_full_override_replaces_path_query_and_keys() {
    // The Codex shape: path + a `?client_version` query (the SPACE proves the reused
    // OAuth `encode_pairs` codec percent-encodes) + `models`/`slug` keys, AND a
    // row-named `context_key` lifting the list's `context_window` metadata (§3.2).
    let over = ModelsOverride {
        path: Some("/models".into()),
        query: vec![("client_version".into(), "0 0".into())],
        array_key: Some("models".into()),
        id_key: Some("slug".into()),
        context_key: Some("context_window".into()),
        ..Default::default()
    };
    let r = models_req(DEF, Some(&over), "https://chatgpt.com/backend-api/codex");
    assert_eq!(
        r.url,
        "https://chatgpt.com/backend-api/codex/models?client_version=0%200"
    );
    assert_eq!(
        (r.keys.array_key, r.keys.id_key, r.keys.strip),
        ("models", "slug", "")
    );
    assert_eq!(r.keys.context_key, "context_window");
}

#[test]
fn a_partial_override_inherits_keys_and_empty_query_adds_no_q() {
    // Only `path` pinned: array_key/id_key AND the metadata keys INHERIT the protocol
    // default (the inherit rule, §3.2), and an empty `query` appends no `?` (the
    // empty-input general path).
    let over = ModelsOverride {
        path: Some("/v2/models".into()),
        ..Default::default()
    };
    let r = models_req(DEF, Some(&over), "https://x.test");
    assert_eq!(r.url, "https://x.test/v2/models");
    assert_eq!((r.keys.array_key, r.keys.id_key), ("data", "id"));
    assert_eq!(r.keys.display_name_key, "");
}

/// A Codex-like row: `openai_responses` over an oauth-less `none` auth (the GET's
/// auth is orthogonal to this test), with the `[provider.models]` override that pins
/// the path, the version query, and the `models`/`slug` response keys.
const CODEX_CONFIG: &str = r#"
[[provider]]
name = "codex"
base_url = "https://chatgpt.test/backend-api/codex"
protocol = "openai_responses"
auth = "none"

[provider.models]
path = "/models"
query = [["client_version", "0.0.0"]]
array_key = "models"
id_key = "slug"
"#;

#[test]
fn override_row_lists_codex_models_via_the_query_and_slug_shape() {
    // Gap A + Gap B together: the GET targets `{base_url}/models?client_version=0.0.0`
    // (the query the route demands), and the `{"models":[{"slug":…}]}` body decodes
    // via the row's `array_key`/`id_key` to the ordered slugs.
    let cfg = temp(CODEX_CONFIG);
    let path = cfg.0.to_str().unwrap();
    let body = br#"{"models":[
        {"slug":"gpt-5.6-sol","context_window":1},
        {"slug":"gpt-5.4"},
        {"slug":"codex-auto-review"}
    ]}"#;
    let tx = MockTransport::ok(vec![body]);
    let o = go(
        &["--list-models", "--provider", "codex", "--config", path],
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "gpt-5.6-sol\ngpt-5.4\ncodex-auto-review\n");
    let sent = tx.requests();
    assert_eq!(sent[0].method, Method::Get);
    assert_eq!(
        sent[0].url,
        "https://chatgpt.test/backend-api/codex/models?client_version=0.0.0"
    );
}

#[test]
fn a_version_gated_empty_codex_list_is_exit_0_with_a_note() {
    // A stale `client_version` returns a valid empty list, not an error (model-discovery
    // §3.2): exit 0, nothing on stdout (text mode), the honest stderr note. The same
    // override path, proving the empty 200 is honest data even through the override.
    let cfg = temp(CODEX_CONFIG);
    let path = cfg.0.to_str().unwrap();
    let tx = MockTransport::ok(vec![br#"{"models":[]}"#]);
    let o = go(
        &["--list-models", "--provider", "codex", "--config", path],
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "");
    assert!(o.stderr.contains("no models returned for `codex`"));
    // The override path was still taken — the GET carried the query.
    assert_eq!(
        tx.requests()[0].url,
        "https://chatgpt.test/backend-api/codex/models?client_version=0.0.0"
    );
}
