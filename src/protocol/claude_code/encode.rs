//! REQUEST projection (claude-code spec §2, §4): canonical request → the pinned
//! `claude` argv + the prompt on stdin. The dialect is SINGLE-TURN and text-only —
//! exactly one `user` message of `Text` blocks; everything unrepresentable REJECTS at
//! encode (`ParseInput`/64, the arch §3.1 rule), never a silent drop or a fabricated
//! transcript. The suppression flag set is composed (never `--bare`, which severs the
//! CLI's own OAuth — spec §2); `max_tokens`/`temperature`/`top_p`/`stop`/`output`
//! have no wire slot (the shipped row strips them, config §4.1.1) and `req.extra` is
//! dropped — the one dialect where the forward valve cannot reach the provider
//! (spec §4.1, an owned, documented inverse).

use crate::canonical::{CanonicalError, CanonicalRequest, Content, ErrorKind, ToolChoice};
use crate::protocol::{ExecSpec, ProviderCtx, WireRequest};

/// Project the canonical request onto the child invocation (spec §4): validate the
/// single-turn text-only shape, join the system/prompt text, and build the
/// `WireRequest` whose `exec` carries the pinned argv and whose `body` is the prompt
/// bytes for the child's stdin. A `claude_code` row without `exec` is an incomplete
/// row surfaced here as a `Config` error (spec §4.3).
pub(super) fn encode(
    req: &CanonicalRequest,
    ctx: &ProviderCtx,
) -> Result<WireRequest, CanonicalError> {
    let program = ctx.exec.ok_or_else(|| CanonicalError {
        kind: ErrorKind::Config,
        message: "claude_code row carries no `exec` (the subprocess program); \
                  add `exec = \"claude\"` to the provider row"
            .to_owned(),
        provider_detail: None,
        retry_after_seconds: None,
    })?;
    if !req.tools.is_empty() {
        return Err(reject(
            "claude_code carries no tool declarations; use the `anthropic` row for tools",
        ));
    }
    if !matches!(req.tool_choice, ToolChoice::Auto) {
        return Err(reject(
            "claude_code cannot express a tool_choice (no tools reach the CLI)",
        ));
    }
    let system = text_only(req.system.as_deref().unwrap_or(&[]), "system")?;
    let prompt = prompt_text(req)?;

    let mut spec = exec_spec(program, &system, ctx.model);
    if let Some(effort) = req.reasoning {
        // The canonical reasoning knob's dialect spelling (spec §4.1, providers §6):
        // `--effort low|medium|high`, the CLI's own vocabulary, verbatim.
        spec.args.push("--effort".to_owned());
        spec.args.push(effort.as_str().to_owned());
    }
    let mut wire = WireRequest::new(String::new(), prompt.into_bytes());
    wire.exec = Some(spec);
    Ok(wire)
}

/// The pinned, request-independent child invocation (spec §2) — the ONE argv home,
/// shared by `encode` (which appends `--effort`) and the trait's `exec_spec` (the
/// `--raw` spine, which has no parsed request and passes an empty system). Every
/// flag is load-bearing; see the spec's table for why each is here and why `--bare`
/// is NOT (it severs the CLI's own OAuth — the row's reason to exist).
pub(super) fn exec_spec(program: &str, system: &str, model: &str) -> ExecSpec {
    let args = [
        "-p",
        "--output-format",
        "stream-json",
        "--include-partial-messages",
        "--verbose", // the CLI refuses `-p --output-format stream-json` without it
        "--setting-sources",
        "", // no settings -> no hooks, no CLAUDE.md, no output styles
        "--tools",
        "", // no built-in tools -> nothing to loop on, no permission prompts
        "--disable-slash-commands",
        "--strict-mcp-config", // with no --mcp-config: zero MCP servers
        "--no-session-persistence",
        "--system-prompt",
        system, // ALWAYS passed: replaces the CLI's default system prompt ("" = none)
        "--model",
        model,
    ];
    ExecSpec {
        program: program.to_owned(),
        args: args.iter().map(|s| (*s).to_owned()).collect(),
    }
}

/// The stdin prompt (spec §4.2): exactly ONE `user` message of `Text` blocks, joined
/// by blank lines. Assistant history, tool blocks, media, or a positional system
/// message reject — the CLI's print mode takes one prompt, and any transcript
/// projection would be fabrication, not translation.
fn prompt_text(req: &CanonicalRequest) -> Result<String, CanonicalError> {
    match req.messages.as_slice() {
        [one] if one.role == crate::canonical::Role::User => text_only(&one.content, "message"),
        _ => Err(reject(
            "claude_code is single-turn: the request must carry exactly one user \
             message (multi-turn replay cannot ride the CLI; use the `anthropic` row)",
        )),
    }
}

/// Join a slot's `Text` blocks with blank lines; any non-`Text` block rejects (the
/// arch §3.1 text-only-slot rule — a documented narrowing, never a silent drop).
fn text_only(blocks: &[Content], slot: &str) -> Result<String, CanonicalError> {
    let mut texts = Vec::with_capacity(blocks.len());
    for block in blocks {
        match block {
            Content::Text(t) => texts.push(t.as_str()),
            _ => {
                return Err(reject(&format!(
                    "claude_code accepts only text content in {slot} \
                     (images/documents/tool blocks cannot ride the CLI)"
                )))
            }
        }
    }
    Ok(texts.join("\n\n"))
}

/// An unrepresentable-input rejection: `ParseInput` → exit 64 (arch §3.1).
fn reject(message: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::ParseInput,
        message: message.to_owned(),
        provider_detail: None,
        retry_after_seconds: None,
    }
}
