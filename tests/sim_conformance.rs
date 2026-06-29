//! End-to-end conformance against a SIMULATED provider HTTP server (bl-7d5d).
//!
//! For each provider, a [`FakeProvider`] replays that provider's golden basic
//! fixture; a temp `--config` points the provider's `base_url` at it; and the real
//! `bz` binary is driven against it. This exercises the REAL `HttpTransport` (the
//! `ureq` round-trip) plus the whole encode → auth → send → frame → decode → project
//! pipeline end to end — the one path the in-process `MockTransport` cannot cover —
//! and asserts the NORMALIZED canonical event grammar, the surface that is identical
//! across every provider. No real provider, no key; runs in plain `cargo test`.

#[allow(dead_code)]
#[path = "live_support/exec.rs"]
mod exec;
#[allow(dead_code)]
#[path = "live_support/grammar.rs"]
mod grammar;
mod sim_support;

use sim_support::FakeProvider;

/// One simulated provider: its registry shape (as a `--config` row) and the golden
/// fixture the fake server replays.
struct Sim {
    /// Provider row name + the `--provider` value.
    name: &'static str,
    protocol: &'static str,
    auth: &'static str,
    /// The full `api_header = { … }` TOML line, or `""` for keyless (`auth = "none"`).
    api_header: &'static str,
    /// Extra row lines (e.g. `body_defaults` / `beta_headers`), or `""`.
    extra: &'static str,
    model: &'static str,
    fixture: &'static str,
    content_type: &'static str,
}

const PROVIDERS: &[Sim] = &[
    Sim {
        name: "anthropic",
        protocol: "anthropic_messages",
        auth: "api_key",
        api_header: r#"api_header = { name = "x-api-key", scheme = "raw" }"#,
        extra: "body_defaults = { max_tokens = 4096 }",
        model: "claude-sim",
        fixture: "anthropic_messages_basic.sse",
        content_type: "text/event-stream",
    },
    Sim {
        name: "openai",
        protocol: "openai_chat",
        auth: "bearer",
        api_header: r#"api_header = { name = "Authorization", scheme = "bearer" }"#,
        extra: "",
        model: "gpt-sim",
        fixture: "openai_chat_basic.sse",
        content_type: "text/event-stream",
    },
    Sim {
        name: "openai-responses",
        protocol: "openai_responses",
        auth: "bearer",
        api_header: r#"api_header = { name = "Authorization", scheme = "bearer" }"#,
        extra: "",
        model: "gpt-sim",
        fixture: "openai_responses_basic.sse",
        content_type: "text/event-stream",
    },
    Sim {
        name: "google",
        protocol: "google_generative_ai",
        auth: "api_key",
        api_header: r#"api_header = { name = "x-goog-api-key", scheme = "raw" }"#,
        extra: "",
        model: "gemini-sim",
        fixture: "google_genai_basic.sse",
        content_type: "text/event-stream",
    },
    Sim {
        name: "ollama",
        protocol: "ollama_chat",
        auth: "none",
        api_header: "",
        extra: "",
        model: "llama-sim",
        fixture: "ollama_chat_basic.ndjson",
        content_type: "application/x-ndjson",
    },
];

/// Read a golden fixture from `tests/fixtures/`.
fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("read fixture {path}: {e}"))
}

/// Write a single-provider config whose `base_url` targets `server`, returning the
/// kept-alive temp file (dropping it deletes the file).
fn config_for(p: &Sim, base_url: &str) -> tempfile::NamedTempFile {
    use std::io::Write;
    let body = format!(
        "[[provider]]\nname = \"{}\"\nbase_url = \"{}\"\nprotocol = \"{}\"\nauth = \"{}\"\n{}\n{}\n",
        p.name, base_url, p.protocol, p.auth, p.api_header, p.extra
    );
    let mut f = tempfile::NamedTempFile::new().expect("temp config");
    f.write_all(body.as_bytes()).expect("write config");
    f.flush().expect("flush config");
    f
}

/// `bz --json …` args targeting `server` for provider `p` (a positional prompt; the
/// fake server ignores it and replays its fixture). The prompt is appended LAST:
/// option parsing stops at the first operand, so every flag must precede it (§5.5).
fn args_for(p: &Sim, cfg_path: String) -> Vec<String> {
    let mut args = vec![
        "--config".into(),
        cfg_path,
        "--provider".into(),
        p.name.into(),
        "--model".into(),
        p.model.into(),
        "--json".into(),
    ];
    if p.auth != "none" {
        args.push("--api-key".into());
        args.push("sk-sim-dummy".into());
    }
    args.push("say hi".into());
    args
}

/// Drive `bz` against a fake server replaying `p`'s fixture, returning the parsed
/// `--json` canonical event stream (asserting a clean exit-0 over real HTTP first).
fn drive(p: &Sim) -> Vec<serde_json::Value> {
    let server = FakeProvider::serve(p.content_type, fixture(p.fixture));
    let cfg = config_for(p, &server.base_url());
    let args = args_for(p, cfg.path().to_string_lossy().into_owned());
    let (code, out, err) = exec::run_bz(&args, "");
    assert_eq!(
        code, 0,
        "{}: bz exited {code} over the real transport (stderr: {err})\nstdout: {out}",
        p.name
    );
    grammar::events(&out).unwrap_or_else(|e| panic!("{}: {e}", p.name))
}

#[test]
fn every_provider_decodes_the_canonical_text_grammar_over_real_http() {
    for p in PROVIDERS {
        let evs = drive(p);
        let ctx = |what: &str| format!("{}: {what}", p.name);

        grammar::want(&evs, &ctx("message_start"), |e| {
            grammar::ty(e) == "message_start"
        })
        .unwrap();
        grammar::want(&evs, &ctx("text content_start"), |e| {
            grammar::kind_has(e, "text")
        })
        .unwrap();
        grammar::want(&evs, &ctx("text_delta"), |e| {
            grammar::delta_has(e, "text_delta")
        })
        .unwrap();
        grammar::last_is(&evs, "end").unwrap_or_else(|e| panic!("{}", ctx(&e)));
    }
}

/// A non-2xx response must map to the right exit code through the REAL transport:
/// the status is read off the actual HTTP response line (not a mock), so this pins
/// the transport's status-peek + `ErrorKind::from_http_status` end to end. A `401`
/// is `Auth` → exit **77**; the provider's error body is surfaced, not swallowed.
#[test]
fn an_http_401_maps_to_exit_77_over_real_http() {
    let openai = &PROVIDERS[1];
    assert_eq!(openai.name, "openai");
    let body = fixture("openai_error_401.json");
    let server = FakeProvider::serve_status(401, "application/json", body);
    let cfg = config_for(openai, &server.base_url());
    let args = args_for(openai, cfg.path().to_string_lossy().into_owned());

    let (code, out, err) = exec::run_bz(&args, "");
    assert_eq!(
        code, 77,
        "401 should exit 77 (Auth); stderr: {err}\nstdout: {out}"
    );
    assert!(
        err.contains("invalid_api_key") || out.contains("invalid_api_key"),
        "the provider's 401 body should be surfaced; stderr: {err}\nstdout: {out}"
    );
}
