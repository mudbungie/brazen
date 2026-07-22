//! `encode` request-shape coverage (anthropic-messages §2): the worked example,
//! the REQUIRED `max_tokens`, `reasoning`, `tool_choice`/`parallel_tool_calls`,
//! and `extra` precedence. The per-`Content`/message projection lives in
//! `anthropic_encode_content`.

use crate::{CanonicalError, CanonicalRequest, ErrorKind, Protocol, ProviderCtx, WireRequest};
use serde_json::{json, Value};

use crate::protocol::anthropic::AnthropicMessages;

/// Encode `req` against a fixed Anthropic-shaped ctx (model + anthropic-version).
fn enc(req: &CanonicalRequest) -> Result<WireRequest, CanonicalError> {
    let beta = [("anthropic-version", "2023-06-01")];
    let ctx = ProviderCtx {
        base_url: "https://api.anthropic.com",
        model: "claude-opus-4-8",
        beta_headers: &beta,
        exec: None,
    };
    AnthropicMessages.encode(req, &ctx)
}

fn from(v: Value) -> CanonicalRequest {
    serde_json::from_value(v).unwrap()
}
fn body(req: &CanonicalRequest) -> Value {
    serde_json::from_slice(&enc(req).unwrap().body).unwrap()
}

#[test]
fn worked_example_projects_every_field_and_header() {
    let req = from(json!({
        "model": "ignored-encode-uses-ctx",
        "system": [{"type":"text","text":"You are a terse weather bot."}],
        "messages": [
            {"role":"user","content":[{"type":"text","text":"Weather in SF?"}]},
            {"role":"assistant","content":[
                {"type":"thinking","text":"think","signature":"EqQBsig"},
                {"type":"tool_use","id":"toolu_01A","name":"get_weather",
                 "input":{"location":"San Francisco, CA"}}
            ]},
            {"role":"tool","content":[
                {"type":"tool_result","tool_use_id":"toolu_01A",
                 "content":[{"type":"text","text":"62F, foggy"}],"is_error":false}
            ]}
        ],
        "tools": [{"name":"get_weather","description":"Look up current weather",
                   "input_schema":{"type":"object",
                     "properties":{"location":{"type":"string"}},"required":["location"]}}],
        "tool_choice": {"type":"auto"},
        "max_tokens": 1024, "temperature": 0.5, "stop": ["\n\nHuman:"], "stream": true,
        "thinking": {"type":"adaptive","display":"summarized"}
    }));
    let wire = enc(&req).unwrap();
    assert_eq!(wire.url, "https://api.anthropic.com/v1/messages");
    // content-type is no longer encode's job — `serve` stamps it from the dialect's
    // one home, `Protocol::content_type()` (bl-da81), so `--raw` carries it too.
    assert_eq!(wire.header("content-type"), None);
    assert_eq!(AnthropicMessages.content_type(), "application/json");
    // beta_headers are no longer encode's job either — `serve` stamps `ctx.beta_headers`
    // for both paths (bl-3e2f), so `--raw` carries anthropic-version too (run_modes pins it).
    assert_eq!(wire.header("anthropic-version"), None);
    assert_eq!(wire.header("x-api-key"), None); // set by Auth, never encode

    let b: Value = serde_json::from_slice(&wire.body).unwrap();
    // The two cache_control marks are the §2.10 AUTO policy: the head mark on the
    // last system block, and — this being an ongoing conversation (an assistant
    // turn before the last message) — the rolling mark on the last block of the
    // last non-assistant wire message (the tool_result turn).
    assert_eq!(
        b,
        json!({
            "model": "claude-opus-4-8",
            "max_tokens": 1024,
            "system": [{"type":"text","text":"You are a terse weather bot.",
                        "cache_control":{"type":"ephemeral"}}],
            "messages": [
                {"role":"user","content":[{"type":"text","text":"Weather in SF?"}]},
                {"role":"assistant","content":[
                    {"type":"thinking","thinking":"think","signature":"EqQBsig"},
                    {"type":"tool_use","id":"toolu_01A","name":"get_weather",
                     "input":{"location":"San Francisco, CA"}}
                ]},
                {"role":"user","content":[
                    {"type":"tool_result","tool_use_id":"toolu_01A",
                     "content":[{"type":"text","text":"62F, foggy"}],
                     "cache_control":{"type":"ephemeral"}}
                ]}
            ],
            "tools": [{"name":"get_weather","description":"Look up current weather",
                       "input_schema":{"type":"object",
                         "properties":{"location":{"type":"string"}},"required":["location"]}}],
            "tool_choice": {"type":"auto"},
            "temperature": 0.5,
            "stop_sequences": ["\n\nHuman:"],
            "stream": true,
            "thinking": {"type":"adaptive","display":"summarized"}
        })
    );
}

#[test]
fn reasoning_projects_extended_thinking_and_couples_max_tokens() {
    // low budget=1024; with the row default max 4096 the floor budget+headroom (5120)
    // wins, so max_tokens bumps. temperature/top_p are OMITTED with thinking (only
    // temperature:1 is accepted) — providers.md §6 / anthropic-messages §2.
    let b = body(&from(json!({
        "model":"x","max_tokens":4096,"temperature":0.7,"top_p":0.5,"reasoning":"low"
    })));
    assert_eq!(
        b["thinking"],
        json!({"type":"enabled","budget_tokens":1024})
    );
    assert_eq!(b["max_tokens"], json!(5120)); // budget(1024) + REASONING_HEADROOM(4096)
    assert!(b.get("temperature").is_none());
    assert!(b.get("top_p").is_none());

    // high budget=24576; default max 4096 floors to 28672 (guarantees max > budget).
    let b = body(&from(
        json!({"model":"x","max_tokens":4096,"reasoning":"high"}),
    ));
    assert_eq!(
        b["thinking"],
        json!({"type":"enabled","budget_tokens":24576})
    );
    assert_eq!(b["max_tokens"], json!(28672));

    // A generous explicit max_tokens above the floor is RESPECTED — no bump.
    let b = body(&from(
        json!({"model":"x","max_tokens":100000,"reasoning":"high"}),
    ));
    assert_eq!(b["max_tokens"], json!(100000));
    assert_eq!(b["thinking"]["budget_tokens"], json!(24576));
}

#[test]
fn reasoning_typed_knob_wins_over_a_body_defaults_thinking_object() {
    // The escape hatch (a raw `thinking` object pinned via body_defaults) rides
    // `extra`; the typed `--reasoning` knob is written before the extra fold, so it
    // WINS on the same key — the two never silently combine (providers §6).
    let b = body(&from(json!({
        "model":"x","max_tokens":4096,"reasoning":"medium",
        "thinking":{"type":"adaptive","display":"summarized"}
    })));
    assert_eq!(
        b["thinking"],
        json!({"type":"enabled","budget_tokens":8192})
    );
}

#[test]
fn max_tokens_is_required_else_config_error() {
    let err = enc(&from(json!({"model":"x"}))).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Config);
    assert_eq!(err.exit_code(), 78);
}

#[test]
fn tool_choice_spellings_and_auto_omitted_without_tools() {
    let tc = |v: Value| {
        body(&from(
            json!({"model":"x","max_tokens":1,"tools":[{"name":"t","input_schema":{}}],
                   "tool_choice":v}),
        ))["tool_choice"]
            .clone()
    };
    assert_eq!(tc(json!({"type":"any"})), json!({"type":"any"}));
    assert_eq!(
        tc(json!({"type":"tool","name":"f"})),
        json!({"type":"tool","name":"f"})
    );
    // Auto with no tools omits the field entirely.
    let b = body(&from(json!({"model":"x","max_tokens":1})));
    assert!(b.get("tool_choice").is_none());
    assert!(b.get("tools").is_none());
}

#[test]
fn parallel_tool_calls_folds_into_tool_choice_object() {
    let fold = |choice: Value, parallel: bool| {
        body(&from(json!({"model":"x","max_tokens":1,
            "tools":[{"name":"t","input_schema":{}}],
            "tool_choice":choice, "parallel_tool_calls":parallel})))["tool_choice"]
            .clone()
    };
    // Some(false) → disable_parallel_tool_use:true NESTED in tool_choice (§2.7), never a
    // top-level body key. Folds onto BOTH auto and any (parallelism is meaningful there).
    assert_eq!(
        fold(json!({"type":"any"}), false),
        json!({"type":"any","disable_parallel_tool_use":true})
    );
    assert_eq!(
        fold(json!({"type":"auto"}), false),
        json!({"type":"auto","disable_parallel_tool_use":true})
    );
    let b = body(&from(json!({"model":"x","max_tokens":1,
        "tools":[{"name":"t","input_schema":{}}],
        "tool_choice":{"type":"any"}, "parallel_tool_calls":false})));
    assert!(b.get("disable_parallel_tool_use").is_none()); // NOT top-level

    // The fold is RESTRICTED to auto/any (§2.7): on `none`/`tool` the field is
    // undocumented/nonsensical (a suppressed or forced single call has no parallelism to
    // disable), so `parallel_tool_calls:false` is INEXPRESSIBLE and DROPS — the object is
    // emitted verbatim, no `disable_parallel_tool_use`.
    assert_eq!(fold(json!({"type":"none"}), false), json!({"type":"none"}));
    assert_eq!(
        fold(json!({"type":"tool","name":"f"}), false),
        json!({"type":"tool","name":"f"})
    );

    // Some(true) is Anthropic's default → no fold, no key (even on auto/any).
    assert_eq!(fold(json!({"type":"any"}), true), json!({"type":"any"}));
    let b = body(&from(json!({"model":"x","max_tokens":1,
        "tools":[{"name":"t","input_schema":{}}], "parallel_tool_calls":true})));
    assert_eq!(b["tool_choice"], json!({"type":"auto"}));

    // No tool_choice object emitted (Auto + no tools) → knob is a no-op, omitted.
    let b = body(&from(
        json!({"model":"x","max_tokens":1,"parallel_tool_calls":false}),
    ));
    assert!(b.get("tool_choice").is_none());
    assert!(b.get("disable_parallel_tool_use").is_none());
}

#[test]
fn extra_merges_top_level_but_typed_fields_win() {
    let b = body(&from(json!({"model":"x","max_tokens":1,
        "stop":["X"], "stop_sequences":["Y"], "metadata":{"user_id":"u"}})));
    assert_eq!(b["stop_sequences"], json!(["X"])); // typed `stop` wins over extra
    assert_eq!(b["metadata"], json!({"user_id":"u"})); // unmodelled key passes through
}

#[test]
fn structured_output_projects_schema_natively_and_narrows_json_mode() {
    // json_schema → `output_config.format` (SCHEMA-ONLY wire, §2.12); `name`/`strict` dropped.
    let b = body(&from(json!({"model":"x","max_tokens":256,"messages":[],
        "output":{"type":"json_schema","name":"Out","schema":{"type":"object"},"strict":true}})));
    assert_eq!(
        b["output_config"],
        json!({"format": {"type": "json_schema", "schema": {"type": "object"}}})
    );
    // `Json` (schemaless) has NO Anthropic spelling → documented NARROWING (omit).
    let b = body(&from(
        json!({"model":"x","max_tokens":256,"messages":[],"output":{"type":"json"}}),
    ));
    assert!(b.get("output_config").is_none());
    // None omits; typed `output` wins over a raw `output_config` passthrough.
    let b = body(&from(json!({"model":"x","max_tokens":256,"messages":[]})));
    assert!(b.get("output_config").is_none());
    let b = body(&from(json!({"model":"x","max_tokens":256,"messages":[],
        "output":{"type":"json_schema","schema":{"type":"object"}},
        "output_config":{"format":{"type":"json_schema","schema":{"raw":true}}}})));
    assert_eq!(
        b["output_config"]["format"]["schema"],
        json!({"type": "object"})
    );
    // A strict custom tool folds `strict` top-level on the tool object (§2.6).
    let b = body(&from(json!({"model":"x","max_tokens":256,"messages":[],
        "tools":[{"name":"f","input_schema":{"type":"object"},"strict":true}]})));
    assert_eq!(b["tools"][0]["strict"], json!(true));
}
