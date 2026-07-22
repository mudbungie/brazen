//! End-to-end `claude-code` (claude-code spec): the shipped row resolves (exec
//! substitutes for base_url, keyless), a full `run` drives the real captured stream
//! through the exec-declaring wire (argv + stdin body asserted at the seam), the
//! logged-out capture exits 77, `--raw` stamps the exec target from protocol DATA,
//! `--list-models` declines (78), and `--dump-config` round-trips the `exec` field.
//! `MockTransport` stands in for the spawn — the same seam the shim's exec kind
//! implements (spec §3.4). Offline, no subprocess.

use crate::testing::{MemoryCredStore, MockTransport};
use crate::tests::config_support::{file, no_env, resolve};
use crate::tests::run_support::{go, Out};
use crate::{CredStore, PartialConfig, ProtocolId, Transport};

const BASIC: &[u8] = include_bytes!("../../tests/fixtures/claude_code_basic.ndjson");
const LOGGED_OUT: &[u8] = include_bytes!("../../tests/fixtures/claude_code_error_loggedout.ndjson");

fn run_cc(argv: &[&str], stdin: &[u8], tx: &dyn Transport, store: &dyn CredStore) -> Out {
    go(argv, &[], stdin, tx, store)
}

#[test]
fn the_shipped_row_resolves_keyless_with_exec_for_base_url() {
    // The defaults row (spec §7.1): exec = "claude", auth = none, base_url completed
    // as "" (the empty-set path), the five slot-less canonical fields stripped.
    let cfg = resolve(
        PartialConfig {
            provider: Some("claude-code".into()),
            model: Some("haiku".into()),
            ..Default::default()
        },
        &no_env(),
        PartialConfig::default(),
        crate::defaults(),
        None,
    )
    .unwrap();
    assert_eq!(cfg.provider.protocol, ProtocolId::ClaudeCode);
    assert_eq!(cfg.provider.exec.as_deref(), Some("claude"));
    assert_eq!(cfg.provider.base_url, "");
    assert_eq!(
        cfg.provider.unsupported_body_keys,
        ["max_tokens", "temperature", "top_p", "stop", "output"]
    );
}

#[test]
fn the_pass_through_run_streams_the_real_capture_end_to_end() {
    // The real captured stream behind the seam: text out is exactly the answer
    // (thinking suppressed without --thinking), exit 0 — and the wire carries the
    // exec target + pinned argv + the prompt as the stdin body (spec §2, §4.3).
    let tx = MockTransport::ok(vec![BASIC]);
    let o = run_cc(
        &["--provider", "claude-code", "-m", "haiku", "say pong"],
        b"",
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!((o.code, o.stdout.as_str()), (0, "pong"));
    let reqs = tx.requests();
    let spec = reqs[0].exec.as_ref().expect("an exec-declaring wire");
    assert_eq!(spec.program, "claude");
    assert_eq!(spec.args[..1], ["-p".to_owned()]);
    let flag = |name: &str| {
        let at = spec.args.iter().position(|a| a == name).expect(name);
        spec.args[at + 1].clone()
    };
    assert_eq!(flag("--output-format"), "stream-json");
    assert_eq!(flag("--system-prompt"), ""); // always passed; empty = no system
    assert_eq!(flag("--model"), "haiku");
    assert_eq!(reqs[0].body, b"say pong");
}

#[test]
fn a_system_prompt_and_effort_reach_the_argv() {
    let tx = MockTransport::ok(vec![BASIC]);
    let o = run_cc(
        &[
            "--provider",
            "claude-code",
            "-m",
            "haiku",
            "--system",
            "be terse",
            "--reasoning",
            "low",
            "q",
        ],
        b"",
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 0);
    let reqs = tx.requests();
    let args = &reqs[0].exec.as_ref().unwrap().args;
    let at = args.iter().position(|a| a == "--system-prompt").unwrap();
    assert_eq!(args[at + 1], "be terse");
    let at = args.iter().position(|a| a == "--effort").unwrap();
    assert_eq!(args[at + 1], "low");
}

#[test]
fn the_logged_out_capture_exits_77_with_the_cli_message() {
    // The crisp canonical error (spec §6): never a dangle, never a bare 69.
    let tx = MockTransport::ok(vec![LOGGED_OUT]);
    let o = run_cc(
        &["--provider", "claude-code", "-m", "haiku", "q"],
        b"",
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 77);
    assert!(o.stderr.contains("Not logged in"), "stderr: {}", o.stderr);
}

#[test]
fn raw_stamps_the_exec_target_from_protocol_data() {
    // The --raw spine skips encode, so the subprocess target rides Protocol::
    // exec_spec exactly as the URL rides path() (spec §3.1): stdin bytes verbatim to
    // the child, the CLI's NDJSON verbatim back.
    let tx = MockTransport::ok(vec![BASIC]);
    let o = run_cc(
        &["--provider", "claude-code", "-m", "haiku", "--raw"],
        b"raw prompt bytes",
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout.as_bytes(), BASIC); // raw-out: the capture verbatim
    let reqs = tx.requests();
    let spec = reqs[0].exec.as_ref().expect("exec stamped on the raw path");
    assert_eq!(spec.program, "claude");
    let at = spec
        .args
        .iter()
        .position(|a| a == "--system-prompt")
        .unwrap();
    assert_eq!(spec.args[at + 1], ""); // the request-independent base argv
    assert_eq!(reqs[0].body, b"raw prompt bytes");
}

#[test]
fn list_models_declines_with_the_next_move() {
    // The honest decline (spec §7.2): Config 78, no GET ever sent.
    let tx = MockTransport::ok(vec![b"{}"]);
    let o = crate::tests::list_models_support::go(
        &["--list-models", "--provider", "claude-code"],
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 78);
    assert!(
        o.stderr.contains("has no models listing"),
        "stderr: {}",
        o.stderr
    );
    assert!(tx.requests().is_empty());
}

#[test]
fn a_user_row_with_exec_and_no_base_url_resolves_and_round_trips() {
    // The §7.1 completion rule on a NON-shipped row + the dump round-trip: `exec`
    // is ordinary row data (fold, serialize, re-parse).
    let toml = "[[provider]]\nname = \"mycli\"\nexec = \"/opt/claude\"\n\
                protocol = \"claude_code\"\nauth = \"none\"\n";
    let cfg = resolve(
        PartialConfig {
            provider: Some("mycli".into()),
            model: Some("sonnet".into()),
            ..Default::default()
        },
        &no_env(),
        file(toml),
        crate::defaults(),
        None,
    )
    .unwrap();
    assert_eq!(cfg.provider.exec.as_deref(), Some("/opt/claude"));
    assert_eq!(cfg.provider.base_url, "");
    let dumped = crate::dump_config(
        PartialConfig::default(),
        &crate::EnvSnapshot::default(),
        file(toml),
    )
    .unwrap();
    assert!(dumped.contains("exec = \"/opt/claude\""), "dump: {dumped}");
    let reparsed = crate::parse_config(&dumped).unwrap();
    let (name, row) = &reparsed.providers[0];
    assert_eq!(
        (name.as_str(), row.exec.as_deref()),
        ("mycli", Some("/opt/claude"))
    );
}
