//! The wire serde for the externally-tagged event vocabulary (§3.2): kept apart
//! from the type defs in `event.rs`, mirroring the request.rs/request_de.rs split.
//! `ServerToolResult` carries a DYNAMIC tag: it serializes as a one-entry map
//! keyed by its `kind` (`"kind":{"web_search_tool_result":{…}}`) and any tag with
//! the `_tool_result` suffix (except the client `tool_result`) decodes back to it
//! — the open-set rule applied to result blocks.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use super::event::{ContentKind, Delta, FinishReason};

/// The single key of an externally-tagged object (`None` if it is not a
/// one-key object), used to dispatch the tag on decode.
fn tag_of(v: &Value) -> Option<&str> {
    v.as_object()?.keys().next().map(String::as_str)
}

/// A string field of `v` (the empty string if absent/non-string — known
/// variants always carry it; the lenient path only meets our own output).
fn str_at(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}

impl Serialize for ContentKind {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // Known fixed-tag variants delegate to a derived externally-tagged enum
        // (the byte-identical `{tag: body}` shape; an empty struct variant renders
        // `{}`, not `null`); `Other` re-emits its `Value`. `ServerToolResult`'s tag
        // is its `kind`, so it hand-rolls the one-entry map — the dynamic tag in
        // exactly the position the fixed tags occupy.
        #[derive(Serialize)]
        #[serde(rename_all = "snake_case")]
        enum Wire<'a> {
            Text {},
            ToolUse {
                id: &'a str,
                name: &'a str,
            },
            // `id` omitted when None → `{"thinking":{}}` (byte-compat, bl-61a9).
            Thinking {
                #[serde(skip_serializing_if = "Option::is_none")]
                id: Option<&'a str>,
            },
            RedactedThinking {
                data: &'a str,
            },
            ServerToolUse {
                id: &'a str,
                name: &'a str,
            },
        }
        #[derive(Serialize)]
        struct SrvResult<'a> {
            tool_use_id: &'a str,
            content: &'a Value,
        }
        match self {
            ContentKind::Text {} => Wire::Text {}.serialize(s),
            ContentKind::ToolUse { id, name } => Wire::ToolUse { id, name }.serialize(s),
            ContentKind::Thinking { id } => Wire::Thinking { id: id.as_deref() }.serialize(s),
            ContentKind::RedactedThinking { data } => Wire::RedactedThinking { data }.serialize(s),
            ContentKind::ServerToolUse { id, name } => {
                Wire::ServerToolUse { id, name }.serialize(s)
            }
            ContentKind::ServerToolResult {
                kind,
                tool_use_id,
                content,
            } => s.collect_map(std::iter::once((
                kind.as_str(),
                SrvResult {
                    tool_use_id,
                    content,
                },
            ))),
            ContentKind::Other(v) => v.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for ContentKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        // Guard order is load-bearing: fixed tags first, then the open-set
        // `*_tool_result` suffix rule, then the forward-compat `Other` catch-all.
        Ok(match tag_of(&v) {
            Some("text") => ContentKind::Text {},
            Some("tool_use") => ContentKind::ToolUse {
                id: str_at(&v["tool_use"], "id"),
                name: str_at(&v["tool_use"], "name"),
            },
            Some("thinking") => ContentKind::Thinking {
                id: v["thinking"]["id"].as_str().map(str::to_owned),
            },
            Some("redacted_thinking") => ContentKind::RedactedThinking {
                data: str_at(&v["redacted_thinking"], "data"),
            },
            Some("server_tool_use") => ContentKind::ServerToolUse {
                id: str_at(&v["server_tool_use"], "id"),
                name: str_at(&v["server_tool_use"], "name"),
            },
            Some(tag) if tag.ends_with("_tool_result") && tag != "tool_result" => {
                ContentKind::ServerToolResult {
                    kind: tag.to_owned(),
                    tool_use_id: str_at(&v[tag], "tool_use_id"),
                    content: v[tag]["content"].clone(),
                }
            }
            _ => ContentKind::Other(v),
        })
    }
}

impl Serialize for Delta {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // A derived externally-tagged newtype enum renders `{"text_delta":"…"}` (byte-
        // identical, `?`-free); short names + renames carry the tags, no extra allow.
        #[derive(Serialize)]
        enum Wire<'a> {
            #[serde(rename = "text_delta")]
            Text(&'a str),
            #[serde(rename = "json_delta")]
            Json(&'a str),
            #[serde(rename = "thinking_delta")]
            Thinking(&'a str),
            #[serde(rename = "signature_delta")]
            Signature(&'a str),
            #[serde(rename = "encrypted_reasoning_delta")]
            EncryptedReasoning(&'a str),
        }
        match self {
            Delta::TextDelta(t) => Wire::Text(t).serialize(s),
            Delta::JsonDelta(t) => Wire::Json(t).serialize(s),
            Delta::ThinkingDelta(t) => Wire::Thinking(t).serialize(s),
            Delta::SignatureDelta(t) => Wire::Signature(t).serialize(s),
            Delta::EncryptedReasoningDelta(t) => Wire::EncryptedReasoning(t).serialize(s),
            Delta::Other(v) => v.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for Delta {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        Ok(match tag_of(&v) {
            Some("text_delta") => Delta::TextDelta(str_at(&v, "text_delta")),
            Some("json_delta") => Delta::JsonDelta(str_at(&v, "json_delta")),
            Some("thinking_delta") => Delta::ThinkingDelta(str_at(&v, "thinking_delta")),
            Some("signature_delta") => Delta::SignatureDelta(str_at(&v, "signature_delta")),
            Some("encrypted_reasoning_delta") => {
                Delta::EncryptedReasoningDelta(str_at(&v, "encrypted_reasoning_delta"))
            }
            _ => Delta::Other(v),
        })
    }
}

impl Serialize for FinishReason {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // Derived structs pin field order without `preserve_order` (a `Map` sorts keys)
        // and stay `?`-free; every variant but `Refusal` is the bare `{"reason": …}`.
        #[derive(Serialize)]
        struct Reason<'a> {
            reason: &'a str,
        }
        #[derive(Serialize)]
        struct Refusal<'a> {
            reason: &'a str,
            category: &'a str,
            explanation: &'a Option<String>,
        }
        let reason = match self {
            FinishReason::Stop => "stop",
            FinishReason::Length => "length",
            FinishReason::ToolUse => "tool_use",
            FinishReason::StopSequence => "stop_sequence",
            FinishReason::Pause => "pause",
            FinishReason::Other(reason) => reason.as_str(),
            FinishReason::Refusal {
                category,
                explanation,
            } => {
                return Refusal {
                    reason: "refusal",
                    category,
                    explanation,
                }
                .serialize(s)
            }
        };
        Reason { reason }.serialize(s)
    }
}

impl<'de> Deserialize<'de> for FinishReason {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            reason: String,
            #[serde(default)]
            category: Option<String>,
            #[serde(default)]
            explanation: Option<String>,
        }
        let raw = Raw::deserialize(d)?;
        Ok(match raw.reason.as_str() {
            "stop" => FinishReason::Stop,
            "length" => FinishReason::Length,
            "tool_use" => FinishReason::ToolUse,
            "stop_sequence" => FinishReason::StopSequence,
            "pause" => FinishReason::Pause,
            "refusal" => FinishReason::Refusal {
                category: raw.category.unwrap_or_default(),
                explanation: raw.explanation,
            },
            _ => FinishReason::Other(raw.reason),
        })
    }
}
