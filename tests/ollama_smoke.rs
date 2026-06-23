//! Live, opt-in Ollama smoke test (bl-76fd) — the ONE check that drives the real
//! `bz` binary over the wire against a real provider on the data plane. Every other
//! `ollama_chat` test is offline (encode/fixture/decode-error, each headed "No
//! network"); they fake the round-trip with `MockTransport`. Ollama is the one
//! provider that runs locally with no API-key cost, so it is the natural home for a
//! live wire-path check — the analogue of `oauth_smoke.rs` (which exercises the real
//! OAuth wire and is likewise `#[ignore]`d + env-gated).
//!
//! It is `#[ignore]`d (so it never runs in CI or the coverage gate) AND gated twice:
//!
//!   * `OLLAMA_SMOKE` must be set (mirrors `oauth_smoke`'s `BZ_SMOKE_*` opt-in), and
//!   * a local Ollama must answer `GET http://localhost:11434/api/version`.
//!
//! A miss on either prints a reason and no-ops, so an accidental `--ignored` run on
//! a box without Ollama is a skip, never a failure.
//!
//! Prereqs:
//!
//!   * `ollama serve` running (the systemd `ollama.service` on this laptop), and
//!   * the model pulled: `ollama pull llama3.2:3b` (override with `OLLAMA_SMOKE_MODEL`).
//!
//! Run it:
//!
//! ```text
//! OLLAMA_SMOKE=1 cargo test -p brazen --test ollama_smoke -- --ignored --nocapture
//! ```
//!
//! NOTE: the `ollama` provider row is `auth = "none"` (keyless), so `bz` needs no
//! credential — no `--api-key` and no `bz login`. A `--api-key` is still accepted and
//! simply ignored, the keyless dual of the keyed rows' "missing key → 77".

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Opt-in gate: `OLLAMA_SMOKE` set and non-empty, else skip.
fn smoke_enabled() -> bool {
    std::env::var("OLLAMA_SMOKE")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

/// The model to drive; small + fast by default, overridable for a different pull.
fn model() -> String {
    std::env::var("OLLAMA_SMOKE_MODEL")
        .ok()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "llama3.2:3b".to_owned())
}

/// Readiness probe: does a local Ollama answer `GET /api/version` with `200`? A raw
/// `TcpStream` keeps the test dependency-free (no HTTP client crate). Any connect /
/// read failure → not ready → skip.
fn ollama_ready() -> bool {
    let addr = "127.0.0.1:11434";
    let Ok(mut sock) = TcpStream::connect(addr) else {
        return false;
    };
    let _ = sock.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = sock.set_write_timeout(Some(Duration::from_secs(5)));
    let req = "GET /api/version HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    if sock.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut resp = String::new();
    if sock.read_to_string(&mut resp).is_err() {
        return false;
    }
    resp.starts_with("HTTP/1.") && resp.contains(" 200")
}

/// Drive a live `bz --json` run against the Ollama row with `stream_flag`
/// (`--stream` or `--no-stream`) and a canonical request on stdin, returning its
/// captured stdout. `None` on a skip (gate unmet / server not ready). No `--api-key`:
/// the `ollama` row is `auth = "none"` (keyless).
fn run_ollama(stream_flag: &str) -> Option<String> {
    if !smoke_enabled() {
        eprintln!("skipping live Ollama smoke: set OLLAMA_SMOKE=1 to run it");
        return None;
    }
    if !ollama_ready() {
        eprintln!(
            "skipping live Ollama smoke: no server answered GET \
             http://localhost:11434/api/version — start `ollama serve`"
        );
        return None;
    }
    let model = model();
    let bz = env!("CARGO_BIN_EXE_bz");
    let request = br#"{"messages":[{"role":"user","content":[{"type":"text","text":"reply with the single word: ok"}]}]}"#;
    let mut child = Command::new(bz)
        .args([
            "--provider",
            "ollama",
            "--model",
            &model,
            "--max-tokens",
            "16",
            stream_flag,
            "--json",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn `bz`");
    child
        .stdin
        .take()
        .expect("bz stdin")
        .write_all(request)
        .expect("write canonical request to bz");
    let out = child.wait_with_output().expect("wait for `bz`");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "live Ollama run ({stream_flag}) failed (exit {:?}); stdout:\n{stdout}",
        out.status.code()
    );
    // Terminal event present → the response framed + decoded to completion (§5.5).
    assert!(
        stdout.trim_end().ends_with(r#"{"type":"end"}"#),
        "expected the canonical stream to end in {{\"type\":\"end\"}}; got:\n{stdout}"
    );
    // A text delta → the completion was non-empty (not just an empty terminated run).
    assert!(
        stdout.contains(r#""text_delta""#),
        "expected at least one text_delta event (non-empty completion); got:\n{stdout}"
    );
    Some(stdout)
}

#[test]
#[ignore = "live Ollama: needs `ollama serve` + a pulled model; run with --ignored"]
fn streamed_completion_decodes_end_to_end() {
    // `--stream` exercises the real NDJSON streaming frame path (the point of a
    // "streamed completion" check) — the framed body decodes to a terminated stream.
    run_ollama("--stream");
}

#[test]
#[ignore = "live Ollama: needs `ollama serve` + a pulled model; run with --ignored"]
fn non_streamed_completion_decodes_end_to_end() {
    // `--no-stream` exercises the real NON-stream path (config §4.2, bl-24c2): Ollama
    // returns ONE aggregate JSON body that `drive` drains whole and folds via
    // `decode_full` — the SAME canonical stream (text + the trailing `{"type":"end"}`)
    // the framed form yields, proving the explode→replay fold live, not just offline.
    run_ollama("--no-stream");
}
