//! Command-valued row headers (config §4.3.2): `header_commands` runs an operator
//! argv over the transport seam and overlays its trimmed stdout on the same-named
//! `beta_headers` entry — on every request the row makes — while a command that
//! yields no usable value leaves the static entry standing.

use std::io;
use std::sync::Mutex;

use crate::testing::{MemoryCredStore, MockTransport};
use crate::tests::run_support::{go, temp, BASIC};
use crate::tests::{count_support, list_models_support};
use crate::{
    CanonicalError, Envelope, ErrorKind, ExecSpec, Transport, TransportResponse, WireRequest,
};

type Reply = fn() -> Result<Vec<io::Result<Vec<u8>>>, CanonicalError>;

/// Answers a subprocess wire with `reply` (recording its spec) and passes every
/// other wire to the canned HTTP double.
struct Commands {
    reply: Reply,
    http: MockTransport,
    ran: Mutex<Vec<ExecSpec>>,
}

impl Commands {
    fn new(reply: Reply) -> Self {
        Commands {
            reply,
            http: MockTransport::ok(vec![BASIC]),
            ran: Mutex::new(Vec::new()),
        }
    }

    /// The `user-agent` the one HTTP request carried.
    fn user_agent(&self) -> String {
        let sent = self.http.requests();
        assert_eq!(sent.len(), 1, "one HTTP round-trip");
        sent[0].header("user-agent").unwrap().to_owned()
    }
}

impl Transport for Commands {
    fn send(&self, wire: WireRequest) -> Result<TransportResponse, CanonicalError> {
        let Some(spec) = wire.exec.clone() else {
            return self.http.send(wire);
        };
        self.ran.lock().unwrap().push(spec);
        Ok(TransportResponse {
            status: 200,
            body: Box::new((self.reply)()?.into_iter()),
            retry_after: None,
        })
    }
}

fn row(commands: &str) -> String {
    format!(
        r#"
[[provider]]
name = "ua-test"
base_url = "https://example.test"
protocol = "anthropic_messages"
auth = "none"
beta_headers = [["User-Agent", "pinned/1"], ["anthropic-version", "2023-06-01"]]
header_commands = {commands}
body_defaults = {{ max_tokens = 8 }}
"#
    )
}

const DERIVED: &str = r#"{ user-agent = ["ua-cmd", "--flag"] }"#;

fn generate(tx: &Commands, commands: &str) {
    let cfg = temp(&row(commands));
    let out = go(
        &["--provider", "ua-test", "--model", "claude-test", "prompt"],
        &[("BRAZEN_CONFIG", cfg.0.to_str().unwrap())],
        b"",
        tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
}

#[test]
fn command_stdout_replaces_the_static_header_on_every_request_kind() {
    let reply: Reply = || Ok(vec![Ok(b"derived/".to_vec()), Ok(b"2\n".to_vec())]);

    let tx = Commands::new(reply);
    generate(&tx, DERIVED);
    assert_eq!(tx.user_agent(), "derived/2");
    // The case-insensitive replace leaves ONE user-agent and the other static header.
    let sent = &tx.http.requests()[0];
    let agents = sent
        .headers
        .iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case("user-agent"));
    assert_eq!(agents.count(), 1);
    assert_eq!(sent.header("anthropic-version"), Some("2023-06-01"));
    assert_eq!(
        *tx.ran.lock().unwrap(),
        vec![ExecSpec {
            program: "ua-cmd".into(),
            args: vec!["--flag".into()],
            envelope: Envelope::Body,
        }]
    );

    let cfg = temp(&row(DERIVED));
    let path = cfg.0.to_str().unwrap();
    let store = MemoryCredStore::new();

    let tx = Commands::new(reply);
    go(
        &["--raw=in", "--provider", "ua-test"],
        &[("BRAZEN_CONFIG", path)],
        b"{\"stream\":true}",
        &tx,
        &store,
    );
    assert_eq!(tx.user_agent(), "derived/2");

    let tx = Commands::new(reply);
    list_models_support::go(
        &["--list-models", "--provider", "ua-test", "--config", path],
        &tx,
        &store,
    );
    assert_eq!(tx.user_agent(), "derived/2");

    let tx = Commands::new(reply);
    count_support::go(
        &[
            "--count-tokens",
            "--provider",
            "ua-test",
            "--model",
            "claude-test",
            "--config",
            path,
        ],
        b"{\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}",
        &tx,
        &store,
    );
    assert_eq!(tx.user_agent(), "derived/2");
}

#[test]
fn a_command_with_no_usable_output_leaves_the_static_header() {
    let failures: [Reply; 5] = [
        || {
            Err(CanonicalError {
                kind: ErrorKind::Transport,
                message: "exec transport: failed to spawn `ua-cmd`".into(),
                provider_detail: None,
                retry_after_seconds: None,
            })
        },
        || {
            Ok(vec![
                Ok(b"half".to_vec()),
                Err(io::Error::other("child exited with status 1")),
            ])
        },
        || Ok(vec![Ok(vec![0xff])]),
        || Ok(vec![Ok(b" \n".to_vec())]),
        || Ok(vec![Ok(b"a\nb".to_vec())]),
    ];
    for reply in failures {
        let tx = Commands::new(reply);
        generate(&tx, DERIVED);
        assert_eq!(tx.user_agent(), "pinned/1");
        assert_eq!(tx.ran.lock().unwrap().len(), 1);
    }
}

#[test]
fn an_empty_argv_runs_nothing() {
    let tx = Commands::new(|| Ok(Vec::new()));
    generate(&tx, "{ user-agent = [] }");
    assert_eq!(tx.user_agent(), "pinned/1");
    assert!(tx.ran.lock().unwrap().is_empty());
}

#[test]
fn a_command_adds_a_header_no_static_entry_names() {
    let tx = Commands::new(|| Ok(vec![Ok(b"v".to_vec())]));
    generate(&tx, r#"{ x-derived = ["cmd"] }"#);
    assert_eq!(tx.user_agent(), "pinned/1");
    assert_eq!(tx.http.requests()[0].header("x-derived"), Some("v"));
}
