//! The replay stash wired end to end through `--in` (ingress.md §5, §14): the
//! encoder's `take_stash` pairs land in the XDG stash after a thinking turn;
//! the next turn's decode recalls by tool-call id (or content hash) and the
//! upstream wire carries the re-injected thinking block; a miss degrades the
//! knob and EXPOSES `thinking_replay` (§4) — aggregate field and SSE comment —
//! and the reject override collapses it to the rung-4 envelope (64).

use std::io::Cursor;

use crate::store::{content_key, ReplayStash};
use crate::testing::MockTransport;
use crate::tests::run_support::*;
use crate::ModelCache;

const THINKING_TOOLS: &[u8] =
    include_bytes!("../../tests/fixtures/anthropic_messages_thinking_tools.sse");
// The tool-call id / signature fragment / thinking fragment the recorded golden
// (anthropic_fixtures.rs) actually carries — the stash keys on the id, the wire
// replays the signature. Fragments (not the whole ~700-char signature) suffice
// for the `contains` anchors.
const TOOL_ID: &str = "toolu_01XXCPWEWpgnJ8BM4hcmSB3e";
const SIG_FRAGMENT: &str = "EvEDCpMBCA8YAipA62hxLLCBiRkZIzoHRmYxA1x7T8mZrQCOIppkwY4UhZUXlVhnmPP8U";
const THINK_FRAGMENT: &str = "use that exactly as they provided it.";

fn masq_cfg(extra: &str) -> TempFile {
    temp(&format!(
        r#"
api_key = "sk-test"
{extra}
[[provider]]
name = "anthropic"
model_aliases = {{ "gpt-4o" = "claude-x" }}

# The shipped openai row owns gpt-* by prefix; clear it so the alias is the
# ONE owner (the masquerade recipe's disambiguation line).
[[provider]]
name = "openai"
model_prefixes = []
"#,
    ))
}

fn go_in_stashed(
    cfg: &TempFile,
    stdin: &[u8],
    tx: &dyn crate::Transport,
    stash: &ReplayStash,
) -> Out {
    go_stashed(
        &["--in", "openai_chat", "--config", cfg.0.to_str().unwrap()],
        &[],
        &mut Cursor::new(stdin.to_vec()),
        tx,
        &empty_store(),
        &crate::testing::MemoryModelCache::new(),
        stash,
    )
}

/// The §5 continuation request: the assistant turn echoes the tool call the
/// fixture produced, plus the tool result and the reasoning knob.
fn continuation() -> Vec<u8> {
    format!(
        r#"{{"model":"gpt-4o","reasoning_effort":"high","messages":[
        {{"role":"user","content":"weather?"}},
        {{"role":"assistant","tool_calls":[{{"id":"{TOOL_ID}","type":"function",
            "function":{{"name":"get_weather","arguments":"{{\"location\":\"SF\"}}"}}}}]}},
        {{"role":"tool","tool_call_id":"{TOOL_ID}","content":"sunny"}}
    ]}}"#
    )
    .into_bytes()
}

#[test]
fn a_thinking_tool_turn_stashes_under_its_tool_id_and_replays() {
    let tmp = tempfile::tempdir().unwrap();
    let stash = ReplayStash::new(tmp.path());
    let cfg = masq_cfg("");

    // Turn 1: the upstream thinks, signs, and calls a tool — the encoder emits
    // the (key, payload) pairs and the shell writes them through the stash.
    let o = go_in_stashed(
        &cfg,
        br#"{"model":"gpt-4o","messages":[{"role":"user","content":"weather?"}]}"#,
        &MockTransport::ok(vec![THINKING_TOOLS]),
        &stash,
    );
    assert_eq!(o.code, 0, "stderr: {}", o.stderr);
    let entry = stash
        .recall(TOOL_ID)
        .expect("stashed under the tool-call id");
    assert!(String::from_utf8_lossy(&entry).contains(SIG_FRAGMENT));

    // Turn 2: the client echoes the tool id; decode recalls and the UPSTREAM
    // wire carries the thinking block, signature intact, before its tool call.
    let tx = MockTransport::ok(vec![BASIC]);
    let o = go_in_stashed(&cfg, &continuation(), &tx, &stash);
    assert_eq!(o.code, 0, "stderr: {}", o.stderr);
    let body: serde_json::Value = serde_json::from_str(&o.stdout).unwrap();
    assert!(body.get("brazen").is_none(), "a hit fires no adaptation");
    let wire = String::from_utf8_lossy(&tx.requests()[0].body).into_owned();
    let think = wire.find(r#""type":"thinking""#).expect(&wire);
    assert!(
        wire.contains(&format!(r#""signature":"{SIG_FRAGMENT}"#)),
        "{wire}"
    );
    assert!(wire.contains(THINK_FRAGMENT), "{wire}");
    let tool = wire.find(r#""type":"tool_use""#).expect(&wire);
    assert!(think < tool, "thinking precedes its tool call: {wire}");
}

#[test]
fn a_non_tool_thinking_turn_stashes_under_the_content_hash() {
    // A thinking turn WITHOUT tools keys on the hash of the assistant text the
    // client will echo back (§5). Craft the upstream stream inline.
    let sse = b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"role\":\"assistant\",\"model\":\"x\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n\
event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"hmm\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig9\"}}\n\n\
event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"The answer is 42.\"}}\n\n\
event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n\
event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":2}}\n\n\
event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
    let tmp = tempfile::tempdir().unwrap();
    let stash = ReplayStash::new(tmp.path());
    let cfg = masq_cfg("");
    let o = go_in_stashed(
        &cfg,
        br#"{"model":"gpt-4o","messages":[{"role":"user","content":"?"}]}"#,
        &MockTransport::ok(vec![sse]),
        &stash,
    );
    assert_eq!(o.code, 0, "stderr: {}", o.stderr);
    let entry = stash
        .recall(&content_key("The answer is 42."))
        .expect("keyed by text hash");
    assert!(String::from_utf8_lossy(&entry).contains("sig9"));
}

#[test]
fn a_miss_adapts_and_the_aggregate_names_the_adaptation() {
    let tmp = tempfile::tempdir().unwrap();
    let stash = ReplayStash::new(tmp.path()); // empty: every recall misses
    let cfg = masq_cfg("");
    let tx = MockTransport::ok(vec![BASIC]);
    let o = go_in_stashed(&cfg, &continuation(), &tx, &stash);
    assert_eq!(
        o.code, 0,
        "the degraded turn still succeeds (fail-open, §5)"
    );
    let body: serde_json::Value = serde_json::from_str(&o.stdout).unwrap();
    assert_eq!(
        body["brazen"]["adaptations"][0], "thinking_replay",
        "{}",
        o.stdout
    );
    // The degraded request went out WITHOUT a fabricated thinking block.
    let wire = String::from_utf8_lossy(&tx.requests()[0].body).into_owned();
    assert!(!wire.contains(r#""type":"thinking""#), "{wire}");
}

#[test]
fn a_miss_on_the_sse_shape_rides_a_comment_line() {
    let tmp = tempfile::tempdir().unwrap();
    let stash = ReplayStash::new(tmp.path());
    let cfg = masq_cfg("");
    let streamed = String::from_utf8_lossy(&continuation()).replace(
        r#""reasoning_effort":"high""#,
        r#""reasoning_effort":"high","stream":true"#,
    );
    let o = go_in_stashed(
        &cfg,
        streamed.as_bytes(),
        &MockTransport::ok(vec![BASIC]),
        &stash,
    );
    assert_eq!(o.code, 0);
    assert!(
        o.stdout.contains(": brazen adaptation=thinking_replay\n"),
        "the §4 SSE exposure: {}",
        o.stdout
    );
}

#[test]
fn the_reject_override_refuses_the_degraded_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let stash = ReplayStash::new(tmp.path());
    // §11: no [ingress] table is REQUIRED, but a present one's lossy fields are
    // honored — here through `lossy_overrides` alone (no dialect, no listener).
    let cfg = masq_cfg("[ingress]\nlossy_overrides = { thinking_replay = \"reject\" }\n");
    let o = go_in_stashed(&cfg, &continuation(), &ok_basic(), &stash);
    assert_eq!(o.code, 64, "rung 4: ParseInput (§3)");
    let body: serde_json::Value = serde_json::from_str(&o.stdout).unwrap();
    assert_eq!(body["error"]["code"], 400);
    assert!(body["error"]["message"].as_str().unwrap().contains(TOOL_ID));
}

#[test]
fn a_typod_override_name_is_config_78_on_in_too() {
    // The ingress §4 never-silently-inert rule holds on THIS door as well: an
    // unknown adaptation name is Config/78 in the dialect envelope, exactly as
    // `--serve` refuses it at startup — never an inert key that silently
    // leaves the adapt default in force. (The honored-spelling twin is
    // `the_reject_override_refuses_the_degraded_turn` above.)
    let tmp = tempfile::tempdir().unwrap();
    let stash = ReplayStash::new(tmp.path());
    let cfg = masq_cfg("[ingress]\nlossy_overrides = { thinking_reply = \"reject\" }\n");
    let o = go_in_stashed(
        &cfg,
        br#"{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}]}"#,
        &ok_basic(),
        &stash,
    );
    assert_eq!(o.code, 78, "stdout: {} stderr: {}", o.stdout, o.stderr);
    assert!(o.stderr.is_empty(), "in-band per §9, never stderr");
    let body: serde_json::Value = serde_json::from_str(&o.stdout).unwrap();
    assert_eq!(body["error"]["code"], 500);
    let msg = body["error"]["message"].as_str().unwrap();
    assert!(msg.contains("thinking_reply"), "{msg}");
    assert!(msg.contains("thinking_replay, document_url_drop"), "{msg}");
}

#[test]
fn the_global_lossy_reject_reads_the_same_knob() {
    let tmp = tempfile::tempdir().unwrap();
    let stash = ReplayStash::new(tmp.path());
    let cfg = masq_cfg("[ingress]\nlossy = \"reject\"\n");
    let o = go_in_stashed(&cfg, &continuation(), &ok_basic(), &stash);
    assert_eq!(o.code, 64);
    // And an explicit per-case ADAPT override beats a global reject.
    let cfg = masq_cfg(
        "[ingress]\nlossy = \"reject\"\nlossy_overrides = { thinking_replay = \"adapt\" }\n",
    );
    let o = go_in_stashed(&cfg, &continuation(), &ok_basic(), &stash);
    assert_eq!(o.code, 0, "stderr: {}", o.stderr);
}

#[test]
fn a_stash_write_failure_never_fails_the_turn() {
    // Root the stash where its dir cannot be created: the `brazen` component is
    // a FILE. The thinking turn still answers 0 — fail-open on write (§5).
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("brazen"), b"in the way").unwrap();
    let stash = ReplayStash::new(tmp.path());
    let cfg = masq_cfg("");
    let o = go_in_stashed(
        &cfg,
        br#"{"model":"gpt-4o","messages":[{"role":"user","content":"weather?"}]}"#,
        &MockTransport::ok(vec![THINKING_TOOLS]),
        &stash,
    );
    assert_eq!(o.code, 0, "stderr: {}", o.stderr);
    assert_eq!(stash.recall(TOOL_ID), None, "nothing landed, nothing broke");
}

#[test]
fn the_masquerade_never_touches_the_model_cache_write_path() {
    // Sanity for §8's never-list-upstream stance on the data route: a masquerade
    // turn behaves exactly like a one-shot run — its learn-on-success append is
    // the ordinary generate behavior, no wholesale write.
    let tmp = tempfile::tempdir().unwrap();
    let stash = ReplayStash::new(tmp.path());
    let cfg = masq_cfg("");
    let cache = crate::testing::MemoryModelCache::new();
    let o = go_stashed(
        &["--in", "openai_chat", "--config", cfg.0.to_str().unwrap()],
        &[],
        &mut Cursor::new(br#"{"model":"gpt-4o","messages":[]}"#.to_vec()),
        &ok_basic(),
        &empty_store(),
        &cache,
        &stash,
    );
    assert_eq!(o.code, 0);
    let puts = cache.puts();
    assert_eq!(puts.len(), 1, "the ordinary learn-on-success append only");
    assert_eq!(puts[0].1.len(), 1);
    assert_eq!(puts[0].1[0].id, "claude-x");
    assert!(cache.get("anthropic").is_some());
}
