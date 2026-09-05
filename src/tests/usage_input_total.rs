//! The canonical prompt total (`Usage::input_total_tokens`, architecture §3.2): one
//! fixture per PROTOCOL SHAPE, pinning the containment rule the counters themselves
//! cannot state. The providers disagree — Anthropic reports its cache slices BESIDE
//! `input_tokens`, while OpenAI chat/Responses and Google report a prompt counter that
//! already CONTAINS the cached slice — so `input + output + cache_read + cache_write`
//! is right on one dialect and double-bills the cache on three (bl-d192). Here the
//! same wire numbers are fed to each decoder and the total is asserted.

use crate::protocol::anthropic::AnthropicMessages;
use crate::protocol::google_genai::GoogleGenAi;
use crate::protocol::ollama_chat::OllamaChat;
use crate::protocol::openai::OpenAiChat;
use crate::protocol::openai_responses::OpenAiResponses;
use crate::tests::decode_full_support::full;
use crate::{Event, Protocol, Usage};

/// The one `Usage` a non-stream body folds to.
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

/// 91,648 of a 93,556-token prompt served from cache — the ball's own measurement,
/// where the naive four-counter sum billed 185,336 for 93,688 tokens of real work.
#[test]
fn a_contained_prompt_counter_is_the_total_unchanged() {
    let openai = usage_of(
        &OpenAiChat,
        r#"{"choices":[{"message":{"content":"hi"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":93556,"completion_tokens":132,
                     "prompt_tokens_details":{"cached_tokens":91648}}}"#,
    );
    assert_eq!(openai.input_tokens, Some(93_556)); // the provider's own number, untouched
    assert_eq!(openai.cache_read_tokens, Some(91_648)); // a SLICE of it, not a sum beside it
    assert_eq!(openai.input_total_tokens, Some(93_556));

    let responses = usage_of(
        &OpenAiResponses,
        r#"{"output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hi"}]}],
            "status":"completed",
            "usage":{"input_tokens":93556,"output_tokens":132,
                     "input_tokens_details":{"cached_tokens":91648}}}"#,
    );
    assert_eq!(responses.input_total_tokens, Some(93_556));

    let google = usage_of(
        &GoogleGenAi,
        r#"{"candidates":[{"content":{"parts":[{"text":"hi"}]},"finishReason":"STOP"}],
            "usageMetadata":{"promptTokenCount":93556,"candidatesTokenCount":132,
                             "cachedContentTokenCount":91648}}"#,
    );
    assert_eq!(google.input_total_tokens, Some(93_556));
}

#[test]
fn a_disjoint_prompt_counter_adds_its_cache_slices_back() {
    // Anthropic documents `input_tokens` as the tokens "which were not read from or
    // used to create a cache" — the three fields are mutually exclusive segments, so
    // the same 93,556-token prompt arrives split and the total is their sum.
    let u = usage_of(
        &AnthropicMessages,
        r#"{"id":"msg_1","model":"claude-opus-4-8","role":"assistant","stop_reason":"end_turn",
            "content":[{"type":"text","text":"hi"}],
            "usage":{"input_tokens":1908,"output_tokens":132,
                     "cache_read_input_tokens":91648,"cache_creation_input_tokens":0}}"#,
    );
    assert_eq!(u.input_tokens, Some(1_908)); // the UNCACHED remainder, untouched
    assert_eq!(u.input_total_tokens, Some(93_556)); // the same prompt as above
}

#[test]
fn a_dialect_with_no_cache_counters_is_the_empty_case_of_the_same_rule() {
    // Ollama reports neither slice, so "add them back" and "leave it alone" coincide —
    // the general path with empty inputs, not a third rule.
    let u = usage_of(
        &OllamaChat,
        r#"{"model":"llama3.2","message":{"role":"assistant","content":"hi"},
            "done":true,"done_reason":"stop","prompt_eval_count":93556,"eval_count":132}"#,
    );
    assert_eq!(u.cache_read_tokens, None);
    assert_eq!(u.input_total_tokens, Some(93_556));
}

#[test]
fn a_counter_nobody_reported_stays_unknown_never_a_fabricated_zero() {
    // Anthropic's `message_delta` reports only `output_tokens`; a total of `0` there
    // would be a lie about the prompt, and summing per-event totals would double-count.
    // The consumer's rule is the one `Usage` has always had: merge per FIELD, last
    // wins, then add `input_total_tokens + output_tokens`.
    assert_eq!(
        Usage::default().with_input_total(true).input_total_tokens,
        None
    );
    assert_eq!(
        Usage {
            output_tokens: Some(2),
            ..Default::default()
        }
        .with_input_total(false)
        .input_total_tokens,
        None
    );
}
