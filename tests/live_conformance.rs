//! Live, opt-in canonical-conformance suite (bl-04dc): one canonical request →
//! the NORMALIZED event grammar, asserted against EVERY provider this box has
//! working auth for. Providers are discovered at runtime (keyless+reachable, a
//! stored `Cred`, or an env API key) and the rest are SKIPPED with a printed
//! reason — never failed (no silent truncation, AGENTS.md).
//!
//! It is `#[ignore]`d (never in CI / the coverage gate) AND env-gated on
//! `BRAZEN_LIVE`, the dual of `ollama_smoke.rs`/`oauth_smoke.rs`. The whole `bz`
//! crate is excluded from the 100% line-coverage gate (Makefile `cov`), so this
//! black-box harness adds no coverage obligation.
//!
//! Run it:
//!
//! ```text
//! BRAZEN_LIVE=1 \
//!   BRAZEN_LIVE_OLLAMA_MODEL=llama3.2 \
//!   OPENAI_API_KEY=sk-… \
//!   cargo test -p brazen --test live_conformance -- --ignored --nocapture
//! ```
//!
//! `--nocapture` surfaces the per-provider RUN/SKIP/assertion lines. Auth comes
//! from whatever is present: a `bz --login --provider <p>` stored cred, an env key below, or a
//! reachable keyless endpoint. See the README "Live conformance suite" section for
//! how to add a provider (it is one `Row` in the table below — quirks are DATA).

mod live_support;

use live_support::{announce, live_enabled, Auth, Row};

/// The per-provider data table. A new provider is ONE row; its quirks are fields,
/// never code branches. Models default to a small/cheap pick and are overridable
/// per box via the named env var (a box rarely has the exact default pulled/enabled).
const TABLE: &[Row] = &[
    // Keyless local Ollama (auth = "none"): discovered by a TCP probe, no key.
    // `bz-smoke:latest` is this box's pulled model; set BRAZEN_LIVE_OLLAMA_MODEL.
    Row {
        provider: "ollama",
        model: "llama3.2",
        model_env: "BRAZEN_LIVE_OLLAMA_MODEL",
        auth: Auth::Keyless {
            probe: "localhost:11434",
        },
        max_tokens: Some(16),
        store_false: false,
        tools: false, // small local models do not reliably tool-call
    },
    // OpenAI "Sign in with ChatGPT" (OAuth2, login-only via `bz --login --provider
    // openai-chatgpt`): discovered by its stored Cred. Codex-backend quirks baked
    // in as DATA (bl-04dc live findings): NO max_tokens (max_output_tokens is
    // rejected), explicit store:false, non-empty instructions (always sent).
    Row {
        provider: "openai-chatgpt",
        model: "gpt-5.4",
        model_env: "BRAZEN_LIVE_OPENAI_CHATGPT_MODEL",
        auth: Auth::Keyed { env: &[] },
        max_tokens: None,
        store_false: true,
        tools: true,
    },
    // Built-in keyed rows (data/defaults.toml). Each runs iff a key is present
    // (stored cred or the listed env var); otherwise SKIP. Models are cheap picks.
    Row {
        provider: "anthropic",
        model: "claude-haiku-4-5-20251001",
        model_env: "BRAZEN_LIVE_ANTHROPIC_MODEL",
        auth: Auth::Keyed {
            env: &["ANTHROPIC_API_KEY", "BRAZEN_API_KEY"],
        },
        max_tokens: Some(16),
        store_false: false,
        tools: false,
    },
    Row {
        provider: "openai",
        model: "gpt-4o-mini",
        model_env: "BRAZEN_LIVE_OPENAI_MODEL",
        auth: Auth::Keyed {
            env: &["OPENAI_API_KEY"],
        },
        max_tokens: Some(16),
        store_false: false,
        tools: false,
    },
    Row {
        provider: "openai-responses",
        model: "gpt-4o-mini",
        model_env: "BRAZEN_LIVE_OPENAI_RESPONSES_MODEL",
        auth: Auth::Keyed {
            env: &["OPENAI_API_KEY"],
        },
        max_tokens: Some(16),
        store_false: false,
        tools: false,
    },
    Row {
        provider: "mistral",
        model: "mistral-small-latest",
        model_env: "BRAZEN_LIVE_MISTRAL_MODEL",
        auth: Auth::Keyed {
            env: &["MISTRAL_API_KEY"],
        },
        max_tokens: Some(16),
        store_false: false,
        tools: false,
    },
    Row {
        provider: "google",
        model: "gemini-1.5-flash",
        model_env: "BRAZEN_LIVE_GOOGLE_MODEL",
        auth: Auth::Keyed {
            env: &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        },
        max_tokens: Some(16),
        store_false: false,
        tools: false,
    },
];

#[test]
#[ignore = "live: touches the network against locally-authed providers; run with --ignored"]
fn canonical_conformance_across_authed_providers() {
    if !live_enabled() {
        eprintln!("skipping live conformance: set BRAZEN_LIVE=1 to run it");
        return;
    }

    let mut ran = 0usize;
    let mut fails: Vec<String> = Vec::new();
    for row in TABLE {
        if let Some(key) = announce(row) {
            ran += 1;
            fails.extend(row.conform(key.as_deref()));
        }
    }

    println!(
        "\n{ran}/{} providers exercised, {} assertion(s) failed",
        TABLE.len(),
        fails.len()
    );
    if ran == 0 {
        // No auth anywhere is a clean no-op, not a failure: a credential-less box
        // must stay green (the suite is opt-in conformance, not a key audit).
        eprintln!("no provider had usable auth on this box — nothing exercised");
        return;
    }
    assert!(
        fails.is_empty(),
        "live conformance failures:\n  {}",
        fails.join("\n  ")
    );
}
