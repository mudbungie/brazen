//! `PrettySink` footer detail (interactive-output spec §5): the finish/usage footer
//! (cache counts only when non-zero, usage merged across the partial events a provider
//! reports in pieces, every `FinishReason` labelled) and the truncated-tool-call flush
//! goldens. Split from `pipeline_pretty.rs`; the writer `io::Error` propagation lives
//! in `pipeline_pretty_write_errors`.

use std::io::Write;

use crate::{
    CanonicalError, ContentKind, Delta, ErrorKind, Event, FinishReason, PrettySink, Sink, Style,
    Usage,
};

const UTF8: Style = Style::Pretty { ascii: false };

/// Drive `stream` through a UTF-8 `PrettySink` and return the stderr (footer) bytes. All
/// constructions in this file coerce the writers to `&mut dyn Write` so every test shares
/// ONE `PrettySink` monomorphization (the production instantiation) — line coverage then
/// merges across the success and failure paths instead of fragmenting per writer type.
fn drive(out: &mut dyn Write, err: &mut dyn Write, thinking: bool, stream: Vec<Event>) {
    let mut sink = PrettySink::new(out, err, thinking, UTF8);
    for ev in stream {
        let _ = sink.write(&ev);
    }
}

fn footer_of(stream: Vec<Event>) -> String {
    let mut out = Vec::new();
    let mut err = Vec::new();
    drive(&mut out, &mut err, false, stream);
    String::from_utf8(err).unwrap()
}

#[test]
fn footer_appends_cache_counts_only_when_nonzero() {
    let stream = vec![
        Event::Usage(Usage {
            input_tokens: Some(10),
            output_tokens: Some(5),
            cache_read_tokens: Some(8),
            cache_write_tokens: Some(0), // zero is omitted — never a fabricated `0`
        }),
        Event::Finish {
            reason: FinishReason::Refusal {
                category: "safety".into(),
                explanation: None,
            },
        },
    ];
    assert_eq!(
        footer_of(stream),
        "\x1b[32m✓\x1b[0m \x1b[2mrefusal: safety · 10 in · 5 out · 8 cache_r\x1b[0m\n"
    );
}

#[test]
fn footer_merges_usage_reported_in_pieces() {
    // The anthropic shape: input at message_start, output at the close — two partial
    // Usage events. The footer must hold BOTH, never the last partial alone.
    let stream = vec![
        Event::Usage(Usage {
            input_tokens: Some(12),
            output_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
        }),
        Event::Usage(Usage {
            input_tokens: None, // absent — must NOT erase the prior input:12
            output_tokens: Some(2),
            cache_read_tokens: None,
            cache_write_tokens: None,
        }),
        Event::Finish {
            reason: FinishReason::Stop,
        },
    ];
    assert_eq!(
        footer_of(stream),
        "\x1b[32m✓\x1b[0m \x1b[2mstop · 12 in · 2 out\x1b[0m\n"
    );
}

#[test]
fn footer_labels_every_finish_reason() {
    // Each reason maps to its bare token (refusal carries its category, covered above).
    let cases = [
        (FinishReason::Length, "length"),
        (FinishReason::ToolUse, "tool_use"),
        (FinishReason::StopSequence, "stop_sequence"),
        (FinishReason::Pause, "pause"),
        (FinishReason::Other("custom".into()), "custom"),
    ];
    for (reason, label) in cases {
        assert_eq!(
            footer_of(vec![Event::Finish { reason }]),
            format!("\x1b[32m✓\x1b[0m \x1b[2m{label}\x1b[0m\n")
        );
    }
}

#[test]
fn mid_stream_truncated_tool_call_flushes_before_the_error_then_end() {
    // The streaming-drop shape (`run::respond::stream`): a tool block opens and streams
    // PARTIAL JSON args, then the transport drops — `ContentStart{ToolUse} →
    // JsonDelta("{partial") → Error(transport) → End`. The drop/EOF paths emit NO
    // `ContentStop` to close the block, so the open tool MUST flush on `Error` (before the
    // red `✗`) and `End` (the universal net) — never silently vanish. This IS the one
    // thing PrettySink exists to surface (spec §5: tool calls are "the real win").
    let stream = vec![
        Event::ContentStart {
            index: 0,
            kind: ContentKind::ToolUse {
                id: "t1".into(),
                name: "get_weather".into(),
            },
        },
        Event::ContentDelta {
            index: 0,
            delta: Delta::JsonDelta("{\"city\":\"S".into()),
        },
        Event::Error(CanonicalError {
            kind: ErrorKind::Transport,
            message: "transport stream dropped".into(),
            provider_detail: None,
            retry_after_seconds: None,
        }),
        Event::End,
    ];
    // The `⚙ get_weather {partial` gutter line precedes the red `✗` (tool-being-built,
    // then the failure); `End` is a no-op since `Error` already flushed the block.
    assert_eq!(
        footer_of(stream),
        "\x1b[33m⚙\x1b[0m \x1b[1mget_weather\x1b[0m \x1b[2m{\"city\":\"S\x1b[0m\n\
         \x1b[31m✗\x1b[0m transport stream dropped\n"
    );
}

#[test]
fn bare_eof_truncated_tool_call_flushes_on_end_with_no_error_event() {
    // The premature-EOF shape with NO `Error` event reaching the sink: the open tool block
    // must still surface, flushed by the universal `End` net alone — the partial args are
    // never dropped just because the cut left no error line behind.
    let stream = vec![
        Event::ContentStart {
            index: 0,
            kind: ContentKind::ToolUse {
                id: "t1".into(),
                name: "search".into(),
            },
        },
        Event::ContentDelta {
            index: 0,
            delta: Delta::JsonDelta("{\"q\"".into()),
        },
        Event::End,
    ];
    assert_eq!(
        footer_of(stream),
        "\x1b[33m⚙\x1b[0m \x1b[1msearch\x1b[0m \x1b[2m{\"q\"\x1b[0m\n"
    );
}
