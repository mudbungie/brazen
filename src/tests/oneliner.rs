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
//! 1. **api-key**, already near-zero-config: a `claude-…` model routes to the
//!    built-in `anthropic` row with NO `--provider` and NO `--config`, and the row's
//!    `ANTHROPIC_API_KEY` ambient source (auth §5.5, bl-5a43) is discovered on the
//!    store miss into the `x-api-key` header, carrying none of the OAuth artifacts.
//!    (The env var → ApiKey read is the shim's `discover`; here the discovered cred is
//!    modeled with `with_ambient`, the same double the OAuth case uses.)
//! 2. **Claude-Code OAuth**, the credential actually on the box: an operator
//!    pastes the `auth = "oauth2"` recipe ONCE (Anthropic OAuth ships as a recipe,
//!    NOT a built-in row — baking one vendor's login policy into the binary is
//!    refused on purpose; auth.md §7, architecture.md §13 item 3). With an
//!    `ambient` block the run then needs NO `--api-key` and NO `bz --login`: it
//!    discovers the Claude Code token, and the bearer header, the
//!    `anthropic-beta: oauth-2025-04-20` header, and the required Claude-Code
//!    system preamble ALL come from the recipe + cred — none typed on the CLI.

use crate::testing::MemoryCredStore;
use crate::tests::run_support::*;
use crate::{Cred, Method, Secret};

const HAIKU: &str = "claude-haiku-4-5-20251001";
const PREAMBLE: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

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
    // `bz --model claude-… "question"` — the ANTHROPIC_API_KEY is the ONLY setup. The
    // model prefix routes to the built-in `anthropic` row (bl-72dc), whose ambient env
    // source is discovered on the store miss (bl-5a43): no inline key, no `bz --login`.
    // The shim's `discover` reads the env var into an ApiKey; here that discovered cred
    // is modeled with `with_ambient`. The api-key path writes `x-api-key` and carries
    // NONE of the OAuth artifacts (no bearer beta header, no Claude-Code preamble):
    // those are auth-mode-dependent and an api-key row pins neither.
    let store = MemoryCredStore::with_ambient(Cred::ApiKey {
        key: Secret::new("sk-ant-api-xyz"),
    });
    let tx = ok_basic();
    // Options precede the positional prompt (§5.5/§13.7); the prompt is last.
    let o = go(&["--model", HAIKU, "question"], &[], b"", &tx, &store);
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
    // The Claude-Code one-liner: NO `--api-key`, NO `--system`, NO `bz --login`. The
    // ambient Claude Code token is discovered on the store miss (bl-8058), and the
    // recipe alone supplies the bearer scheme, the `oauth-2025-04-20` beta header,
    // and the leading Claude-Code system block. A far-future `expires_at` keeps the
    // cred fresh, so no refresh POST is sent. The recipe row claims no
    // `model_prefixes` so it opts OUT of fuzzy matching: the EXPLICIT full id is taken
    // LITERALLY (model-discovery §5.1, the bl-3989 fix), so NO models-list probe fires
    // — the generation POST is the ONLY round-trip, riding the discovered OAuth seam.
    let cfg = temp(OAUTH_RECIPE);
    let store = MemoryCredStore::with_ambient(Cred::OAuth2 {
        access_token: Secret::new("at-claude-code"),
        refresh_token: Secret::new("rt-claude-code"),
        expires_at: 4_000_000_000,
        scope: None,
        account_id: None,
    });
    let tx = ok_basic();
    let o = go(
        &[
            "--provider",
            "anthropic-oauth",
            "--model",
            HAIKU,
            // Prompt last: options must precede the positional (§5.5/§13.7).
            "question",
        ],
        &[("BRAZEN_CONFIG", cfg.0.to_str().unwrap())],
        b"",
        &tx,
        &store,
    );
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "Hello");

    let reqs = tx.requests();
    // Exactly one round-trip: the prefix-less row + present model takes the literal id,
    // so there is no probe GET — only the generation POST.
    assert_eq!(
        reqs.len(),
        1,
        "no probe — the literal path is one round-trip"
    );
    let req = &reqs[0];
    assert_eq!(req.method, Method::Post);
    let body_carries_id = String::from_utf8_lossy(&req.body).contains(HAIKU);
    assert!(body_carries_id, "the POST carries the literal model id");
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
