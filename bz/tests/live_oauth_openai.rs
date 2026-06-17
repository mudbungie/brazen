//! Live, opt-in test of the openai-chatgpt (codex) OAuth **auth circuit** (bl-0272)
//! — the half bl-b72f scoped ("near-expiry -> silent refresh; revoked -> exit 77")
//! but left uncovered because it needs deliberate token manipulation, not request
//! fuzzing. Three brazen circuits (not OpenAI's), each driving the REAL `bz` binary:
//!
//!   * `revoked-access`  — a fresh-expiry cred with a bad ACCESS token: brazen skips
//!     refresh and sends the bad bearer -> codex 401 -> `from_http_status(401)=Auth`
//!     -> exit 77 (the mapping bl-b72f's all-400 matrix left UNTESTED live).
//!   * `revoked-refresh` — an expired cred with a bad REFRESH token: brazen refreshes
//!     -> token endpoint `invalid_grant` -> `auth_error` -> exit 77.
//!   * `silent-refresh`  — an expired cred with the REAL refresh token: brazen mints a
//!     new access token over the token endpoint, persists it, and completes 200.
//!
//! Safety: the two REVOKED circuits run against a THROWAWAY temp `XDG_DATA_HOME` with
//! SYNTHETIC tokens — the real refresh token is never sent. `silent-refresh` MUST send
//! the real refresh token (OpenAI rotates it on use, auth.md §10.3 line 538), so it
//! forces refresh on the REAL store and KEEPS brazen's persisted result — a normal
//! early refresh — restoring the backup only if brazen never minted a fresh token. It
//! also GENERATES, so it sits behind the second opt-in `BRAZEN_LIVE_FUZZ_SPEND=1`.
//!
//! `#[ignore]`d AND `BRAZEN_LIVE`-gated; SKIPS (printed) without a stored cred.
//!
//! ```text
//! BRAZEN_LIVE=1 [BRAZEN_LIVE_FUZZ_SPEND=1] \
//!   cargo test -p bz --test live_oauth_openai -- --ignored --nocapture
//! ```

#[allow(dead_code)] // `connectable`/`run_bz` unused here; this suite drives `run_bz_in`.
#[path = "live_support/exec.rs"]
mod exec;
#[path = "live_support/grammar.rs"]
mod grammar;

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use exec::{cred_file, run_bz_in};
use grammar::{delta_has, events, kind_has, last_is, ty, want};

const PROVIDER: &str = "openai-chatgpt";
const MODEL: &str = "gpt-5.4";
const MODEL_ENV: &str = "BRAZEN_LIVE_OPENAI_CHATGPT_MODEL";
const SYSTEM: &str = "You are a terse assistant. Reply with exactly one word when asked.";
const PROMPT: &str = "reply with the single word: ok";
/// A 401/403 auth failure -> `ErrorKind::Auth` -> `ExitClass::NoPerm` (canonical/error.rs).
const AUTH_EXIT: i32 = 77;
/// A clearly-invalid token: forces the codex data plane to 401 and the token endpoint
/// to `invalid_grant`. Never the real credential.
const BAD: &str = "brazen-live-bl-0272-revoked";

/// An env flag is "on" iff set and non-empty (dual of the smoke / fuzz gates).
fn flag(name: &str) -> bool {
    std::env::var(name).map(|v| !v.is_empty()).unwrap_or(false)
}

/// The model to drive: `$BRAZEN_LIVE_OPENAI_CHATGPT_MODEL` if set, else `gpt-5.4`.
fn model() -> String {
    std::env::var(MODEL_ENV)
        .ok()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| MODEL.to_owned())
}

/// Wall-clock unix seconds — only to set/check absolute `expires_at` in the cred JSON.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_secs()
}

/// The fully-valid codex request body: stream + `store:false` + instructions (the
/// codex backend 400s without each — bl-04dc), so any non-2xx here is the AUTH fault.
fn valid_body() -> String {
    let mut m = Map::new();
    m.insert("stream".into(), json!(true));
    m.insert("store".into(), json!(false));
    m.insert("system".into(), json!([{ "type": "text", "text": SYSTEM }]));
    m.insert(
        "messages".into(),
        json!([{ "role": "user", "content": [{ "type": "text", "text": PROMPT }] }]),
    );
    Value::Object(m).to_string()
}

/// Argv: provider + model + `--json`. NO `--api-key` — the OAuth2 stored cred is
/// `bz`'s to read (here from whichever `XDG_DATA_HOME` the run is pointed at).
fn args() -> Vec<String> {
    vec![
        "--provider".into(),
        PROVIDER.into(),
        "--model".into(),
        model(),
        "--json".into(),
    ]
}

/// A synthetic `Cred::OAuth2` JSON — never carries the real refresh token.
fn synthetic_cred(access: &str, refresh: &str, expires_at: u64) -> Value {
    json!({ "OAuth2": {
        "access_token": access,
        "refresh_token": refresh,
        "expires_at": expires_at,
        "scope": "openid",
        "account_id": "bl-0272-synthetic",
    } })
}

/// Write `cred` to `<dir>/brazen/credentials/<provider>.json` — the `XdgCredStore` path.
fn plant(dir: &Path, cred: &Value) {
    let creds = dir.join("brazen").join("credentials");
    fs::create_dir_all(&creds).expect("mkdir credentials");
    fs::write(creds.join(format!("{PROVIDER}.json")), cred.to_string()).expect("write cred");
}

/// Print + return a provider-qualified failure. `None` = green.
fn fail(label: &str, m: &str) -> Option<String> {
    println!("  {label:<22} FAIL: {m}");
    Some(format!("{PROVIDER}/{label}: {m}"))
}

/// One revoked circuit: a synthetic cred in a THROWAWAY temp store that must drive
/// `bz` to exit 77. Near-free — the data plane / token endpoint rejects before any
/// generation. The real credential store is untouched.
fn check_revoked(label: &str, cred: &Value) -> Option<String> {
    let tmp = tempfile::tempdir().expect("tempdir");
    plant(tmp.path(), cred);
    let env = [("XDG_DATA_HOME", tmp.path().to_str().expect("utf8 tmp path"))];
    let (code, _out, err) = run_bz_in(&args(), &valid_body(), &env);
    if code != AUTH_EXIT {
        return fail(
            label,
            &format!("exit {code} (want {AUTH_EXIT}); {}", err.trim()),
        );
    }
    println!("  {label:<22} ok (exit {AUTH_EXIT})");
    None
}

/// The canonical 2xx grammar a streamed codex completion must decode to.
fn grammar_ok(out: &str) -> Result<(), String> {
    let evs = events(out)?;
    want(&evs, "message_start", |e| ty(e) == "message_start")?;
    want(&evs, "text content_start", |e| kind_has(e, "text"))?;
    want(&evs, "text_delta", |e| delta_has(e, "text_delta"))?;
    want(&evs, "finish", |e| ty(e) == "finish")?;
    last_is(&evs, "end")
}

/// Silent refresh on the REAL store (auth §6): force expiry, let brazen mint+persist a
/// fresh token over the token endpoint and complete 200. KEEP brazen's persisted result
/// (the rotated token is now the valid one — a normal early refresh); restore the backup
/// only if brazen minted NOTHING (refresh failed → the original token was never rotated).
fn check_silent_refresh(real: &Path) -> Option<String> {
    let backup = fs::read(real).expect("read real cred");
    let orig: Value = serde_json::from_slice(&backup).expect("parse cred");
    let old_access = orig["OAuth2"]["access_token"]
        .as_str()
        .unwrap_or("")
        .to_owned();

    let mut expired = orig.clone();
    expired["OAuth2"]["expires_at"] = json!(0);
    fs::write(real, expired.to_string()).expect("write expired cred");

    let (code, out, err) = run_bz_in(&args(), &valid_body(), &[]);

    // Did brazen persist a freshly-minted token (access token changed)? If so it is now
    // the only valid credential — KEEP it. Otherwise restore the untouched original.
    let after = fs::read_to_string(real).unwrap_or_default();
    let fresh = serde_json::from_str::<Value>(&after).ok();
    let new_access = fresh
        .as_ref()
        .and_then(|v| v["OAuth2"]["access_token"].as_str())
        .unwrap_or("");
    let minted = !new_access.is_empty() && new_access != old_access;
    if !minted {
        fs::write(real, &backup).expect("restore cred");
    }

    if code != 0 {
        return fail(
            "silent-refresh",
            &format!("exit {code} (want 0); {}", err.trim()),
        );
    }
    if let Err(e) = grammar_ok(&out) {
        return fail("silent-refresh", &e);
    }
    if !minted {
        return fail(
            "silent-refresh",
            "exit 0 but no freshly-minted token persisted",
        );
    }
    // The codex token endpoint sends NO `expires_in`; expiry comes from the new access
    // token's own JWT `exp` (auth §10.3) — so a future `expires_at` proves that path.
    let new_exp = fresh
        .as_ref()
        .and_then(|v| v["OAuth2"]["expires_at"].as_u64())
        .unwrap_or(0);
    if new_exp <= now() {
        return fail(
            "silent-refresh",
            &format!("persisted expires_at {new_exp} not in the future (jwt_exp path?)"),
        );
    }
    println!("  silent-refresh         ok (exit 0, minted token, expires_at {new_exp})");
    None
}

#[test]
#[ignore = "live: drives the codex OAuth circuit over the network; run with --ignored"]
fn oauth_circuit_openai_chatgpt() {
    if !flag("BRAZEN_LIVE") {
        eprintln!("skipping OpenAI ChatGPT-SSO OAuth circuit: set BRAZEN_LIVE=1 to run it");
        return;
    }
    let Some(real) = cred_file(PROVIDER) else {
        eprintln!(
            "skipping OpenAI ChatGPT-SSO OAuth circuit: no stored `{PROVIDER}` cred — `bz login {PROVIDER}` first"
        );
        return;
    };
    println!("== {PROVIDER} OAuth circuit ==  model {}", model());
    let mut fails: Vec<String> = Vec::new();

    // 1) Revoked -> 77 (near-free; synthetic tokens in a throwaway store — the real
    //    refresh token is never sent). (a) fresh expiry + bad ACCESS token -> codex 401
    //    -> Auth -> 77; (b) expired + bad REFRESH token -> token endpoint invalid_grant
    //    -> auth_error -> 77.
    println!("-- revoked -> 77 (near-free, synthetic temp store) --");
    let far = now() + 86_400;
    for (label, cred) in [
        ("revoked-access", synthetic_cred(BAD, BAD, far)),
        ("revoked-refresh", synthetic_cred(BAD, BAD, 0)),
    ] {
        if let Some(f) = check_revoked(label, &cred) {
            fails.push(f);
        }
    }

    // 2) Silent refresh -> 0. TOKEN-COSTING (generates) AND sends the real refresh token
    //    (which OpenAI rotates), so it is behind the second opt-in and runs on the real
    //    store, keeping the refreshed result.
    let ran_refresh = flag("BRAZEN_LIVE_FUZZ_SPEND");
    if ran_refresh {
        println!("-- silent refresh (real store, token-costing) --");
        if let Some(f) = check_silent_refresh(&real) {
            fails.push(f);
        }
    } else {
        println!(
            "-- silent refresh: SKIPPED (token-costing + rotates the real refresh token; set BRAZEN_LIVE_FUZZ_SPEND=1) --"
        );
    }

    println!(
        "\n2 revoked + {} silent-refresh case(s) exercised; {} failure(s)",
        ran_refresh as u8,
        fails.len()
    );
    assert!(
        fails.is_empty(),
        "openai-chatgpt OAuth circuit failures:\n  {}",
        fails.join("\n  ")
    );
}
