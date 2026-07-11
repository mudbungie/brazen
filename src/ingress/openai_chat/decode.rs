//! Top-level body decode (openai-chat-mapping §2.1, inverted): each known
//! `chat/completions` key lands on its typed canonical field — the single source of
//! truth the egress encoders re-project — and every unknown key rides `extra`
//! verbatim. The `messages[]` fold lives in [`super::messages`].

use serde_json::Value;

use super::{arr_of, bad, err, messages, obj_of, opt_bool, opt_str, str_of};
use crate::canonical::{CanonicalRequest, OutputFormat, ReasoningEffort, Tool, ToolChoice};
use crate::ingress::IngressError;

/// OpenAI `chat/completions` request bytes → `CanonicalRequest` (ingress.md §2).
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
            "tools" => req.tools = tools(&v)?,
            "tool_choice" => req.tool_choice = tool_choice(&v)?,
            "parallel_tool_calls" => {
                req.parallel_tool_calls = Some(v.as_bool().ok_or_else(|| bad(&k, "a boolean"))?);
            }
            // Both wire spellings are the ONE canonical fact; the egress encoder
            // re-picks the key from the `reasoning` signal (openai-chat-mapping §2.7).
            "max_tokens" | "max_completion_tokens" => req.max_tokens = Some(u32_of(&v, &k)?),
            "temperature" => req.temperature = Some(f32_of(&v, &k)?),
            "top_p" => req.top_p = Some(f32_of(&v, &k)?),
            "reasoning_effort" => req.reasoning = Some(effort(&v)?),
            "stop" => req.stop = stop(&v)?,
            "stream" => req.stream = Some(v.as_bool().ok_or_else(|| bad(&k, "a boolean"))?),
            "response_format" => req.output = output(&v)?,
            // The long-tail valve (arch §3.1): unknown top-level keys — including the
            // client's `stream_options`, kept for the response encoder's shape
            // decision — forward verbatim, never rejected (ingress.md §2).
            _ => {
                req.extra.insert(k, v);
            }
        }
    }
    Ok(req)
}

/// A `u32` wire field (the token bound); floats and negatives are shapeless here.
fn u32_of(v: &Value, path: &str) -> Result<u32, IngressError> {
    v.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| bad(path, "an unsigned 32-bit integer"))
}

/// An `f32` wire field (the sampling knobs).
fn f32_of(v: &Value, path: &str) -> Result<f32, IngressError> {
    v.as_f64()
        .map(|f| f as f32)
        .ok_or_else(|| bad(path, "a number"))
}

/// `reasoning_effort` → the closed canonical effort (providers §6). An effort the
/// canonical model cannot represent has no projection → rung 4.
fn effort(v: &Value) -> Result<ReasoningEffort, IngressError> {
    str_of(Some(v), "reasoning_effort")?
        .parse()
        .map_err(|()| bad("reasoning_effort", "one of \"low\" | \"medium\" | \"high\""))
}

/// `stop`: the wire allows a bare string or an array; canonically always the array
/// (the string-vs-list distinction dies at decode, arch §3.1).
fn stop(v: &Value) -> Result<Vec<String>, IngressError> {
    match v {
        Value::String(s) => Ok(vec![s.clone()]),
        Value::Array(a) => a
            .iter()
            .enumerate()
            .map(|(i, s)| Ok(str_of(Some(s), &format!("stop[{i}]"))?.to_owned()))
            .collect(),
        _ => Err(bad("stop", "a string or an array of strings")),
    }
}

/// `tools[]` → `Tool::Custom` (openai-chat-mapping §2.5, inverted), including the
/// lifted `function.strict` knob. Only `"function"` tools have a canonical
/// projection; a missing `type` is tolerated as the unambiguous default.
fn tools(v: &Value) -> Result<Vec<Tool>, IngressError> {
    let mut out = Vec::new();
    for (i, t) in arr_of(Some(v), "tools")?.iter().enumerate() {
        let path = format!("tools[{i}]");
        let obj = obj_of(Some(t), &path)?;
        if let Some(ty) = obj.get("type") {
            if str_of(Some(ty), &format!("{path}.type"))? != "function" {
                return Err(err(format!(
                    "`{path}.type` has no canonical projection: only \"function\" tools are representable"
                )));
            }
        }
        let fp = format!("{path}.function");
        let f = obj_of(obj.get("function"), &fp)?;
        out.push(Tool::Custom {
            name: str_of(f.get("name"), &format!("{fp}.name"))?.to_owned(),
            description: opt_str(f.get("description"), &format!("{fp}.description"))?,
            input_schema: f.get("parameters").cloned().unwrap_or(Value::Null),
            strict: opt_bool(f.get("strict"), &format!("{fp}.strict"))?,
        });
    }
    Ok(out)
}

/// `tool_choice` spellings, inverted (§2.6): `"auto"` → `Auto` (also the default
/// when the key is absent), `"required"` → `Any`, `"none"` → `None`, and the named
/// function object → `Tool{name}`.
fn tool_choice(v: &Value) -> Result<ToolChoice, IngressError> {
    match v {
        Value::String(s) => match s.as_str() {
            "auto" => Ok(ToolChoice::Auto),
            "required" => Ok(ToolChoice::Any),
            "none" => Ok(ToolChoice::None),
            other => Err(err(format!(
                "`tool_choice` \"{other}\" has no canonical projection"
            ))),
        },
        Value::Object(o) => {
            if str_of(o.get("type"), "tool_choice.type")? != "function" {
                return Err(err(
                    "`tool_choice.type` has no canonical projection: only \"function\" is representable",
                ));
            }
            let f = obj_of(o.get("function"), "tool_choice.function")?;
            Ok(ToolChoice::Tool {
                name: str_of(f.get("name"), "tool_choice.function.name")?.to_owned(),
            })
        }
        _ => Err(bad("tool_choice", "a string or an object")),
    }
}

/// `response_format` → the portable `output` knob (§2.5.1, inverted): `"text"` is
/// the wire's spelling of the default → `None`; `json_object` → JSON mode;
/// `json_schema` unwraps chat's nested `{name, schema, strict}` object.
fn output(v: &Value) -> Result<Option<OutputFormat>, IngressError> {
    let o = obj_of(Some(v), "response_format")?;
    match str_of(o.get("type"), "response_format.type")? {
        "text" => Ok(None),
        "json_object" => Ok(Some(OutputFormat::Json)),
        "json_schema" => {
            let js = obj_of(o.get("json_schema"), "response_format.json_schema")?;
            Ok(Some(OutputFormat::JsonSchema {
                name: opt_str(js.get("name"), "response_format.json_schema.name")?,
                schema: js.get("schema").cloned().unwrap_or(Value::Null),
                strict: opt_bool(js.get("strict"), "response_format.json_schema.strict")?,
            }))
        }
        other => Err(err(format!(
            "`response_format.type` \"{other}\" has no canonical projection"
        ))),
    }
}
