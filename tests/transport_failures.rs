//! Every way a selected transport can fail, through the REAL `bz` binary and the
//! real exit table (transport spec §6): startup, protocol, truncation, cancellation
//! and non-2xx. Each delegate is a three-line `/bin/sh` stub that answers from
//! `printf` alone — a failing transport needs no server, so these run in
//! milliseconds and can never flake on a socket.
#![cfg(unix)]

#[allow(dead_code)]
#[path = "live_support/exec.rs"]
mod exec;
mod transport_support;

use transport_support::temp::{self, script, TempPath};

/// A row whose HTTP is delegated to `program`. `base_url` is a black hole: a
/// delegate stub answers from itself, so nothing must ever reach the network — and
/// if a regression made `bz` perform the request itself, the run would fail loudly
/// rather than pass quietly.
fn config(program: &str) -> TempPath {
    temp::config(&format!(
        r#"
[[provider]]
name = "obs"
base_url = "http://127.0.0.1:1"
protocol = "anthropic_messages"
auth = "api_key"
api_header = {{ name = "x-api-key", scheme = "raw" }}
body_defaults = {{ max_tokens = 16 }}

  [provider.transport]
  program = "{program}"
"#
    ))
}

/// Run `bz --json` through `program` and return (exit, stdout, stderr).
fn run(program: &str, extra: &[&str]) -> (i32, String, String) {
    let cfg = config(program);
    let mut args: Vec<String> = [
        "--json",
        "--config",
        cfg.to_str().expect("config path"),
        "--provider",
        "obs",
        "--model",
        "claude-x",
        "--api-key",
        "sk",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    args.extend(extra.iter().map(|s| (*s).to_owned()));
    args.push("hi".to_owned());
    exec::run_bz(&args, "")
}

#[test]
fn a_delegate_that_cannot_be_spawned_is_a_transport_failure_naming_it() {
    // Startup: the exec analogue of an unreachable host (spec §6).
    let (code, out, _) = run("/nonexistent/relay-binary", &[]);
    assert_eq!(code, 69);
    assert!(out.contains("/nonexistent/relay-binary"), "{out}");
}

#[test]
fn a_delegate_that_answers_garbage_is_a_transport_failure_that_echoes_nothing() {
    // Protocol: no status line. The diagnostic names a reason and NEVER echoes head
    // bytes — a delegate that echoed the request back must not leak the credential.
    let stub = script(r#"cat > /dev/null; printf 'I am not HTTP\r\n\r\nbody'"#);
    let (code, out, err) = run(stub.to_str().expect("stub path"), &[]);
    assert_eq!(code, 69);
    assert!(out.contains("no HTTP status line"), "{out}");
    assert!(!out.contains("sk") && !err.contains("sk"), "{out}{err}");
}

#[test]
fn a_delegate_that_dies_surfaces_its_own_stderr() {
    // Protocol: the operator's program failed on its own terms. Its stderr is the
    // real diagnostic (a bad flag, a missing profile), so it survives.
    let stub = script("cat > /dev/null; echo 'profile \"reference\" not installed' >&2; exit 3");
    let (code, out, _) = run(stub.to_str().expect("stub path"), &[]);
    assert_eq!(code, 69);
    assert!(out.contains("profile"), "{out}");
}

#[test]
fn a_delegate_that_answers_a_head_and_nothing_else_is_a_premature_eof() {
    // Truncation: a 200 whose stream never terminates is the existing premature-EOF
    // path — the delegate changes nothing about how a cut stream is judged.
    let stub = script(r#"cat > /dev/null; printf 'HTTP/1.1 200 OK\r\n\r\n'"#);
    let (code, out, _) = run(stub.to_str().expect("stub path"), &[]);
    assert_eq!(code, 69);
    assert!(out.contains("error"), "{out}");
}

#[test]
fn a_delegate_that_says_nothing_is_killed_by_the_silence_budget() {
    // Cancellation: `bz` enforces the inter-chunk bound itself rather than trusting
    // the delegate (spec §5.3) — the child is killed and reaped, never orphaned.
    let stub = script("cat > /dev/null; sleep 60");
    let started = std::time::Instant::now();
    let (code, out, _) = run(stub.to_str().expect("stub path"), &["--timeout", "1"]);
    assert_eq!(code, 69);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "not killed"
    );
    assert!(out.contains("silence budget"), "{out}");
}

#[test]
fn a_delegates_429_carries_the_status_and_the_retry_after_hint() {
    // Non-2xx: NOT a transport failure. The delegate's parsed status flows through
    // the same status→kind table as ureq's, and `retry-after` — the one response
    // header brazen keeps — reaches the caller as the canonical pacing hint.
    let body = r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#;
    let stub = script(&format!(
        r#"cat > /dev/null; printf 'HTTP/1.1 429 Too Many Requests\r\nRetry-After: 7\r\nContent-Type: application/json\r\n\r\n{body}'"#
    ));
    let (code, out, _) = run(stub.to_str().expect("stub path"), &[]);
    assert_eq!(
        code, 69,
        "429 rides the 4xx exit, distinguished by retryable"
    );
    let v: serde_json::Value = out
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|v| v["type"] == "error")
        .expect("a canonical error event");
    assert_eq!(v["kind"]["provider"]["status"], 429);
    assert_eq!(v["retry_after_seconds"], 7);
    assert!(v["provider_detail"].to_string().contains("slow down"));
}

#[test]
fn a_delegates_2xx_stream_completes_normally() {
    // The happy path through a delegate, end to end: head parsed, body streamed,
    // exit 0 — so every assertion above is about failure, not about the seam.
    let sse = include_str!("fixtures/anthropic_messages_basic.sse").replace('\n', "\\n");
    let stub = script(&format!(
        r#"cat > /dev/null; printf 'HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n{sse}'"#
    ));
    let (code, out, err) = run(stub.to_str().expect("stub path"), &[]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("content_delta") || out.contains("Hello"),
        "{out}"
    );
}
