//! The `[provider.transport]` CONFIG surface (transport spec §4.2): how the block
//! folds across config layers, and the two ways a row can state it wrongly. The
//! "which requests ride it" half is `transport_select`.

use crate::tests::run_support::{empty_store, go, ok_basic, temp};
use crate::{Envelope, ExecSpec};

#[test]
fn a_file_layer_selects_a_transport_for_an_embedded_row_without_redeclaring_it() {
    // The block folds like `[provider.models]` (config §3.2): patch ONE field of an
    // embedded row — the shipped `anthropic` row keeps its url/protocol/auth.
    let cfg = temp(
        r#"
[[provider]]
name = "anthropic"

  [provider.transport]
  program = "relay"
"#,
    );
    let tx = ok_basic();
    let o = go(
        &[
            "--config",
            cfg.0.to_str().unwrap(),
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
            "hi",
        ],
        &[],
        b"",
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    let sent = tx.requests();
    assert_eq!(
        sent[0].exec,
        Some(ExecSpec {
            program: "relay".into(),
            args: vec![],
            envelope: Envelope::Http,
        })
    );
    assert_eq!(sent[0].url, "https://api.anthropic.com/v1/messages");
}

#[test]
fn a_row_that_sets_both_exec_and_transport_is_config_78() {
    // The two readings of one subprocess seam — "the child IS the provider" vs "the
    // child IS the transport" — cannot both hold; the contradiction is surfaced.
    let cfg = temp(
        r#"
[[provider]]
name = "both"
exec = "claude"
protocol = "claude_code"
auth = "none"

  [provider.transport]
  program = "relay"
"#,
    );
    let o = go(
        &[
            "--config",
            cfg.0.to_str().unwrap(),
            "--provider",
            "both",
            "--model",
            "sonnet",
            "hi",
        ],
        &[],
        b"",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 78);
    assert!(o.stderr.contains("transport"), "{}", o.stderr);
    assert!(o.stderr.contains("exec"), "{}", o.stderr);
}

#[test]
fn an_unknown_key_in_the_block_is_a_malformed_file() {
    // `deny_unknown_fields`, like `[provider.models]`: a typo is not silently dropped.
    let cfg = temp(
        r#"
[[provider]]
name = "anthropic"

  [provider.transport]
  program = "relay"
  arguments = ["--nope"]
"#,
    );
    let o = go(
        &["--config", cfg.0.to_str().unwrap(), "hi"],
        &[],
        b"",
        &ok_basic(),
        &empty_store(),
    );
    assert_eq!(o.code, 78);
    assert!(o.stderr.contains("malformed config"), "{}", o.stderr);
}
