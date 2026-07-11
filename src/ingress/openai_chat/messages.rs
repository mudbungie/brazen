//! The `messages[]` fold (openai-chat-mapping §2.2–§2.4, inverted): the LEADING
//! system/developer message → `req.system`; every other wire message → one
//! canonical `Message` — except `role:"tool"`, where consecutive wire messages
//! re-coalesce into ONE `Role::Tool` turn, the exact inverse of the encoder's
//! one-message-per-`ToolResult` fan-out. Content parts invert per slot: text
//! everywhere, `image_url`/`file` only where the wire allows media (user turns);
//! the encoder's data-URI embeddings lift back to base64 sources. Assistant turns
//! stay clean join points for the replay stash (ingress.md §5, sibling ball):
//! tool-call ids and text content decode verbatim, nothing is synthesized.

use serde_json::{json, Map, Value};

use super::{arr_of, bad, err, obj_of, str_of};
use crate::canonical::{CanonicalRequest, Content, DocumentSource, ImageSource, Message, Role};
use crate::ingress::IngressError;

/// Fold the wire `messages` array into `req.system` + `req.messages`.
pub(super) fn fold(v: &Value, req: &mut CanonicalRequest) -> Result<(), IngressError> {
    for (i, m) in arr_of(Some(v), "messages")?.iter().enumerate() {
        let path = format!("messages[{i}]");
        let obj = obj_of(Some(m), &path)?;
        match str_of(obj.get("role"), &format!("{path}.role"))? {
            // "developer" is the o-series spelling of the same slot (§2.3) — rung 1.
            "system" | "developer" => {
                let content = parts(obj.get("content"), &path, false)?;
                if i == 0 {
                    req.system = Some(content); // the leading system message IS `req.system`
                } else {
                    req.messages.push(Message {
                        role: Role::System,
                        content,
                    });
                }
            }
            "user" => req.messages.push(Message {
                role: Role::User,
                content: parts(obj.get("content"), &path, true)?,
            }),
            "assistant" => req.messages.push(assistant(obj, &path)?),
            "tool" => tool_result(obj, &path, &mut req.messages)?,
            other => {
                return Err(err(format!(
                    "`{path}.role` \"{other}\" is not a chat/completions role"
                )));
            }
        }
    }
    Ok(())
}

/// An assistant turn (§2.2, inverted): wire `content` (string | parts | null) plus
/// `tool_calls[]` → `ToolUse` parts appended after the text. The encoder's `""`
/// fabrication for a content-less turn inverts back to no parts; `signature` is
/// `None` — this wire carries no thinking to replay (§2.9).
fn assistant(obj: &Map<String, Value>, path: &str) -> Result<Message, IngressError> {
    let mut content = match obj.get("content") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(s)) if s.is_empty() => Vec::new(),
        c => parts(c, path, false)?,
    };
    if let Some(tc) = obj.get("tool_calls") {
        let tp = format!("{path}.tool_calls");
        for (i, c) in arr_of(Some(tc), &tp)?.iter().enumerate() {
            content.push(tool_use(c, &format!("{tp}[{i}]"))?);
        }
    }
    Ok(Message {
        role: Role::Assistant,
        content,
    })
}

/// One `tool_calls[]` entry → `Content::ToolUse` (§2.2, inverted). `arguments` is
/// the wire's JSON-encoded string, parsed exactly once here: an empty string — a
/// shape real models emit — is the empty object, and a client that already sends
/// an object is accepted verbatim (rung 2, zero loss).
fn tool_use(v: &Value, path: &str) -> Result<Content, IngressError> {
    let obj = obj_of(Some(v), path)?;
    if let Some(ty) = obj.get("type") {
        if str_of(Some(ty), &format!("{path}.type"))? != "function" {
            return Err(err(format!(
                "`{path}.type` has no canonical projection: only \"function\" calls are representable"
            )));
        }
    }
    let fp = format!("{path}.function");
    let f = obj_of(obj.get("function"), &fp)?;
    let input = match f.get("arguments") {
        None => json!({}),
        Some(Value::String(s)) if s.trim().is_empty() => json!({}),
        Some(Value::String(s)) => serde_json::from_str(s).map_err(|e| {
            err(format!(
                "`{fp}.arguments` is not a JSON-encoded string: {e}"
            ))
        })?,
        Some(Value::Object(o)) => Value::Object(o.clone()),
        Some(_) => return Err(bad(&format!("{fp}.arguments"), "a JSON-encoded string")),
    };
    Ok(Content::ToolUse {
        id: str_of(obj.get("id"), &format!("{path}.id"))?.to_owned(),
        name: str_of(f.get("name"), &format!("{fp}.name"))?.to_owned(),
        input,
        signature: None,
    })
}

/// A `role:"tool"` wire message → a `ToolResult`, coalesced onto the previous
/// canonical `Role::Tool` turn when consecutive (the §2.4 fan-out, inverted, so
/// decode∘encode is identity). The `"[error] "` content prefix is the encoder's
/// textual `is_error` (§2.4, CR-3), lifted back to the structural flag.
fn tool_result(
    obj: &Map<String, Value>,
    path: &str,
    messages: &mut Vec<Message>,
) -> Result<(), IngressError> {
    let id = str_of(obj.get("tool_call_id"), &format!("{path}.tool_call_id"))?;
    let (content, is_error) = match obj.get("content") {
        Some(Value::String(s)) => match s.strip_prefix("[error] ") {
            Some(rest) => (vec![Content::Text(rest.to_owned())], true),
            None => (vec![Content::Text(s.clone())], false),
        },
        c => (parts(c, path, false)?, false),
    };
    let result = Content::ToolResult {
        tool_use_id: id.to_owned(),
        content,
        is_error,
    };
    match messages.last_mut() {
        Some(m) if m.role == Role::Tool => m.content.push(result),
        _ => messages.push(Message {
            role: Role::Tool,
            content: vec![result],
        }),
    }
    Ok(())
}

/// A wire `content` value → canonical parts: a bare string is one `Text` (the
/// string-vs-list distinction dies at decode, arch §3.1); an array maps per part.
/// `media` gates image/file parts to the slots whose wire allows them (user turns);
/// in a text-only slot they fall through to the named rung-4 rejection.
fn parts(v: Option<&Value>, path: &str, media: bool) -> Result<Vec<Content>, IngressError> {
    match v {
        Some(Value::String(s)) => Ok(vec![Content::Text(s.clone())]),
        Some(Value::Array(a)) => a
            .iter()
            .enumerate()
            .map(|(i, p)| part(p, &format!("{path}.content[{i}]"), media))
            .collect(),
        _ => Err(bad(
            &format!("{path}.content"),
            "a string or an array of content parts",
        )),
    }
}

/// One content part → one `Content` (§2.2, inverted).
fn part(p: &Value, path: &str, media: bool) -> Result<Content, IngressError> {
    let obj = obj_of(Some(p), path)?;
    match str_of(obj.get("type"), &format!("{path}.type"))? {
        "text" => Ok(Content::Text(
            str_of(obj.get("text"), &format!("{path}.text"))?.to_owned(),
        )),
        "image_url" if media => {
            let o = obj_of(obj.get("image_url"), &format!("{path}.image_url"))?;
            let url = str_of(o.get("url"), &format!("{path}.image_url.url"))?;
            Ok(Content::Image {
                source: image_source(url),
            })
        }
        "file" if media => document(obj.get("file"), path),
        other => Err(err(format!(
            "`{path}.type` \"{other}\" has no canonical projection in this slot"
        ))),
    }
}

/// An `image_url.url` → the canonical source: the encoder's data-URI embedding
/// (`data:{mt};base64,{data}`, §6 CR-1) lifts back to `Base64`; anything else —
/// including a non-base64 data URI — passes through as `Url` verbatim (carry the
/// spec: the upstream is the authority on what it will fetch).
fn image_source(url: &str) -> ImageSource {
    match data_uri(url) {
        Some((media_type, data)) => ImageSource::Base64 { media_type, data },
        None => ImageSource::Url {
            url: url.to_owned(),
        },
    }
}

/// Split the exact `data:{media_type};base64,{data}` shape the encoder emits.
fn data_uri(url: &str) -> Option<(String, String)> {
    let (media_type, data) = url.strip_prefix("data:")?.split_once(";base64,")?;
    Some((media_type.to_owned(), data.to_owned()))
}

/// A `file` part → `Content::Document` (§2.2, inverted): only base64 `file_data`
/// (the encoder's data-URI) is representable — an uploaded `file_id` lives on the
/// provider and has no canonical slot → rung 4. The `filename` is dropped: it is
/// the encoder's own synthesis (§2.2), a fabrication, not a fact.
fn document(v: Option<&Value>, path: &str) -> Result<Content, IngressError> {
    let f = obj_of(v, &format!("{path}.file"))?;
    let data = str_of(f.get("file_data"), &format!("{path}.file.file_data")).map_err(|_| {
        err(format!(
            "`{path}.file` has no canonical projection without base64 `file_data` — an uploaded `file_id` cannot be carried"
        ))
    })?;
    let Some((media_type, data)) = data_uri(data) else {
        return Err(bad(
            &format!("{path}.file.file_data"),
            "a data:{media_type};base64,{data} URI",
        ));
    };
    Ok(Content::Document {
        source: DocumentSource::Base64 { media_type, data },
    })
}
