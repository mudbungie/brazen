//! The `messages[]` / `system` folds (anthropic-messages §2.3–§2.5, inverted): the
//! top-level `system` (string | text blocks) → `req.system`; each wire message → one
//! canonical `Message`. A `"user"` message bearing any `tool_result` block is a
//! `Role::Tool` turn (Anthropic packs tool results into user turns, §2.3); every other
//! `"user"` message is `Role::User`, `"assistant"` is `Role::Assistant`. Content parts
//! invert per §2.5 — thinking `signature`, `redacted_thinking` `data`, and the two
//! server-tool blocks all decode VERBATIM (the dialect carries them natively, so the
//! replay stash stays idle here). Per-block `cache_control` marks are the encoder's own
//! automatic policy (§2.10) with no canonical home → ignored (tolerant reader).

use serde_json::Value;

use super::{arr_of, bad, err, obj_of, opt_str, str_of};
use crate::canonical::{CanonicalRequest, Content, DocumentSource, ImageSource, Message, Role};
use crate::ingress::IngressError;

/// Fold the wire `messages` array into `req.messages`.
pub(super) fn fold(v: &Value, req: &mut CanonicalRequest) -> Result<(), IngressError> {
    for (i, m) in arr_of(Some(v), "messages")?.iter().enumerate() {
        let path = format!("messages[{i}]");
        let obj = obj_of(Some(m), &path)?;
        let content = parts(obj.get("content"), &path)?;
        let role = match str_of(obj.get("role"), &format!("{path}.role"))? {
            "assistant" => Role::Assistant,
            // A tool-result-bearing user turn is the canonical `Role::Tool` (§2.3);
            // any other user turn is `Role::User` — the inverse of the egress
            // `Role::Tool` → `"user"` + `tool_result` projection.
            "user" if content.iter().any(is_tool_result) => Role::Tool,
            "user" => Role::User,
            other => {
                return Err(err(format!(
                    "`{path}.role` \"{other}\" is not a messages role"
                )))
            }
        };
        req.messages.push(Message { role, content });
    }
    Ok(())
}

/// The top-level `system` (§2.4, inverted): a bare string or an array of TEXT blocks
/// → `Vec<Content::Text>`. The wire `system` is a text-only slot (the automatic head
/// cache mark rides it, ignored here); a non-text block has no canonical projection
/// in this slot → rung 4.
pub(super) fn system(v: &Value) -> Result<Vec<Content>, IngressError> {
    match v {
        Value::String(s) => Ok(vec![Content::Text(s.clone())]),
        Value::Array(a) => a
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let path = format!("system[{i}]");
                match str_of(obj_of(Some(b), &path)?.get("type"), &format!("{path}.type"))? {
                    "text" => Ok(Content::Text(
                        str_of(b.get("text"), &format!("{path}.text"))?.to_owned(),
                    )),
                    other => Err(err(format!(
                        "`{path}.type` \"{other}\" has no canonical projection in the text-only system slot"
                    ))),
                }
            })
            .collect(),
        _ => Err(bad("system", "a string or an array of text blocks")),
    }
}

fn is_tool_result(c: &Content) -> bool {
    matches!(c, Content::ToolResult { .. })
}

/// A wire `content` value → canonical parts: a bare string is one `Text` (the
/// string-vs-list distinction dies at decode, arch §3.1); an array maps per block.
fn parts(v: Option<&Value>, path: &str) -> Result<Vec<Content>, IngressError> {
    match v {
        Some(Value::String(s)) => Ok(vec![Content::Text(s.clone())]),
        Some(Value::Array(a)) => a
            .iter()
            .enumerate()
            .map(|(i, p)| block(p, &format!("{path}.content[{i}]")))
            .collect(),
        _ => Err(bad(
            &format!("{path}.content"),
            "a string or an array of content blocks",
        )),
    }
}

/// One wire ContentBlockParam → one `Content` (§2.5, inverted). Thinking/redacted/
/// server-tool blocks decode VERBATIM (the dialect carries them natively).
fn block(p: &Value, path: &str) -> Result<Content, IngressError> {
    let obj = obj_of(Some(p), path)?;
    let ty = str_of(obj.get("type"), &format!("{path}.type"))?;
    Ok(match ty {
        "text" => Content::Text(str_of(obj.get("text"), &format!("{path}.text"))?.to_owned()),
        "image" => Content::Image {
            source: image_source(obj.get("source"), &format!("{path}.source"))?,
        },
        "document" => Content::Document {
            source: document_source(obj.get("source"), &format!("{path}.source"))?,
        },
        "tool_use" => Content::ToolUse {
            id: str_of(obj.get("id"), &format!("{path}.id"))?.to_owned(),
            name: str_of(obj.get("name"), &format!("{path}.name"))?.to_owned(),
            input: obj.get("input").cloned().unwrap_or(Value::Null),
            signature: None,
        },
        "tool_result" => Content::ToolResult {
            tool_use_id: str_of(obj.get("tool_use_id"), &format!("{path}.tool_use_id"))?.to_owned(),
            content: tool_result_content(obj.get("content"), path)?,
            is_error: obj.get("is_error") == Some(&Value::Bool(true)),
        },
        "thinking" => Content::Thinking {
            text: str_of(obj.get("thinking"), &format!("{path}.thinking"))?.to_owned(),
            signature: opt_str(obj.get("signature"), &format!("{path}.signature"))?,
            id: None,
            encrypted_content: None,
        },
        "redacted_thinking" => Content::RedactedThinking {
            data: str_of(obj.get("data"), &format!("{path}.data"))?.to_owned(),
        },
        "server_tool_use" => Content::ServerToolUse {
            id: str_of(obj.get("id"), &format!("{path}.id"))?.to_owned(),
            name: str_of(obj.get("name"), &format!("{path}.name"))?.to_owned(),
            input: obj.get("input").cloned().unwrap_or(Value::Null),
        },
        // The open `*_tool_result` family (§2.5): the tag is carried as `kind` data,
        // suffix-matched exactly as decode does, `content` whole — zero per-tool knowledge.
        t if t.ends_with("_tool_result") => Content::ServerToolResult {
            kind: t.to_owned(),
            tool_use_id: str_of(obj.get("tool_use_id"), &format!("{path}.tool_use_id"))?.to_owned(),
            content: obj.get("content").cloned().unwrap_or(Value::Null),
        },
        other => {
            return Err(err(format!(
                "`{path}.type` \"{other}\" has no canonical projection"
            )))
        }
    })
}

/// `tool_result.content` (§2.5, inverted): a text/image-only slot — a bare string, or
/// an array of text/image blocks. Any other nested block is unrepresentable → rung 4.
fn tool_result_content(v: Option<&Value>, path: &str) -> Result<Vec<Content>, IngressError> {
    match v {
        Some(Value::String(s)) => Ok(vec![Content::Text(s.clone())]),
        Some(Value::Array(a)) => a
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let bp = format!("{path}.content[{i}]");
                let obj = obj_of(Some(b), &bp)?;
                match str_of(obj.get("type"), &format!("{bp}.type"))? {
                    "text" => Ok(Content::Text(
                        str_of(obj.get("text"), &format!("{bp}.text"))?.to_owned(),
                    )),
                    "image" => Ok(Content::Image {
                        source: image_source(obj.get("source"), &format!("{bp}.source"))?,
                    }),
                    other => Err(err(format!(
                        "`{bp}.type` \"{other}\" has no canonical projection in the tool_result slot"
                    ))),
                }
            })
            .collect(),
        _ => Err(bad(
            &format!("{path}.content"),
            "a string or an array of text/image blocks",
        )),
    }
}

/// A wire image `source` → the canonical source (§2.5): `base64` (media_type+data) or
/// `url`. The wire tags on `type`; the canonical enum tags on `kind`.
fn image_source(v: Option<&Value>, path: &str) -> Result<ImageSource, IngressError> {
    let o = obj_of(v, path)?;
    match str_of(o.get("type"), &format!("{path}.type"))? {
        "base64" => Ok(ImageSource::Base64 {
            media_type: str_of(o.get("media_type"), &format!("{path}.media_type"))?.to_owned(),
            data: str_of(o.get("data"), &format!("{path}.data"))?.to_owned(),
        }),
        "url" => Ok(ImageSource::Url {
            url: str_of(o.get("url"), &format!("{path}.url"))?.to_owned(),
        }),
        other => Err(err(format!(
            "`{path}.type` \"{other}\" is not a representable image source"
        ))),
    }
}

/// A wire document `source` → the canonical source (§2.5): `base64` or `url`.
fn document_source(v: Option<&Value>, path: &str) -> Result<DocumentSource, IngressError> {
    let o = obj_of(v, path)?;
    match str_of(o.get("type"), &format!("{path}.type"))? {
        "base64" => Ok(DocumentSource::Base64 {
            media_type: str_of(o.get("media_type"), &format!("{path}.media_type"))?.to_owned(),
            data: str_of(o.get("data"), &format!("{path}.data"))?.to_owned(),
        }),
        "url" => Ok(DocumentSource::Url {
            url: str_of(o.get("url"), &format!("{path}.url"))?.to_owned(),
        }),
        other => Err(err(format!(
            "`{path}.type` \"{other}\" is not a representable document source"
        ))),
    }
}
