//! Top-level body decode (anthropic-messages §2, inverted): each known `POST
//! /v1/messages` key lands on its typed canonical field — the single source of
//! truth the egress encoder re-projects — and every unknown key rides `extra`
//! verbatim. The `messages[]`/`system[]` folds live in [`super::messages`].

use serde_json::Value;

use super::{arr_of, bad, err, f32_of, messages, obj_of, opt_str, str_of, u32_of};
use crate::canonical::{CanonicalRequest, OutputFormat, Tool, ToolChoice};
use crate::ingress::IngressError;

/// Anthropic `POST /v1/messages` request bytes → `CanonicalRequest` (ingress.md §2).
pub(crate) fn decode_request(bytes: &[u8]) -> Result<CanonicalRequest, IngressError> {
    let top: Value =
        serde_json::from_slice(bytes).map_err(|e| err(format!("request body is not JSON: {e}")))?;
    let Value::Object(body) = top else {
        return Err(bad("request body", "a JSON object"));
    };
    let mut req = CanonicalRequest::default();
    for (k, v) in body {
        match k.as_str() {
            "model" => req.model = str_of(Some(&v), "model")?.to_owned(),
            "messages" => messages::fold(&v, &mut req)?,
            "system" => req.system = Some(messages::system(&v)?),
            "tools" => req.tools = tools(&v)?,
            "tool_choice" => tool_choice(&v, &mut req)?,
            "max_tokens" => req.max_tokens = Some(u32_of(&v, &k)?),
            "temperature" => req.temperature = Some(f32_of(&v, &k)?),
            "top_p" => req.top_p = Some(f32_of(&v, &k)?),
            // RENAME inverse: wire `stop_sequences` → canonical `stop` (§2.2).
            "stop_sequences" => req.stop = stop(&v)?,
            "stream" => req.stream = Some(v.as_bool().ok_or_else(|| bad(&k, "a boolean"))?),
            "output_config" => req.output = output(&v)?,
            // The long-tail valve (arch §3.1): unknown top-level keys — the wire
            // `thinking` knob, `metadata`, `top_k`, `service_tier`, `container`, … —
            // forward verbatim, never rejected (ingress.md §2). `thinking` has no clean
            // `budget→effort` inverse, so it rides here (req.reasoning stays None, §5).
            _ => {
                req.extra.insert(k, v);
            }
        }
    }
    Ok(req)
}

/// `stop_sequences`: the wire allows a bare string or an array; canonically always
/// the array (the string-vs-list distinction dies at decode, arch §3.1).
fn stop(v: &Value) -> Result<Vec<String>, IngressError> {
    match v {
        Value::String(s) => Ok(vec![s.clone()]),
        Value::Array(a) => a
            .iter()
            .enumerate()
            .map(|(i, s)| Ok(str_of(Some(s), &format!("stop_sequences[{i}]"))?.to_owned()))
            .collect(),
        _ => Err(bad("stop_sequences", "a string or an array of strings")),
    }
}

/// `tools[]` → the canonical two-variant enum (§2.6, inverted): a wire object WITH a
/// `type` key is a `Provider` tool (opaque `kind` = the wire `type`, every other key
/// verbatim config); one WITHOUT is a `Custom` tool (name, JSON-Schema `input_schema`,
/// and the lifted `strict` knob) — the same `type`-keyed split the canonical
/// `request_de` makes on egress input.
fn tools(v: &Value) -> Result<Vec<Tool>, IngressError> {
    let mut out = Vec::new();
    for (i, t) in arr_of(Some(v), "tools")?.iter().enumerate() {
        let path = format!("tools[{i}]");
        let obj = obj_of(Some(t), &path)?;
        out.push(match obj.get("type") {
            Some(ty) => {
                let kind = str_of(Some(ty), &format!("{path}.type"))?.to_owned();
                let mut config = obj.clone();
                config.remove("type");
                config.remove("name");
                Tool::Provider {
                    name: str_of(obj.get("name"), &format!("{path}.name"))?.to_owned(),
                    kind,
                    config,
                }
            }
            None => Tool::Custom {
                name: str_of(obj.get("name"), &format!("{path}.name"))?.to_owned(),
                description: opt_str(obj.get("description"), &format!("{path}.description"))?,
                input_schema: obj.get("input_schema").cloned().unwrap_or(Value::Null),
                strict: opt_bool(obj.get("strict"), &format!("{path}.strict"))?,
            },
        });
    }
    Ok(out)
}

/// `tool_choice` (§2.7, inverted): the four wire shapes → the four canonical intents,
/// and the nested `disable_parallel_tool_use: true` → `parallel_tool_calls = Some(false)`
/// (Anthropic's home for the lifted knob — never a top-level key). Absence keeps the
/// canonical default (`Auto`, `parallel_tool_calls = None`).
fn tool_choice(v: &Value, req: &mut CanonicalRequest) -> Result<(), IngressError> {
    let o = obj_of(Some(v), "tool_choice")?;
    req.tool_choice = match str_of(o.get("type"), "tool_choice.type")? {
        "auto" => ToolChoice::Auto,
        "any" => ToolChoice::Any,
        "none" => ToolChoice::None,
        "tool" => ToolChoice::Tool {
            name: str_of(o.get("name"), "tool_choice.name")?.to_owned(),
        },
        other => {
            return Err(err(format!(
                "`tool_choice.type` \"{other}\" has no canonical projection"
            )))
        }
    };
    if o.get("disable_parallel_tool_use") == Some(&Value::Bool(true)) {
        req.parallel_tool_calls = Some(false);
    }
    Ok(())
}

/// `output_config` → the portable `output` knob (§2.12, inverted): Anthropic's wire is
/// SCHEMA-ONLY, so the ONE representable shape is `json_schema` (`name`/`strict` are
/// narrowed out on egress and simply absent here → `None`). Any other format `type`
/// has no canonical projection → rung 4.
fn output(v: &Value) -> Result<Option<OutputFormat>, IngressError> {
    let f = obj_of(
        obj_of(Some(v), "output_config")?.get("format"),
        "output_config.format",
    )?;
    match str_of(f.get("type"), "output_config.format.type")? {
        "json_schema" => Ok(Some(OutputFormat::JsonSchema {
            name: None,
            schema: f.get("schema").cloned().unwrap_or(Value::Null),
            strict: None,
        })),
        other => Err(err(format!(
            "`output_config.format.type` \"{other}\" has no canonical projection"
        ))),
    }
}

/// Optional bool: absent and `null` are one absence.
fn opt_bool(v: Option<&Value>, path: &str) -> Result<Option<bool>, IngressError> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(bad(path, "a boolean")),
    }
}
