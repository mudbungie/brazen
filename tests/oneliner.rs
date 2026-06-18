//! The composed ergonomic one-liner, end-to-end through `run` (bl-ce84). The
//! children each deliver one gap — model→provider routing (bl-72dc), the
//! auth-mode system preamble (bl-b3d9), credential discovery (bl-8058), and the
//! operator OAuth recipe (bl-2485) — and this is the integration lock that proves
//! they COMPOSE into the README north star: "pipe in a question; it speaks the
//! answer," with no hand-rolled provider config to reach a model.
//!
//! Two faces of "just works", both driven through the real `run` seam with zero
//! network (`MockTransport` captures the wire request encode+auth produced):
//!
//! 1. **api-key**, already near-zero-config: a `claude-…` model with an
//!    `ANTHROPIC_API_KEY` in the env routes to the built-in `anthropic` row with
//!    NO `--provider` and NO `--config`, and carries none of the OAuth artifacts.
//! 2. **Claude-Code OAuth**, the credential actually on the box: an operator
//!    pastes the `auth = "oauth2"` recipe ONCE (Anthropic OAuth ships as a recipe,
//!    NOT a built-in row — baking one vendor's login policy into the binary is
//!    refused on purpose; auth.md §7, architecture.md §13 item 3). With an
//!    `ambient` block the run then needs NO `--api-key` and NO `bz login`: it
//!    discovers the Claude Code token, and the bearer header, the
//!    `anthropic-beta: oauth-2025-04-20` header, and the required Claude-Code
//!    system preamble ALL come from the recipe + cred — none typed on the CLI.

mod run_support;

use brazen::testing::{MemoryCredStore, ScriptedTransport};
use brazen::{Cred, Method, Secret};
use run_support::*;

const HAIKU: &str = "claude-haiku-4-5-20251001";
const PREAMBLE: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

/// An anthropic `/v1/models` body listing the HAIKU id (model-discovery §3.1) — the
/// probe's response, so the recipe row (which owns NO `model_prefixes`, reached via
/// `--provider`) expands the full id to itself (exact match, §4) before generation.
const MODELS: &[u8] =
    br#"{"data":[{"type":"model","id":"claude-haiku-4-5-20251001"}],"has_more":false}"#;

/// The operator OAuth recipe (auth.md §7, README "Sign in with ChatGPT" shape):
/// a user-config `oauth2` row, NOT a `defaults.toml` row. `ambient` opts the row
/// into zero-setup discovery; the `oauth` block's `beta_headers`/`system_preamble`
/// are the auth-mode-dependent header/body facts applied without a CLI flag.
const OAUTH_RECIPE: &str = r#"
[[provider]]
name = "anthropic-oauth"
base_url = "https://api.anthropic.com"
protocol = "anthropic_messages"
auth = "oauth2"
api_header = { name = "Authorization", scheme = "bearer" }
beta_headers = [["anthropic-version", "2023-06-01"]]
body_defaults = { max_tokens = 4096 }
ambient = { format = "claude_code", path = "~/.claude/.credentials.json" }

[provider.oauth]
authorize_url = "https://claude.ai/oauth/authorize"
token_url = "https://console.anthropic.com/v1/oauth/token"
client_id = "9d1c250a-e61b-44d9-88ed-5944d1962f5e"
scope = "user:inference user:profile"
beta_headers = [["anthropic-beta", "oauth-2025-04-20"]]
system_preamble = "You are Claude Code, Anthropic's official CLI for Claude."
"#;

#[test]
fn api_key_oneliner_routes_by_model_with_no_provider_or_config() {
    // `bz --model claude-… "question"` — the env key is the ONLY setup. The model
    // prefix routes to the built-in `anthropic` row (bl-72dc), and the api-key path
    // writes `x-api-key` and carries NONE of the OAuth artifacts (no bearer beta
    // header, no Claude-Code preamble): the preamble/beta are auth-mode-dependent
    // and an api-key row pins neither.
    let tx = ok_basic();
    let o = go(
        &["question", "--model", HAIKU],
        &[("ANTHROPIC_API_KEY", "sk-ant-api-xyz")],
        b"",
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "Hello");

    let reqs = tx.requests();
    let req = &reqs[0];
    assert_eq!(req.header("x-api-key"), Some("sk-ant-api-xyz"));
    assert_eq!(req.header("anthropic-version"), Some("2023-06-01"));
    assert_eq!(req.header("anthropic-beta"), None);
    assert_eq!(req.header("Authorization"), None);
    let body = String::from_utf8_lossy(&req.body);
    assert!(
        !body.contains(PREAMBLE),
        "api-key path must inject no preamble"
    );
}

#[test]
fn oauth_oneliner_discovers_ambient_cred_and_injects_bearer_beta_and_preamble() {
    // The Claude-Code one-liner: NO `--api-key`, NO `--system`, NO `bz login`. The
    // ambient Claude Code token is discovered on the store miss (bl-8058), and the
    // recipe alone supplies the bearer scheme, the `oauth-2025-04-20` beta header,
    // and the leading Claude-Code system block. A far-future `expires_at` keeps the
    // cred fresh, so no refresh POST is sent. The recipe row claims no
    // `model_prefixes` (reached via `--provider`), so the full id is NOT prefix-owned
    // and `serve` prepends ONE models-list probe (model-discovery §5.2) that expands
    // it to itself — the probe GET and the generation POST both ride the SAME OAuth
    // auth seam (the discovered bearer + beta), proving discovery composes with the probe.
    let cfg = temp(OAUTH_RECIPE);
    let store = MemoryCredStore::with_ambient(Cred::OAuth2 {
        access_token: Secret::new("at-claude-code"),
        refresh_token: Secret::new("rt-claude-code"),
        expires_at: 4_000_000_000,
        scope: None,
        account_id: None,
    });
    let tx = ScriptedTransport::new(vec![(200, MODELS.to_vec()), (200, BASIC.to_vec())]);
    let o = go(
        &[
            "question",
            "--provider",
            "anthropic-oauth",
            "--model",
            HAIKU,
        ],
        &[("BRAZEN_CONFIG", cfg.0.to_str().unwrap())],
        b"",
        &tx,
        &store,
    );
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "Hello");

    let reqs = tx.requests();
    // Send #1 is the probe GET to the models endpoint, carrying the discovered bearer
    // + the auth-mode beta header (the same `Auth::apply` seam the generation uses).
    let probe = &reqs[0];
    assert_eq!(probe.method, Method::Get);
    assert_eq!(probe.url, "https://api.anthropic.com/v1/models");
    assert_eq!(probe.header("Authorization"), Some("Bearer at-claude-code"));
    assert_eq!(probe.header("anthropic-beta"), Some("oauth-2025-04-20"));
    // Send #2 is the generation POST.
    let req = &reqs[1];
    assert_eq!(req.method, Method::Post);
    // Bearer from the discovered cred; both beta headers; the api-key header is gone.
    assert_eq!(req.header("Authorization"), Some("Bearer at-claude-code"));
    assert_eq!(req.header("anthropic-beta"), Some("oauth-2025-04-20"));
    assert_eq!(req.header("anthropic-version"), Some("2023-06-01"));
    assert_eq!(req.header("x-api-key"), None);
    // The system LEADS with the Claude-Code preamble — the only system block, so it
    // is necessarily first; sourced from the recipe, never typed on the CLI.
    let body = String::from_utf8_lossy(&req.body);
    assert!(
        body.contains(&format!(
            "\"system\":[{{\"text\":\"{PREAMBLE}\",\"type\":\"text\"}}]"
        )),
        "system must lead with the Claude-Code preamble; body was: {body}"
    );
}
