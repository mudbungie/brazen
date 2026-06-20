//! Live conformance harness (bl-04dc). Drives the REAL `bz` binary against the
//! providers this box has working auth for, asserting the NORMALIZED canonical
//! surface (one canonical request → the same event grammar across every provider).
//! Pure black-box: spawn `bz`, feed a canonical request on stdin, parse the
//! `--json` event stream — no lib linkage, so it lives in the coverage-excluded
//! `bz/` crate (Makefile `cov` ignores `bz/`) and never runs in `make check`. The
//! opt-in gate + the data table live in `live_conformance.rs`.
//!
//! Why a LIVE suite when every other test is offline: `MockTransport` ignores the
//! request URL and the network, so a whole class of wire defects passes offline.
//! This suite caught two (`--raw` sends an empty URL → bl-080b; `--stream` never
//! reaches the wire → bl-5363) that 100%-covered offline tests could not.

mod exec;
mod grammar;

use serde_json::{json, Value};

use exec::{connectable, cred_file, run_bz};
use grammar::{delta_has, events, kind_has, last_is, ty, want};

/// The one-word prompt every provider answers; the assertion is grammar, not text.
const PROMPT: &str = "reply with the single word: ok";
/// A tool-eliciting prompt for the round-trip assertion (rows that opt in).
const TOOL_PROMPT: &str = "What is the weather in Paris? Call the get_weather tool.";
/// Non-empty `system` — covers the system/instructions surface AND satisfies the
/// codex backend, which 400s without instructions (bl-04dc live finding).
const SYSTEM: &str = "You are a helpful, terse assistant. Use any provided tools when relevant.";
/// A model no provider has: the deliberate bad request for the error-mapping check.
const BAD_MODEL: &str = "brazen-live-no-such-model-zzz";
/// A bad request maps to a 4xx provider status → `Unavailable` → 69. The error
/// BODY may be empty for some providers (the known error-body gap), so the
/// assertion is the exit CODE, never the text.
const ERR_EXIT: i32 = 69;

/// How a provider row proves it can authenticate on THIS machine — DATA, not code.
pub enum Auth {
    /// Keyless (`auth = "none"`, e.g. Ollama): usable iff `probe` (host:port)
    /// answers a TCP connect. No credential is sent.
    Keyless { probe: &'static str },
    /// Keyed: usable iff a `Cred` is stored for this provider (an OAuth2 login),
    /// OR one of `env` holds an API key (passed via `--api-key`). OAuth2 rows list
    /// no `env`; a stored cred is `bz`'s to read, so no key is passed.
    Keyed { env: &'static [&'static str] },
}

/// One provider under test. Every quirk is a per-row datum (one canonical request,
/// normalized behavior, knobs as data — never branches).
pub struct Row {
    /// `--provider` name (must exist in the resolved config / defaults).
    pub provider: &'static str,
    /// Default model; overridden by `model_env` so a box can point at a pulled one.
    pub model: &'static str,
    /// Env var that overrides `model` on this machine.
    pub model_env: &'static str,
    /// Credential-discovery strategy.
    pub auth: Auth,
    /// `--max-tokens` to pass, or `None` to OMIT it — the codex backend rejects
    /// `max_output_tokens`, so `openai-chatgpt` carries `None` (bl-04dc finding).
    pub max_tokens: Option<u32>,
    /// Send explicit `store: false` (folds through the request `extra` passthrough).
    /// The codex backend REQUIRES it; other providers do not want it.
    pub store_false: bool,
    /// Run the tool round-trip assertion (rows whose model reliably tool-calls).
    pub tools: bool,
}

impl Row {
    /// The model to drive: `$<model_env>` if set and non-empty, else the default.
    fn model(&self) -> String {
        std::env::var(self.model_env)
            .ok()
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| self.model.to_owned())
    }

    /// Discover whether this provider can run here. `Ok(key)` runs (with an optional
    /// `--api-key`); `Err(reason)` is a SKIP whose reason is printed (no silent
    /// truncation — AGENTS.md).
    fn detect(&self) -> Result<Option<String>, String> {
        match &self.auth {
            Auth::Keyless { probe } => connectable(probe)
                .then_some(None)
                .ok_or_else(|| format!("keyless {probe} unreachable")),
            Auth::Keyed { env } => {
                if cred_file(self.provider).is_some() {
                    return Ok(None); // a stored Cred — bz reads it, no key passed
                }
                for var in *env {
                    if let Ok(v) = std::env::var(var) {
                        if !v.is_empty() {
                            return Ok(Some(v));
                        }
                    }
                }
                Err(format!(
                    "no stored cred and none of [{}] set",
                    env.join(", ")
                ))
            }
        }
    }

    /// The canonical request JSON on stdin. Always carries `system` (instructions)
    /// and `stream:true` — the `--stream` flag is a no-op (bl-5363), so streaming is
    /// requested in the body. `store:false` and the tool are per-row.
    fn request(&self, with_tool: bool) -> String {
        let mut req = serde_json::Map::new();
        req.insert("stream".into(), json!(true));
        if self.store_false {
            req.insert("store".into(), json!(false));
        }
        req.insert("system".into(), json!([{ "type": "text", "text": SYSTEM }]));
        let text = if with_tool { TOOL_PROMPT } else { PROMPT };
        req.insert(
            "messages".into(),
            json!([{ "role": "user", "content": [{ "type": "text", "text": text }] }]),
        );
        if with_tool {
            req.insert(
                "tools".into(),
                json!([{
                    "name": "get_weather",
                    "description": "Get the weather for a city",
                    "input_schema": {
                        "type": "object",
                        "properties": { "city": { "type": "string" } },
                        "required": ["city"]
                    }
                }]),
            );
        }
        Value::Object(req).to_string()
    }

    /// Base argv shared by every assertion: provider + model (+ `--max-tokens` when
    /// the row carries one, + `--api-key` when discovery handed one back).
    fn base_args(&self, model: &str, key: Option<&str>) -> Vec<String> {
        let mut a = vec![
            "--provider".into(),
            self.provider.into(),
            "--model".into(),
            model.into(),
        ];
        if let Some(n) = self.max_tokens {
            a.push("--max-tokens".into());
            a.push(n.to_string());
        }
        if let Some(k) = key {
            a.push("--api-key".into());
            a.push(k.into());
        }
        a
    }

    /// Run every applicable assertion; print one status line each. Returns the
    /// provider-qualified failure messages (empty = all green).
    pub fn conform(&self, key: Option<&str>) -> Vec<String> {
        let model = self.model();
        let mut fails = Vec::new();
        let mut check = |label: &str, r: Result<(), String>| match r {
            Ok(()) => println!("  {label:<18} ok"),
            Err(e) => {
                println!("  {label:<18} FAIL: {e}");
                fails.push(format!("{}/{label}: {e}", self.provider));
            }
        };
        check("json/streamed", self.assert_stream(&model, key));
        check("text-projection", self.assert_text(&model, key));
        check("error-mapping", self.assert_error(key));
        if self.tools {
            check("tool-round-trip", self.assert_tools(&model, key));
        } else {
            println!(
                "  {:<18} skipped (row: tools unsupported)",
                "tool-round-trip"
            );
        }
        // raw projection: KNOWN GAP — the data-plane raw path sends an empty URL
        // (bl-080b), so a live raw run is a transport error regardless. Skipped
        // loudly rather than asserted; flip to a real check once bl-080b lands.
        println!(
            "  {:<18} skipped (known gap bl-080b: raw sends empty URL)",
            "raw-projection"
        );
        fails
    }

    /// Streamed text over `--json`: the canonical grammar — `message_start` first,
    /// a text `content_start`, ≥1 `text_delta`, a `usage`, a `finish`, `end` last.
    fn assert_stream(&self, model: &str, key: Option<&str>) -> Result<(), String> {
        let mut args = self.base_args(model, key);
        args.push("--json".into());
        let (code, out, err) = run_bz(&args, &self.request(false));
        if code != 0 {
            return Err(format!("exit {code} (want 0); stderr: {}", err.trim()));
        }
        let evs = events(&out)?;
        want(&evs, "message_start", |e| {
            ty(e) == "message_start" && e.get("role").is_some()
        })?;
        want(&evs, "text content_start", |e| kind_has(e, "text"))?;
        want(&evs, "text_delta", |e| delta_has(e, "text_delta"))?;
        want(&evs, "usage", |e| ty(e) == "usage")?;
        want(&evs, "finish", |e| ty(e) == "finish")?;
        last_is(&evs, "end")
    }

    /// `--text` projection: the human stream collapses to bare text — exit 0 and a
    /// non-empty body (the assistant's reply).
    fn assert_text(&self, model: &str, key: Option<&str>) -> Result<(), String> {
        let mut args = self.base_args(model, key);
        args.push("--text".into());
        let (code, out, err) = run_bz(&args, &self.request(false));
        if code != 0 {
            return Err(format!("exit {code} (want 0); stderr: {}", err.trim()));
        }
        if out.trim().is_empty() {
            return Err("empty --text output".into());
        }
        Ok(())
    }

    /// Error mapping: a bad model is a deliberately-bad request → a 4xx provider
    /// status → exit 69. Asserted on the exit CODE, not the body (the error body
    /// may be empty — the known gap).
    fn assert_error(&self, key: Option<&str>) -> Result<(), String> {
        let mut args = self.base_args(BAD_MODEL, key);
        args.push("--json".into());
        let (code, _out, _err) = run_bz(&args, &self.request(false));
        if code == ERR_EXIT {
            Ok(())
        } else {
            Err(format!("exit {code} for a bad request (want {ERR_EXIT})"))
        }
    }

    /// Tool round-trip over `--json`: a tool `content_start` (carrying the tool id +
    /// name) and ≥1 `json_delta` (the streamed arguments).
    fn assert_tools(&self, model: &str, key: Option<&str>) -> Result<(), String> {
        let mut args = self.base_args(model, key);
        args.push("--json".into());
        let (code, out, err) = run_bz(&args, &self.request(true));
        if code != 0 {
            return Err(format!("exit {code} (want 0); stderr: {}", err.trim()));
        }
        let evs = events(&out)?;
        want(&evs, "tool_use content_start", |e| kind_has(e, "tool_use"))?;
        want(&evs, "json_delta", |e| delta_has(e, "json_delta"))?;
        Ok(())
    }
}

/// Detect + print one row's RUN/SKIP line; `Some(key)` means run with that key.
pub fn announce(row: &Row) -> Option<Option<String>> {
    match row.detect() {
        Ok(key) => {
            let how = if key.is_some() {
                "env api-key"
            } else {
                "stored cred / keyless"
            };
            println!("== {} ==  RUN ({how})", row.provider);
            Some(key)
        }
        Err(reason) => {
            println!("== {} ==  SKIP ({reason})", row.provider);
            None
        }
    }
}

/// Opt-in gate: `BRAZEN_LIVE` set and non-empty (mirrors the smoke tests' gate).
pub fn live_enabled() -> bool {
    std::env::var("BRAZEN_LIVE")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}
