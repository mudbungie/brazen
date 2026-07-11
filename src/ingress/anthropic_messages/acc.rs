//! The `anthropic_messages` ingress ENCODE fold (ingress.md §10): the cross-event
//! accumulator every event mutates — the open blocks keyed by canonical content index,
//! the finished wire blocks (the non-stream `message.content`), the cumulative usage,
//! and the terminal stop reason/details. The aggregate `message` body IS this fold
//! rendered once at `End`. [`super::encode`] dispatches events into these; the wire
//! renderings live in [`super::frames`].

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::canonical::{ContentKind, Usage};

/// Cross-event anthropic fold. The generic identity/adaptation/error fields live on the
/// shared [`IngressState`](crate::ingress::state::IngressState); this is the dialect's own.
#[derive(Default)]
pub(crate) struct AnthAcc {
    pub(super) open: BTreeMap<u32, OpenBlock>,
    pub(super) content: Vec<Value>,
    pub(super) usage: Usage,
    pub(super) stop_reason: Option<String>,
    pub(super) stop_details: Option<Value>,
}

/// One in-flight block accumulating toward its finished wire shape (§10).
pub(super) enum OpenBlock {
    Text(String),
    /// A `tool_use` or `server_tool_use` call: `ty` is the verbatim wire tag; `args`
    /// are the `input_json_delta` fragments, concatenated, parsed only at close.
    Tool {
        ty: &'static str,
        id: String,
        name: String,
        args: String,
    },
    Thinking {
        text: String,
        signature: String,
    },
    Redacted(String),
    /// A server-tool RESULT: the whole block arrived inline at start, re-emitted whole.
    ServerResult(Value),
    /// A block this dialect cannot carry (unknown kind): dropped from both shapes.
    Skip,
}

/// A `ContentKind` → (its fold accumulator, its `content_block` shape at start). `None`
/// for a kind with no anthropic wire slot (`Other`, and any future kind).
pub(super) fn open_block(kind: &ContentKind) -> (OpenBlock, Option<Value>) {
    match kind {
        ContentKind::Text {} => (
            OpenBlock::Text(String::new()),
            Some(json!({"type": "text", "text": ""})),
        ),
        ContentKind::ToolUse { id, name } => tool("tool_use", id, name),
        ContentKind::ServerToolUse { id, name } => tool("server_tool_use", id, name),
        ContentKind::Thinking { .. } => (
            OpenBlock::Thinking {
                text: String::new(),
                signature: String::new(),
            },
            Some(json!({"type": "thinking", "thinking": "", "signature": ""})),
        ),
        ContentKind::RedactedThinking { data } => (
            OpenBlock::Redacted(data.clone()),
            Some(json!({"type": "redacted_thinking", "data": data})),
        ),
        ContentKind::ServerToolResult {
            kind,
            tool_use_id,
            content,
        } => {
            let cb = json!({"type": kind, "tool_use_id": tool_use_id, "content": content});
            (OpenBlock::ServerResult(cb.clone()), Some(cb))
        }
        _ => (OpenBlock::Skip, None),
    }
}

/// A `tool_use`/`server_tool_use` block: identity at start, `input:{}` (real args
/// stream as `input_json_delta`s).
fn tool(ty: &'static str, id: &str, name: &str) -> (OpenBlock, Option<Value>) {
    (
        OpenBlock::Tool {
            ty,
            id: id.to_owned(),
            name: name.to_owned(),
            args: String::new(),
        },
        Some(json!({"type": ty, "id": id, "name": name, "input": {}})),
    )
}

/// An open block → its finished wire block for the non-stream `message.content` (§10);
/// `None` for a `Skip` block (no wire shape).
pub(super) fn finish_block(block: OpenBlock) -> Option<Value> {
    Some(match block {
        OpenBlock::Text(text) => json!({"type": "text", "text": text}),
        OpenBlock::Tool { ty, id, name, args } => {
            json!({"type": ty, "id": id, "name": name, "input": parse_args(&args)})
        }
        OpenBlock::Thinking { text, signature } => {
            json!({"type": "thinking", "thinking": text, "signature": signature})
        }
        OpenBlock::Redacted(data) => json!({"type": "redacted_thinking", "data": data}),
        OpenBlock::ServerResult(v) => v,
        OpenBlock::Skip => return None,
    })
}

/// Accumulated `input_json_delta` fragments → the block's parsed `input` (parsed only
/// at close, never mid-stream): `""` is the wire's empty-input convention → `{}`; a
/// fragment stream that never became JSON degrades to `null`, never a panic (arch §9.5).
fn parse_args(args: &str) -> Value {
    if args.is_empty() {
        return Value::Object(Map::new());
    }
    serde_json::from_str(args).unwrap_or(Value::Null)
}

/// Merge one `Usage` event into the cumulative fold, last-wins per field (§3.6): a
/// reported counter overwrites; an absent one leaves the prior value.
pub(super) fn merge_usage(acc: &mut Usage, u: &Usage) {
    if u.input_tokens.is_some() {
        acc.input_tokens = u.input_tokens;
    }
    if u.output_tokens.is_some() {
        acc.output_tokens = u.output_tokens;
    }
    if u.cache_read_tokens.is_some() {
        acc.cache_read_tokens = u.cache_read_tokens;
    }
    if u.cache_write_tokens.is_some() {
        acc.cache_write_tokens = u.cache_write_tokens;
    }
}
