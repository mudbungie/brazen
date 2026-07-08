//! `PrettySink` golden streams (interactive-output spec §4–§6): drive literal
//! `Event` streams through split `(out, err)` writers and assert BOTH byte-for-byte.
//! The contract: stdout is byte-identical to `TextSink` (the answer is never styled),
//! all chrome is on stderr with a sigil gutter, and `--thinking` is dim-on-stdout.
//! THE regression test proves `PrettySink.out == TextSink.out` with no `--thinking`.

use crate::{
    CanonicalError, ContentKind, Delta, ErrorKind, Event, FinishReason, PrettySink, Role, Sink,
    Style, TextSink, Usage,
};

/// Drive `stream` through a `PrettySink` of the given `style`/`thinking`, returning
/// `(stdout, stderr)` bytes. The writers are coerced to `&mut dyn Write` so every test
/// (here and in the failure-path siblings) shares ONE `PrettySink` monomorphization —
/// the same instantiation production uses — so line coverage merges cleanly across them.
fn pretty_run(style: Style, thinking: bool, stream: Vec<Event>) -> (Vec<u8>, Vec<u8>) {
    let mut out = Vec::new();
    let mut err = Vec::new();
    {
        let (o, e): (&mut dyn std::io::Write, &mut dyn std::io::Write) = (&mut out, &mut err);
        let mut sink = PrettySink::new(o, e, thinking, style);
        for ev in stream {
            sink.write(&ev).unwrap();
        }
    }
    (out, err)
}

/// Drive `stream` through a plain `TextSink`, returning `(stdout, stderr)` bytes.
fn text_run(thinking: bool, stream: Vec<Event>) -> (Vec<u8>, Vec<u8>) {
    let mut out = Vec::new();
    let mut err = Vec::new();
    {
        let mut sink = TextSink::new(&mut out, &mut err, thinking);
        for ev in stream {
            sink.write(&ev).unwrap();
        }
    }
    (out, err)
}

const UTF8: Style = Style::Pretty { ascii: false };
const ASCII: Style = Style::Pretty { ascii: true };

/// A plain text answer, with a finish + usage so the footer fires.
fn answer_stream() -> Vec<Event> {
    vec![
        Event::message_start(Some("m".into()), Some("x".into()), Role::Assistant),
        Event::ContentStart {
            index: 0,
            kind: ContentKind::Text {},
        },
        Event::ContentDelta {
            index: 0,
            delta: Delta::TextDelta("Hel".into()),
        },
        Event::ContentDelta {
            index: 0,
            delta: Delta::TextDelta("lo".into()),
        },
        Event::ContentStop { index: 0 },
        Event::Usage(Usage {
            input_tokens: Some(312),
            output_tokens: Some(47),
            cache_read_tokens: None,
            cache_write_tokens: None,
        }),
        Event::Finish {
            reason: FinishReason::Stop,
        },
        Event::End,
    ]
}

#[test]
fn answer_on_stdout_is_unstyled_footer_on_stderr() {
    let (out, err) = pretty_run(UTF8, false, answer_stream());
    // The answer is byte-pristine — no SGR even on a tty.
    assert_eq!(out, b"Hello");
    // The footer: a green `✓` gutter, then the dim reason + token counts (cache omitted).
    assert_eq!(
        String::from_utf8(err).unwrap(),
        "\x1b[32m✓\x1b[0m \x1b[2mstop · 312 in · 47 out\x1b[0m\n"
    );
}

/// A tool call: open the block, stream JSON arg fragments, close it.
fn tool_stream() -> Vec<Event> {
    vec![
        Event::ContentStart {
            index: 0,
            kind: ContentKind::ToolUse {
                id: "t1".into(),
                name: "get_weather".into(),
            },
        },
        Event::ContentDelta {
            index: 0,
            delta: Delta::JsonDelta("{\"city\":".into()),
        },
        Event::ContentDelta {
            index: 0,
            delta: Delta::JsonDelta("\"SF\"}".into()),
        },
        Event::ContentStop { index: 0 },
        Event::End,
    ]
}

#[test]
fn tool_call_is_a_stderr_gutter_line_name_bold_args_dim() {
    let (out, err) = pretty_run(UTF8, false, tool_stream());
    // Tool calls produce NO stdout (today silently dropped in text mode — the win is
    // surfacing them on stderr).
    assert!(out.is_empty());
    assert_eq!(
        String::from_utf8(err).unwrap(),
        "\x1b[33m⚙\x1b[0m \x1b[1mget_weather\x1b[0m \x1b[2m{\"city\":\"SF\"}\x1b[0m\n"
    );
}

#[test]
fn ascii_degradation_swaps_glyphs_keeps_sgr() {
    let (_, err) = pretty_run(ASCII, false, tool_stream());
    // The `⚙` degrades to `*`; the bold/dim SGR stays (color is still on).
    assert_eq!(
        String::from_utf8(err).unwrap(),
        "\x1b[33m*\x1b[0m \x1b[1mget_weather\x1b[0m \x1b[2m{\"city\":\"SF\"}\x1b[0m\n"
    );
}

/// A reasoning-then-answer stream (the `--thinking` shape).
fn thinking_stream() -> Vec<Event> {
    vec![
        Event::ContentStart {
            index: 0,
            kind: ContentKind::Thinking { id: None },
        },
        Event::ContentDelta {
            index: 0,
            delta: Delta::ThinkingDelta("rea".into()),
        },
        Event::ContentDelta {
            index: 0,
            delta: Delta::ThinkingDelta("son".into()),
        },
        Event::ContentStop { index: 0 },
        Event::ContentStart {
            index: 1,
            kind: ContentKind::Text {},
        },
        Event::ContentDelta {
            index: 1,
            delta: Delta::TextDelta("4".into()),
        },
        Event::End,
    ]
}

#[test]
fn thinking_is_dim_on_stdout_separator_and_answer_unstyled() {
    let (out, err) = pretty_run(UTF8, true, thinking_stream());
    // Each thinking delta is dim-wrapped; the `\n` separator and the answer are raw.
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "\x1b[2mrea\x1b[0m\x1b[2mson\x1b[0m\n4"
    );
    // Thinking is dim-on-stdout, NOT a stderr line (overrides the design mockup).
    assert!(err.is_empty());
}

#[test]
fn thinking_off_drops_reasoning_like_textsink() {
    // Same stream, plain pretty (no --thinking): reasoning drops, only the answer.
    let (out, err) = pretty_run(UTF8, false, thinking_stream());
    assert_eq!(out, b"4");
    assert!(err.is_empty());
}

#[test]
fn json_delta_with_no_open_tool_block_is_dropped() {
    // A `JsonDelta` only ever rides an open tool block; a stray one (no `ToolUse` start)
    // has nowhere to accumulate and is silently dropped — no stdout, no stderr.
    let (out, err) = pretty_run(
        UTF8,
        false,
        vec![Event::ContentDelta {
            index: 0,
            delta: Delta::JsonDelta("{}".into()),
        }],
    );
    assert!(out.is_empty());
    assert!(err.is_empty());
}

#[test]
fn error_is_a_red_stderr_line_answer_untouched() {
    let stream = vec![
        Event::ContentDelta {
            index: 0,
            delta: Delta::TextDelta("partial".into()),
        },
        Event::Error(CanonicalError {
            kind: ErrorKind::Transport,
            message: "premature upstream EOF".into(),
            provider_detail: None,
            retry_after_seconds: None,
        }),
    ];
    let (out, err) = pretty_run(UTF8, false, stream);
    assert_eq!(out, b"partial");
    // A red `✗` label, then the message — the exit code is `pump`'s, untouched.
    assert_eq!(
        String::from_utf8(err).unwrap(),
        "\x1b[31m✗\x1b[0m premature upstream EOF\n"
    );
}

// The footer detail (cache counts, usage merge, finish-reason labels) lives in
// `pipeline_pretty_footer.rs` — split out to keep each test file under the 300-line cap.

// ---- THE regression test: the building-block contract ----

#[test]
fn stdout_is_byte_identical_to_textsink_without_thinking() {
    // A representative stream (answer + a tool call + footer). PrettySink's stdout
    // must equal TextSink's stdout byte-for-byte — all the chrome diverges only on
    // stderr. This IS the building-block contract: `bz "q" | tool` is unchanged.
    let stream = vec![
        Event::ContentStart {
            index: 0,
            kind: ContentKind::ToolUse {
                id: "t".into(),
                name: "search".into(),
            },
        },
        Event::ContentDelta {
            index: 0,
            delta: Delta::JsonDelta("{}".into()),
        },
        Event::ContentStop { index: 0 },
        Event::ContentStart {
            index: 1,
            kind: ContentKind::Text {},
        },
        Event::ContentDelta {
            index: 1,
            delta: Delta::TextDelta("the answer".into()),
        },
        Event::Usage(Usage {
            input_tokens: Some(1),
            output_tokens: Some(2),
            cache_read_tokens: None,
            cache_write_tokens: None,
        }),
        Event::Finish {
            reason: FinishReason::Stop,
        },
        Event::End,
    ];
    let (pretty_out, pretty_err) = pretty_run(UTF8, false, stream.clone());
    let (text_out, text_err) = text_run(false, stream);
    assert_eq!(
        pretty_out, text_out,
        "stdout must be byte-identical to TextSink"
    );
    assert_eq!(text_err, b"", "TextSink emits no stderr on a clean stream");
    assert!(!pretty_err.is_empty(), "PrettySink chrome lives on stderr");
}

#[test]
fn thinking_stdout_differs_only_by_dim_sgr_around_reasoning() {
    // With --thinking, the answer-portion bytes match TextSink; pretty's stdout adds
    // only the dim escapes bracketing the thinking text.
    let (pretty_out, _) = pretty_run(UTF8, true, thinking_stream());
    let (text_out, _) = text_run(true, thinking_stream());
    assert_eq!(text_out, b"reason\n4");
    // Strip the dim SGR from pretty's stdout — what's left is exactly TextSink's.
    let stripped = String::from_utf8(pretty_out)
        .unwrap()
        .replace("\x1b[2m", "")
        .replace("\x1b[0m", "");
    assert_eq!(stripped.as_bytes(), text_out.as_slice());
}
