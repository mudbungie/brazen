//! The decode-side stash join (ingress.md §5, §14): hit → re-injection (thinking
//! blocks lead the turn, tool signatures restored by id), miss → fail-open with
//! the `thinking_replay` adaptation exactly when the upstream would need the
//! payload (a tool continuation on a reasoning request), reject-override →
//! rung-4, and the fail-open tolerance for unparseable or un-injectable payloads.

use serde_json::json;

use crate::ingress::{reinject, THINKING_REPLAY};
use crate::store::{content_key, ReplayStash};
use crate::testing::FakeClock;
use crate::{CanonicalRequest, Content, Message, ReasoningEffort, Role};

fn stash_in(dir: &tempfile::TempDir) -> ReplayStash {
    ReplayStash::new(dir.path())
}

fn tool_use(id: &str) -> Content {
    Content::ToolUse {
        id: id.into(),
        name: "get_weather".into(),
        input: json!({}),
        signature: None,
    }
}

fn assistant(content: Vec<Content>) -> Message {
    Message {
        role: Role::Assistant,
        content,
    }
}

/// A request with one assistant turn and (optionally) the reasoning knob set.
fn req(turn: Vec<Content>, reasoning: bool) -> CanonicalRequest {
    CanonicalRequest {
        reasoning: reasoning.then_some(ReasoningEffort::High),
        messages: vec![
            Message {
                role: Role::User,
                content: vec![Content::Text("hi".into())],
            },
            assistant(turn),
        ],
        ..CanonicalRequest::default()
    }
}

fn thinking(text: &str, sig: &str) -> Content {
    Content::Thinking {
        text: text.into(),
        signature: Some(sig.into()),
        id: None,
        encrypted_content: None,
    }
}

#[test]
fn a_tool_id_hit_prepends_the_thinking_blocks_in_stash_order() {
    let tmp = tempfile::tempdir().unwrap();
    let stash = stash_in(&tmp);
    let payload = serde_json::to_vec(&vec![
        thinking("Let me check.", "sig1"),
        Content::RedactedThinking {
            data: "opaque".into(),
        },
    ])
    .unwrap();
    stash
        .stash("toolu_01A", &payload, &FakeClock::new(0))
        .unwrap();

    let mut r = req(
        vec![Content::Text("On it.".into()), tool_use("toolu_01A")],
        true,
    );
    let fired = reinject(&mut r, &stash, false).unwrap();
    assert!(fired.is_empty(), "a hit fires no adaptation");
    let turn = &r.messages[1].content;
    assert_eq!(turn.len(), 4);
    assert_eq!(turn[0], thinking("Let me check.", "sig1"), "thinking leads");
    assert_eq!(
        turn[1],
        Content::RedactedThinking {
            data: "opaque".into()
        },
        "stash (wire) order is preserved"
    );
    assert!(matches!(&turn[2], Content::Text(t) if t == "On it."));
    assert!(
        matches!(&turn[3], Content::ToolUse { .. }),
        "before its tool call"
    );
}

#[test]
fn any_echoed_tool_id_recalls_and_signatures_restore_by_id() {
    let tmp = tempfile::tempdir().unwrap();
    let stash = stash_in(&tmp);
    // Stashed under the SECOND id only — any echoed id must join (§5).
    let payload = serde_json::to_vec(&vec![Content::ToolUse {
        id: "toolu_02B".into(),
        name: "get_weather".into(),
        input: json!({}),
        signature: Some("thought-sig".into()),
    }])
    .unwrap();
    stash
        .stash("toolu_02B", &payload, &FakeClock::new(0))
        .unwrap();

    // A text part rides along: the restore walk skips it untouched.
    let mut r = req(
        vec![
            Content::Text("calling".into()),
            tool_use("toolu_01A"),
            tool_use("toolu_02B"),
        ],
        false,
    );
    assert_eq!(
        reinject(&mut r, &stash, false).unwrap(),
        Vec::<String>::new()
    );
    let turn = &r.messages[1].content;
    assert!(
        matches!(&turn[1], Content::ToolUse { id, signature: None, .. } if id == "toolu_01A"),
        "the other call keeps its absent signature"
    );
    assert!(
        matches!(&turn[2], Content::ToolUse { id, signature: Some(s), .. }
            if id == "toolu_02B" && s == "thought-sig"),
        "the Google thoughtSignature lands back on ITS call"
    );
}

#[test]
fn a_non_tool_turn_joins_on_the_content_hash() {
    let tmp = tempfile::tempdir().unwrap();
    let stash = stash_in(&tmp);
    let payload = serde_json::to_vec(&vec![thinking("hmm", "sig2")]).unwrap();
    // The key is the concatenation of the turn's text parts, no separator —
    // exactly how the encoder accumulated it (§5).
    let key = content_key("The answer is 42.");
    stash.stash(&key, &payload, &FakeClock::new(0)).unwrap();

    // A non-text part in the turn does not perturb the key (only text hashes).
    let mut r = req(
        vec![
            Content::Text("The answer ".into()),
            Content::RedactedThinking { data: "x".into() },
            Content::Text("is 42.".into()),
        ],
        true,
    );
    assert!(reinject(&mut r, &stash, false).unwrap().is_empty());
    assert_eq!(r.messages[1].content[0], thinking("hmm", "sig2"));
}

#[test]
fn a_miss_on_a_reasoning_tool_turn_adapts_once_and_is_named() {
    let tmp = tempfile::tempdir().unwrap();
    let stash = stash_in(&tmp);
    // TWO missing tool turns → ONE adaptation name (the exposure names the
    // adaptation, not each degraded turn).
    let mut r = req(vec![tool_use("toolu_gone")], true);
    r.messages
        .push(assistant(vec![tool_use("toolu_also_gone")]));
    let fired = reinject(&mut r, &stash, false).unwrap();
    assert_eq!(fired, vec![THINKING_REPLAY.to_owned()]);
    assert_eq!(
        r.messages[1].content.len(),
        1,
        "the degraded turn is untouched"
    );
}

#[test]
fn a_miss_without_reasoning_or_without_tools_stays_silent() {
    let tmp = tempfile::tempdir().unwrap();
    let stash = stash_in(&tmp);
    // Tool-bearing but no reasoning asked: nothing was ever required (§5).
    let mut plain_tools = req(vec![tool_use("toolu_gone")], false);
    assert!(reinject(&mut plain_tools, &stash, false)
        .unwrap()
        .is_empty());
    // Reasoning asked but a text-only turn: no upstream requires the payload.
    let mut text_only = req(vec![Content::Text("hi there".into())], true);
    assert!(reinject(&mut text_only, &stash, false).unwrap().is_empty());
}

#[test]
fn the_reject_override_collapses_the_miss_to_rung_four() {
    let tmp = tempfile::tempdir().unwrap();
    let stash = stash_in(&tmp);
    let mut r = req(vec![tool_use("toolu_gone")], true);
    let err = reinject(&mut r, &stash, true).unwrap_err();
    assert!(err.message.contains("toolu_gone"), "{}", err.message);
    assert!(err.message.contains("reject"), "{}", err.message);
}

#[test]
fn reject_never_fires_on_a_hit_or_a_silent_miss() {
    let tmp = tempfile::tempdir().unwrap();
    let stash = stash_in(&tmp);
    let payload = serde_json::to_vec(&vec![thinking("t", "s")]).unwrap();
    stash
        .stash("toolu_01A", &payload, &FakeClock::new(0))
        .unwrap();
    let mut hit = req(vec![tool_use("toolu_01A")], true);
    assert!(reinject(&mut hit, &stash, true).is_ok());
    let mut silent = req(vec![tool_use("toolu_gone")], false);
    assert!(
        reinject(&mut silent, &stash, true).is_ok(),
        "no reasoning, no requirement"
    );
}

#[test]
fn an_unparseable_payload_is_a_miss_and_uninjectable_blocks_drop() {
    let tmp = tempfile::tempdir().unwrap();
    let stash = stash_in(&tmp);
    let clock = FakeClock::new(0);
    // Garbage bytes under the key: parses as nothing → the fail-open miss path,
    // which (reasoning + tools) fires the adaptation instead of erroring.
    stash.stash("toolu_01A", b"not json", &clock).unwrap();
    let mut r = req(vec![tool_use("toolu_01A")], true);
    assert_eq!(
        reinject(&mut r, &stash, false).unwrap(),
        vec![THINKING_REPLAY.to_owned()]
    );

    // A payload holding blocks with no re-injection slot (text; a signature-less
    // tool echo) injects nothing and drops them silently (fail-open).
    let odd =
        serde_json::to_vec(&vec![Content::Text("stray".into()), tool_use("toolu_02B")]).unwrap();
    stash.stash("toolu_02B", &odd, &clock).unwrap();
    let mut r = req(vec![tool_use("toolu_02B")], true);
    assert!(
        reinject(&mut r, &stash, false).unwrap().is_empty(),
        "a hit, however odd"
    );
    assert_eq!(
        r.messages[1].content.len(),
        1,
        "nothing injected, nothing lost"
    );
}
