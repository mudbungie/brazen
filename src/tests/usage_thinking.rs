//! The thinking split (`Usage::thinking_tokens`, architecture §3.2, bl-2042): one
//! fixture per PROTOCOL SHAPE in BOTH states of the `usage_fold_thinking` knob. The
//! providers disagree about containment on the output side — Google serves the answer
//! BESIDE its thoughts, OpenAI chat/Responses serve a total that CONTAINS them, and
//! Anthropic/Ollama serve no split at all — so the decoder answers once: `output_tokens`
//! is the answer wherever the split is known, and the knob (applied at the one stamp
//! site) folds it back into the industry convention.

use std::io::Cursor;

use crate::protocol::anthropic::AnthropicMessages;
use crate::protocol::google_cloudcode::GoogleCloudCode;
use crate::protocol::google_genai::GoogleGenAi;
use crate::protocol::ollama_chat::OllamaChat;
use crate::protocol::openai::OpenAiChat;
use crate::protocol::openai_responses::OpenAiResponses;
use crate::testing::MockTransport;
use crate::tests::decode_full_support::full;
use crate::tests::run_support::*;
use crate::{parse_args, parse_config, partial_from_env, EnvSnapshot, Event, Protocol, Usage};

fn usage_of(proto: &dyn Protocol, body: &str) -> Usage {
    full(proto, body.as_bytes())
        .0
        .into_iter()
        .find_map(|e| match e {
            Event::Usage(u) => Some(u),
            _ => None,
        })
        .unwrap()
}

/// `(output_tokens, thinking_tokens)` with the knob off, then on.
fn both(u: Usage) -> [(Option<u32>, Option<u32>); 2] {
    let folded = u.clone().fold_thinking();
    [
        (u.output_tokens, u.thinking_tokens),
        (folded.output_tokens, folded.thinking_tokens),
    ]
}

const GOOGLE: &str = r#"{"candidates":[{"content":{"parts":[{"text":"hi"}]},"finishReason":"STOP"}],
    "usageMetadata":{"promptTokenCount":9,"candidatesTokenCount":13,"thoughtsTokenCount":383}}"#;

#[test]
fn google_serves_the_answer_beside_the_thoughts() {
    // The bl-0aa3 probe's own wire numbers: 383 thoughts + 13 answer.
    let expect = [(Some(13), Some(383)), (Some(396), Some(383))];
    assert_eq!(both(usage_of(&GoogleGenAi, GOOGLE)), expect);
    // Cloud Code reuses the genai decoder inside its `response` envelope.
    let wrapped = format!(r#"{{"response":{GOOGLE}}}"#);
    assert_eq!(both(usage_of(&GoogleCloudCode, &wrapped)), expect);
}

#[test]
fn openai_chat_subtracts_the_contained_reasoning() {
    let u = usage_of(
        &OpenAiChat,
        r#"{"choices":[{"message":{"content":"hi"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":9,"completion_tokens":396,
                     "completion_tokens_details":{"reasoning_tokens":383}}}"#,
    );
    assert_eq!(both(u), [(Some(13), Some(383)), (Some(396), Some(383))]);
}

#[test]
fn openai_responses_subtracts_the_contained_reasoning() {
    let u = usage_of(
        &OpenAiResponses,
        r#"{"output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hi"}]}],
            "status":"completed",
            "usage":{"input_tokens":9,"output_tokens":396,
                     "output_tokens_details":{"reasoning_tokens":383}}}"#,
    );
    assert_eq!(both(u), [(Some(13), Some(383)), (Some(396), Some(383))]);
}

#[test]
fn a_dialect_with_no_split_keeps_its_number_and_the_knob_is_the_identity() {
    // Anthropic and Ollama count thinking inside the served output with no slice:
    // `thinking_tokens` is unknown (never 0), and folding has nothing to add.
    let anthropic = usage_of(
        &AnthropicMessages,
        r#"{"id":"m","model":"claude-x","role":"assistant","stop_reason":"end_turn",
            "content":[{"type":"text","text":"hi"}],
            "usage":{"input_tokens":9,"output_tokens":396}}"#,
    );
    let ollama = usage_of(
        &OllamaChat,
        r#"{"model":"llama3.2","message":{"role":"assistant","content":"hi"},
            "done":true,"done_reason":"stop","prompt_eval_count":9,"eval_count":396}"#,
    );
    // An OpenAI-shaped server that omits the details object is the same empty case.
    let bare = usage_of(
        &OpenAiChat,
        r#"{"choices":[{"message":{"content":"hi"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":9,"completion_tokens":396}}"#,
    );
    for u in [anthropic, ollama, bare] {
        assert_eq!(both(u), [(Some(396), None), (Some(396), None)]);
    }
}

#[test]
fn an_absent_split_is_omitted_from_the_json_projection() {
    let line = serde_json::to_string(&Event::Usage(Usage::default())).unwrap();
    assert!(!line.contains("thinking_tokens"), "{line}");
    let split = Usage {
        thinking_tokens: Some(383),
        ..Default::default()
    };
    let line = serde_json::to_string(&Event::Usage(split)).unwrap();
    assert!(line.contains(r#""thinking_tokens":383"#), "{line}");
}

/// An OpenAI non-stream body whose 396 completion tokens contain 383 reasoning.
const OPENAI_BODY: &[u8] = br#"{"id":"c","model":"gpt-x","choices":[{"index":0,"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],
    "usage":{"prompt_tokens":9,"completion_tokens":396,"total_tokens":405,
             "completion_tokens_details":{"reasoning_tokens":383}}}"#;

/// The one `usage` line of a `--json` run against the OpenAI body.
fn json_usage(extra: &[&str], env: &[(&str, &str)]) -> serde_json::Value {
    let mut argv = vec![
        "--json",
        "--no-stream",
        "--provider",
        "openai",
        "-m",
        "gpt-x",
        "--api-key",
        "sk",
    ];
    argv.extend_from_slice(extra);
    argv.push("hi");
    let tx = MockTransport::ok(vec![OPENAI_BODY]);
    let o = go(&argv, env, b"", &tx, &empty_store());
    assert_eq!(o.code, 0, "{} {}", o.stdout, o.stderr);
    let line = o.stdout.lines().find(|l| l.contains(r#""type":"usage""#));
    serde_json::from_str(line.unwrap()).unwrap()
}

#[test]
fn the_stamp_site_applies_the_knob_from_flag_or_env() {
    let off = json_usage(&[], &[]);
    assert_eq!(
        (
            off["output_tokens"].as_u64(),
            off["thinking_tokens"].as_u64()
        ),
        (Some(13), Some(383))
    );
    for on in [
        json_usage(&["--usage-fold-thinking"], &[]),
        json_usage(&[], &[("BRAZEN_USAGE_FOLD_THINKING", "true")]),
    ] {
        assert_eq!(
            (on["output_tokens"].as_u64(), on["thinking_tokens"].as_u64()),
            (Some(396), Some(383))
        );
    }
}

#[test]
fn the_knob_is_spelled_on_all_three_layers() {
    let flags = parse_args(&["--usage-fold-thinking".to_string()]).unwrap();
    assert_eq!(flags.config.usage_fold_thinking, Some(true));
    let file = parse_config("usage_fold_thinking = true").unwrap();
    assert_eq!(file.usage_fold_thinking, Some(true));
    let env =
        |v: &str| EnvSnapshot([("BRAZEN_USAGE_FOLD_THINKING".to_string(), v.to_string())].into());
    assert_eq!(
        partial_from_env(&env("false")).unwrap().usage_fold_thinking,
        Some(false)
    );
    assert!(partial_from_env(&env("maybe")).is_err());
}

#[test]
fn a_masquerade_speaks_the_clients_accounting_whatever_the_knob() {
    // An OpenAI-dialect client counts reasoning inside `completion_tokens`, and reads
    // the split from `completion_tokens_details` — forced on, even with the knob unset.
    let cfg = temp("api_key = \"sk\"\n");
    // The masquerade streams upstream (ingress §10), so the same numbers arrive as SSE.
    let sse: &[u8] = br#"data: {"id":"c","model":"gpt-x","choices":[{"index":0,"delta":{"role":"assistant","content":"hi"},"finish_reason":null}]}

data: {"id":"c","model":"gpt-x","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}

data: {"id":"c","model":"gpt-x","choices":[],"usage":{"prompt_tokens":9,"completion_tokens":396,"total_tokens":405,"completion_tokens_details":{"reasoning_tokens":383}}}

data: [DONE]

"#;
    let tx = MockTransport::ok(vec![sse]);
    let o = go_reader(
        &["--in", "openai_chat", "--config", cfg.0.to_str().unwrap()],
        &[],
        &mut Cursor::new(
            br#"{"model":"gpt-x","messages":[{"role":"user","content":"hi"}]}"#.to_vec(),
        ),
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 0, "{} {}", o.stdout, o.stderr);
    let body: serde_json::Value = serde_json::from_str(&o.stdout).unwrap();
    assert_eq!(body["usage"]["completion_tokens"], 396);
    assert_eq!(
        body["usage"]["completion_tokens_details"]["reasoning_tokens"],
        383
    );
    assert_eq!(body["usage"]["total_tokens"], 405);
}
