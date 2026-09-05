//! The context window in-band on `Usage` (model-discovery §5.5): the input-token limit
//! — the DENOMINATOR for the counters — rides every `Usage` event, so a harness that
//! never calls `--list-models` still learns it. Three sources, one ladder: what the
//! request BODY pins (Ollama's `options.num_ctx`), else what the provider's list
//! SERVED, else what the provider row DECLARES for that model. Absent when nothing
//! states one, and never fabricated. `MockTransport`/`MemoryModelCache`; zero network.

use std::io::Cursor;

use crate::testing::{MemoryModelCache, MockTransport};
use crate::tests::login_support::{temp, TempFile};
use crate::tests::run_support::*;
use crate::Model;

/// The ollama NDJSON turn — two content lines and a `done` carrying the counters, so
/// the window has a `Usage` event to ride.
const OLLAMA: &[u8] = include_bytes!("../../tests/fixtures/ollama_chat_basic.ndjson");

/// A cache row for `anthropic`, optionally stating a window — the only difference
/// between the two arms of the fact (stated / not stated).
fn cache(window: Option<u32>) -> MemoryModelCache {
    MemoryModelCache::with(
        "anthropic",
        vec![Model {
            id: "claude-opus-4-1-20250805".into(),
            default: false,
            context_window: window,
            ..Default::default()
        }],
    )
}

fn go_window(cache: &MemoryModelCache, model: &str, tx: &MockTransport) -> Out {
    go_window_cfg(cache, model, &[], tx)
}

/// The same drive with a config file in scope — the arm that carries a row's own
/// `context_windows` declaration.
fn go_window_cfg(
    cache: &MemoryModelCache,
    model: &str,
    env: &[(&str, &str)],
    tx: &MockTransport,
) -> Out {
    go_cached(
        &[
            "--json",
            "--provider",
            "anthropic",
            "--model",
            model,
            "--api-key",
            "sk",
            "hi",
        ],
        env,
        &mut Cursor::new(Vec::new()),
        tx,
        &empty_store(),
        cache,
    )
}

/// An `anthropic` row declaring `window` for the opus wire id — a per-FIELD patch of
/// the embedded row (config §3.2), nothing else redeclared.
fn declaring(model: &str, window: u32) -> TempFile {
    temp(&format!(
        "[[provider]]\nname = \"anthropic\"\ncontext_windows = {{ \"{model}\" = {window} }}\n"
    ))
}

/// The one window on the turn's `usage` events, or `None` when the key never appears.
/// Asserts the events AGREE — one stamp site cannot disagree with itself.
fn stamped(o: &Out) -> Option<u64> {
    let usage: Vec<serde_json::Value> = o
        .stdout
        .lines()
        .filter(|l| l.contains(r#""type":"usage""#))
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(!usage.is_empty(), "a usage event to ride: {}", o.stdout);
    let windows: Vec<Option<u64>> = usage
        .iter()
        .map(|u| u.get("context_window").and_then(serde_json::Value::as_u64))
        .collect();
    assert!(
        windows.iter().all(|w| *w == windows[0]),
        "every usage event carries the same window: {}",
        o.stdout
    );
    windows[0]
}

#[test]
fn a_stated_window_rides_every_usage_event() {
    // The row states 200000, so BOTH usage events (message_start's input count and
    // message_delta's output count, §3.6) carry the denominator beside the counters —
    // one stamp site, every event, no second call.
    let tx = MockTransport::ok(vec![BASIC]);
    let o = go_window(&cache(Some(200_000)), "opus", &tx);
    assert_eq!(o.code, 0);
    let usage: Vec<&str> = o
        .stdout
        .lines()
        .filter(|l| l.contains(r#""type":"usage""#))
        .collect();
    assert_eq!(usage.len(), 2, "both usage events: {}", o.stdout);
    for line in &usage {
        assert!(
            line.contains(r#""context_window":200000"#),
            "the window rides the usage event: {line}"
        );
    }
    // Only usage carries it — it is a counter's denominator, not a message fact.
    assert!(
        !o.stdout
            .lines()
            .any(|l| l.contains("context_window") && !l.contains(r#""type":"usage""#)),
        "no other event grows the key: {}",
        o.stdout
    );
}

#[test]
fn a_row_stating_no_window_omits_the_key() {
    // Absent stays absent (the Usage zero-vs-unknown principle): a row whose provider
    // serves no limit leaves the field off the wire entirely — byte-identical to the
    // pre-window event, never a fabricated 0.
    let tx = MockTransport::ok(vec![BASIC]);
    let o = go_window(&cache(None), "opus", &tx);
    assert_eq!(o.code, 0);
    assert!(
        !o.stdout.contains("context_window"),
        "no key at all when the row states none: {}",
        o.stdout
    );
    assert!(
        o.stdout.contains(r#""type":"usage""#),
        "the usage events are still there: {}",
        o.stdout
    );
}

#[test]
fn a_verbatim_model_the_cache_cannot_place_has_no_window() {
    // The seed resolved `Verbatim` (the cache holds a window, but for a DIFFERENT id):
    // the window belongs to the row, not the provider, so an unplaced id carries none.
    let tx = MockTransport::ok(vec![BASIC]);
    let o = go_window(&cache(Some(200_000)), "claude-not-in-the-cache", &tx);
    assert_eq!(o.code, 0);
    assert!(
        !o.stdout.contains("context_window"),
        "an unplaced id borrows no other row's window: {}",
        o.stdout
    );
}

#[test]
fn a_row_declaring_a_window_fills_where_the_list_serves_none() {
    // The second source (§5.5): Anthropic's list GET serves no limit, so the cached row
    // states nothing — and the provider row's own `context_windows` answers for the
    // WIRE id the seed expanded to (`opus` → `claude-opus-4-1-20250805`), which is the
    // id the request will carry. Before this, the denominator was dark on nearly every
    // turn and a harness kept its own table beside brazen's.
    let cfg = declaring("claude-opus-4-1-20250805", 200_000);
    let tx = MockTransport::ok(vec![BASIC]);
    let o = go_window_cfg(
        &cache(None),
        "opus",
        &[("BRAZEN_CONFIG", cfg.0.to_str().unwrap())],
        &tx,
    );
    assert_eq!(o.code, 0);
    assert_eq!(stamped(&o), Some(200_000));
}

#[test]
fn a_served_window_outranks_the_rows_declaration() {
    // Observation beats declaration, per model: the list a `--list-models` GET actually
    // served moves with the provider, while a declared number is whatever the operator
    // last knew. The row states 111111 for the very same id and is not consulted.
    let cfg = declaring("claude-opus-4-1-20250805", 111_111);
    let tx = MockTransport::ok(vec![BASIC]);
    let o = go_window_cfg(
        &cache(Some(200_000)),
        "opus",
        &[("BRAZEN_CONFIG", cfg.0.to_str().unwrap())],
        &tx,
    );
    assert_eq!(o.code, 0);
    assert_eq!(stamped(&o), Some(200_000));
}

#[test]
fn a_declaration_for_another_model_states_nothing() {
    // Carried, never fabricated: the table is keyed per model, so a row that states a
    // window for a DIFFERENT id leaves the key off the wire entirely — the resolved
    // model borrows no sibling's number.
    let cfg = declaring("claude-haiku-4-5", 200_000);
    let tx = MockTransport::ok(vec![BASIC]);
    let o = go_window_cfg(
        &cache(None),
        "opus",
        &[("BRAZEN_CONFIG", cfg.0.to_str().unwrap())],
        &tx,
    );
    assert_eq!(o.code, 0);
    assert!(
        !o.stdout.contains("context_window"),
        "no key at all: {}",
        o.stdout
    );
}

/// Drive the built-in `ollama` row with `body_defaults = { options = { num_ctx = N } }`
/// patched onto it (config §4.1, bl-f19d) — the one dialect whose request body states
/// its own window.
fn go_ollama(cache: &MemoryModelCache, num_ctx: &str, tx: &MockTransport) -> Out {
    let cfg = temp(&format!(
        "[[provider]]\nname = \"ollama\"\nbody_defaults = {{ options = {{ num_ctx = {num_ctx} }} }}\n"
    ));
    go_cached(
        &[
            "--json",
            "--provider",
            "ollama",
            "--model",
            "llama3.2",
            "hi",
        ],
        &[("BRAZEN_CONFIG", cfg.0.to_str().unwrap())],
        &mut Cursor::new(Vec::new()),
        tx,
        &empty_store(),
        cache,
    )
}

#[test]
fn a_pinned_num_ctx_is_the_window_on_the_event() {
    // The first source (§5.5): Ollama's `/api/tags` serves no limit and `/api/show` is a
    // round-trip brazen never makes, so the number in force is the one the row pinned in
    // the request body. It IS the honest denominator — the turn runs in exactly that
    // window — and it reaches the event through the one stamp site, off the canonical
    // request the encoder is about to project.
    let tx = MockTransport::ok(vec![OLLAMA]);
    let o = go_ollama(&MemoryModelCache::new(), "32768", &tx);
    assert_eq!(o.code, 0);
    assert_eq!(stamped(&o), Some(32_768));
}

#[test]
fn a_pinned_num_ctx_outranks_a_served_window() {
    // A pinned `num_ctx` TRUNCATES the model's capacity to the window this turn will
    // run in, so it outranks a served number for the same model rather than deferring
    // to it: 8192 is what the request gets, whatever the list says the model could do.
    let served = MemoryModelCache::with(
        "ollama",
        vec![Model {
            id: "llama3.2".into(),
            default: false,
            context_window: Some(131_072),
            ..Default::default()
        }],
    );
    let tx = MockTransport::ok(vec![OLLAMA]);
    let o = go_ollama(&served, "8192", &tx);
    assert_eq!(o.code, 0);
    assert_eq!(stamped(&o), Some(8192));
}

#[test]
fn an_unreadable_num_ctx_states_nothing_and_the_stated_window_answers() {
    // A row that pins a `num_ctx` no window can be read out of (a string) states
    // nothing — the ladder falls through to the served list rather than to a repaired
    // number. Nothing anywhere in this path invents a figure.
    let served = MemoryModelCache::with(
        "ollama",
        vec![Model {
            id: "llama3.2".into(),
            default: false,
            context_window: Some(131_072),
            ..Default::default()
        }],
    );
    let tx = MockTransport::ok(vec![OLLAMA]);
    let o = go_ollama(&served, "\"lots\"", &tx);
    assert_eq!(o.code, 0);
    assert_eq!(stamped(&o), Some(131_072));
}
