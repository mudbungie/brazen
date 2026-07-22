//! `claude_code` REQUEST projection (claude-code spec §2, §4): the pinned argv +
//! stdin prompt, the system/effort mappings, and every single-turn/text-only
//! rejection — each an encode-time `ParseInput` (64), never a silent drop. Pure
//! table tests, no IO.

use crate::protocol::claude_code::ClaudeCode;
use crate::{
    CanonicalError, CanonicalRequest, Content, ErrorKind, Message, Protocol, ProviderCtx,
    ReasoningEffort, Role, Tool, ToolChoice, WireRequest,
};

/// The fixed ctx of the shipped row: exec = "claude", empty base_url, no betas.
fn ctx<'a>() -> ProviderCtx<'a> {
    ProviderCtx {
        base_url: "",
        model: "haiku",
        beta_headers: &[],
        exec: Some("claude"),
    }
}

fn user(text: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![Content::Text(text.into())],
    }
}

fn req(text: &str) -> CanonicalRequest {
    CanonicalRequest {
        messages: vec![user(text)],
        ..Default::default()
    }
}

fn enc(req: &CanonicalRequest) -> Result<WireRequest, CanonicalError> {
    ClaudeCode.encode(req, &ctx())
}

/// The request-independent base argv (spec §2), with `system`/`model` spliced in —
/// the test's own spelling of the pinned flag set, asserted against encode's.
fn base_args(system: &str, model: &str) -> Vec<String> {
    [
        "-p",
        "--output-format",
        "stream-json",
        "--include-partial-messages",
        "--verbose",
        "--setting-sources",
        "",
        "--tools",
        "",
        "--disable-slash-commands",
        "--strict-mcp-config",
        "--no-session-persistence",
        "--system-prompt",
        system,
        "--model",
        model,
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

#[test]
fn the_pinned_argv_and_the_stdin_prompt() {
    // The spec §2 invocation exactly: suppression flags, --system-prompt "" when the
    // request has no system (the empty-set path, same flag), the prompt on stdin.
    let wire = enc(&req("say pong")).unwrap();
    let spec = wire.exec.expect("an exec target");
    assert_eq!(spec.program, "claude");
    assert_eq!(spec.args, base_args("", "haiku"));
    assert_eq!(wire.body, b"say pong");
    assert_eq!(wire.url, ""); // no HTTP target — inert on the exec path
}

#[test]
fn system_blocks_join_and_ride_the_flag() {
    let mut r = req("q");
    r.system = Some(vec![Content::Text("a".into()), Content::Text("b".into())]);
    let spec = enc(&r).unwrap().exec.unwrap();
    assert_eq!(spec.args, base_args("a\n\nb", "haiku"));
}

#[test]
fn multiple_prompt_text_blocks_join_with_blank_lines() {
    let mut r = req("");
    r.messages[0].content = vec![Content::Text("one".into()), Content::Text("two".into())];
    assert_eq!(enc(&r).unwrap().body, b"one\n\ntwo");
}

#[test]
fn reasoning_maps_to_the_effort_flag() {
    // The canonical knob's dialect spelling (spec §4.1): appended after the base argv.
    let mut r = req("q");
    r.reasoning = Some(ReasoningEffort::High);
    let spec = enc(&r).unwrap().exec.unwrap();
    let mut expected = base_args("", "haiku");
    expected.extend(["--effort".to_owned(), "high".to_owned()]);
    assert_eq!(spec.args, expected);
}

/// Assert an encode rejection: `ParseInput` (64) whose message carries `needle`.
fn rejects(r: &CanonicalRequest, needle: &str) {
    let err = enc(r).unwrap_err();
    assert_eq!(err.kind, ErrorKind::ParseInput);
    assert!(err.message.contains(needle), "message: {}", err.message);
}

#[test]
fn multi_turn_and_non_user_transcripts_reject() {
    // Single-turn only (spec §4.2): assistant history cannot ride the CLI.
    let mut r = req("q");
    r.messages.push(user("again"));
    rejects(&r, "single-turn");
    let mut r = req("q");
    r.messages[0].role = Role::Assistant;
    rejects(&r, "single-turn");
    let mut r = req("q");
    r.messages.clear();
    rejects(&r, "single-turn");
}

#[test]
fn non_text_content_rejects_in_prompt_and_system() {
    let img = Content::Image {
        source: crate::ImageSource::Url { url: "u".into() },
    };
    let mut r = req("q");
    r.messages[0].content.push(img.clone());
    rejects(&r, "only text content in message");
    let mut r = req("q");
    r.system = Some(vec![img]);
    rejects(&r, "only text content in system");
}

#[test]
fn tools_and_a_forced_tool_choice_reject() {
    let mut r = req("q");
    r.tools = vec![Tool::Custom {
        name: "t".into(),
        description: None,
        input_schema: serde_json::json!({}),
        strict: None,
    }];
    rejects(&r, "no tool declarations");
    let mut r = req("q");
    r.tool_choice = ToolChoice::None;
    rejects(&r, "tool_choice");
}

#[test]
fn a_row_without_exec_is_a_config_error() {
    // The incomplete-row backstop (spec §4.3): Config (78), naming the fix.
    let mut c = ctx();
    c.exec = None;
    let err = ClaudeCode.encode(&req("q"), &c).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Config);
    assert!(err.message.contains("exec"));
}

#[test]
fn the_dialect_facts_are_data() {
    // path/content_type are inert on the exec path but stay one-home (spec §4.3);
    // the models listing DECLINES (spec §7.2); exec_spec mirrors the row's target
    // for the --raw spine and is None exactly when the row carries no exec.
    assert_eq!(ClaudeCode.path(&ctx()), "");
    assert_eq!(ClaudeCode.content_type(), "text/plain");
    assert_eq!(ClaudeCode.models_shape(), None);
    let spec = ClaudeCode.exec_spec(&ctx()).unwrap();
    assert_eq!(spec.program, "claude");
    assert_eq!(spec.args, base_args("", "haiku"));
    let mut bare = ctx();
    bare.exec = None;
    assert!(ClaudeCode.exec_spec(&bare).is_none());
}
