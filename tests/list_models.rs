//! End-to-end `bz list-models` verb (model-discovery §2): provider resolution (the
//! same `into_resolved(None)` query), the one models GET (auth + the row's required
//! `anthropic-version` header), the two output shapes (`--json` object, default text),
//! and the error paths (NoProvider/78, auth/77, non-2xx/69-70). `MockTransport`; offline.

use std::collections::BTreeMap;
use std::io::{self, Write};

use brazen::testing::{Chunk, FakeClock, MemoryCredStore, MockTransport};
use brazen::{list_models, Args, Cred, CredStore, EnvSnapshot, ListIo, Method, Secret, Transport};

/// The anthropic `/v1/models` body (newest-first), as `data[].id` (§3.1).
const MODELS: &[u8] = br#"{"data":[
    {"type":"model","id":"claude-opus-4-1-20250805"},
    {"type":"model","id":"claude-sonnet-4-5-20250929"}
],"has_more":false}"#;

/// Outcome of one `list-models`: exit code, captured stdout, captured stderr.
struct Out {
    code: u8,
    stdout: String,
    stderr: String,
}

/// Drive `brazen::list_models` against the in-memory seams. The argv begins with the
/// `list-models` verb word (the shim strips none — the verb parses `argv[1..]`).
fn go(argv: &[&str], tx: &dyn Transport, store: &dyn CredStore) -> Out {
    let mut out = Vec::new();
    let code = go_out(argv, tx, store, &mut out);
    Out {
        code: code.0,
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: code.1,
    }
}

/// Drive the verb against an arbitrary stdout writer (e.g. a failing one), returning
/// the exit code and captured stderr.
fn go_out(
    argv: &[&str],
    tx: &dyn Transport,
    store: &dyn CredStore,
    out: &mut dyn Write,
) -> (u8, String) {
    let args = Args {
        argv: argv.iter().map(|s| (*s).to_string()).collect(),
        env: EnvSnapshot(BTreeMap::new()),
        tty: false,
    };
    let clock = FakeClock::new(0);
    let mut err = Vec::new();
    let code = {
        let mut io = ListIo {
            stdout: out,
            stderr: &mut err,
            transport: tx,
            store,
            clock: &clock,
        };
        list_models(&args, &mut io)
    };
    (code, String::from_utf8_lossy(&err).into_owned())
}

/// A stdout writer that always fails — the listing write-failure path (→69).
struct FailWriter;
impl Write for FailWriter {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("disk full"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::other("disk full"))
    }
}

#[test]
fn text_prints_ids_one_per_line_in_provider_order() {
    let tx = MockTransport::ok(vec![MODELS]);
    let o = go(
        &["list-models", "--provider", "anthropic", "--api-key", "sk"],
        &tx,
        &MemoryCredStore::new(),
    );
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
            "list-models",
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
fn unflagged_ids_carry_no_suffix() {
    // No dialect flags a default today (§3.1), so a real listing has no ` (default)`
    // suffix — the bare ids, one per line. The suffix branch itself (a provider that
    // DOES flag one) is unit-tested on `print_models` in the module (no integration
    // body can set `default:true`).
    let body = br#"{"data":[{"id":"a"},{"id":"b"}]}"#;
    let tx = MockTransport::ok(vec![body]);
    let o = go(
        &["list-models", "--provider", "openai", "--api-key", "sk"],
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "a\nb\n");
}

#[test]
fn no_provider_is_config_78() {
    // No `--provider` and no configured model → `into_resolved` cannot route → 78 on
    // stderr (the verb has no in-band stream).
    let tx = MockTransport::ok(vec![MODELS]);
    let o = go(
        &["list-models", "--api-key", "sk"],
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 78);
    assert!(!o.stderr.is_empty());
    assert!(o.stdout.is_empty());
    assert!(tx.requests().is_empty()); // resolution failed before any send
}

#[test]
fn unknown_provider_is_config_78() {
    let tx = MockTransport::ok(vec![MODELS]);
    let o = go(
        &["list-models", "--provider", "nope", "--api-key", "sk"],
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 78);
    assert!(tx.requests().is_empty());
}

#[test]
fn missing_credential_is_auth_77() {
    // No `--api-key` and an empty store → `Auth::apply` fails MissingCreds → 77.
    let tx = MockTransport::ok(vec![MODELS]);
    let o = go(
        &["list-models", "--provider", "anthropic"],
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 77);
    assert!(tx.requests().is_empty()); // auth failed before the send
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
    let o = go(&["list-models", "--provider", "anthropic"], &tx, &store);
    assert_eq!(o.code, 0);
    assert_eq!(tx.requests()[0].header("x-api-key"), Some("sk-store"));
}

#[test]
fn a_non_2xx_models_response_maps_the_status_and_carries_the_body() {
    // The discovery path drains the non-2xx body and routes it through the SAME
    // `http_error` home the data plane uses (bl-dcfe): the status drives the exit AND
    // the body reaches the user as `message` (the verb's stderr channel), never a
    // bespoke "HTTP {status}" that throws it away. Each case is a distinct envelope —
    // `error.message`, a `detail` (codex shape), a bare plain body — so the projection
    // assumes no uniform `{"error":…}` shape; the `want` substring is the lifted message.
    for (status, body, exit, want) in [
        (
            503u16,
            &br#"{"error":{"message":"is down"}}"#[..],
            70,
            "is down",
        ),
        (401, &br#"{"detail":"bad version"}"#[..], 77, "bad version"),
        (404, &b"no route"[..], 69, "no route"),
    ] {
        let tx = MockTransport::new(status, vec![Chunk::Data(body.to_vec())]);
        let o = go(
            &["list-models", "--provider", "anthropic", "--api-key", "sk"],
            &tx,
            &MemoryCredStore::new(),
        );
        assert_eq!(o.code, exit, "{status} → exit {exit}");
        assert!(
            o.stderr.contains(want),
            "{status} carries body: {}",
            o.stderr
        );
    }
}

#[test]
fn an_empty_list_prints_nothing_at_0() {
    // The verb LISTS, it does not select: a well-formed EMPTY body is a successful
    // empty listing (0). The empty-list→Config(78) contract is the probe's (serve),
    // proven in `run_probe` — not the verb's.
    let tx = MockTransport::ok(vec![br#"{"data":[]}"#]);
    let o = go(
        &["list-models", "--provider", "anthropic", "--api-key", "sk"],
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "");
}

#[test]
fn a_malformed_body_is_provider_70() {
    // A drained 2xx that does not project to the list shape is the `Provider{502}`
    // `decode_models` raises → exit 70 (model-discovery §2).
    let tx = MockTransport::ok(vec![b"{not json"]);
    let o = go(
        &["list-models", "--provider", "anthropic", "--api-key", "sk"],
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 70);
    assert!(o.stderr.contains("malformed models list"));
}

#[test]
fn a_mid_body_transport_drop_is_69() {
    // A 200 whose body fails part-way (an injected mid-stream drop) → `drain` surfaces
    // a `Transport` error → 69.
    let tx = MockTransport::new(
        200,
        vec![
            Chunk::Data(br#"{"data":["#.to_vec()),
            Chunk::Fail(io::ErrorKind::ConnectionReset),
        ],
    );
    let o = go(
        &["list-models", "--provider", "anthropic", "--api-key", "sk"],
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 69);
    assert!(o.stderr.contains("failed to read models response body"));
}

#[test]
fn a_stdout_write_failure_is_69() {
    // The listing cannot be written (a closed/failing stdout) → `Transport` (→69), the
    // verb's pre-sink analogue of the data plane's write handling.
    let tx = MockTransport::ok(vec![MODELS]);
    let (code, stderr) = go_out(
        &["list-models", "--provider", "anthropic", "--api-key", "sk"],
        &tx,
        &MemoryCredStore::new(),
        &mut FailWriter,
    );
    assert_eq!(code, 69);
    assert!(stderr.contains("failed to write model list"));
}

#[test]
fn a_usage_error_in_the_verb_argv_is_64() {
    // The verb reuses the full flag parser → an unknown flag is the same usage error 64.
    let tx = MockTransport::ok(vec![MODELS]);
    let o = go(
        &["list-models", "--provider", "anthropic", "--bogus"],
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 64);
    assert!(tx.requests().is_empty());
}
