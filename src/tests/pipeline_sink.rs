//! Output projection + pump tests (§5.1–§5.4, §5.8, §8): each mode is driven
//! from a literal `Event` stream. NDJSON is byte-identical to the §5.2 sample;
//! `--text` is text-deltas-only with errors on stderr; `--raw` is verbatim and
//! drops everything but `Raw`; and `pump` computes last-error-wins / BrokenPipe.

use std::io::{self, Write};

use crate::{
    pump, CanonicalError, ContentKind, Delta, ErrorKind, Event, NdjsonSink, RawSink, Role, Sink,
    TextSink, Usage,
};

/// The §5.2 sample stream, shared across the NDJSON and text projections.
fn sample_stream() -> Vec<Event> {
    vec![
        Event::message_start(
            Some("msg_01…".into()),
            Some("claude-3-5-sonnet".into()),
            Role::Assistant,
        ),
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
            input_tokens: Some(12),
            output_tokens: Some(2),
            cache_read_tokens: None,
            cache_write_tokens: None,
        }),
        Event::Finish {
            reason: crate::FinishReason::Stop,
        },
        Event::End,
    ]
}

#[test]
fn ndjson_sink_is_byte_identical_to_the_5_2_sample() {
    let mut buf = Vec::new();
    let mut sink = NdjsonSink::new(&mut buf);
    for ev in sample_stream() {
        sink.write(&ev).unwrap();
    }
    let expected = concat!(
        r#"{"type":"message_start","v":1,"id":"msg_01…","model":"claude-3-5-sonnet","role":"assistant"}"#,
        "\n",
        r#"{"type":"content_start","index":0,"kind":{"text":{}}}"#,
        "\n",
        r#"{"type":"content_delta","index":0,"delta":{"text_delta":"Hel"}}"#,
        "\n",
        r#"{"type":"content_delta","index":0,"delta":{"text_delta":"lo"}}"#,
        "\n",
        r#"{"type":"content_stop","index":0}"#,
        "\n",
        r#"{"type":"usage","input_tokens":12,"output_tokens":2,"cache_read_tokens":null,"cache_write_tokens":null}"#,
        "\n",
        r#"{"type":"finish","reason":"stop"}"#,
        "\n",
        r#"{"type":"end"}"#,
        "\n",
    );
    assert_eq!(String::from_utf8(buf).unwrap(), expected);
}

#[test]
fn text_sink_emits_only_text_deltas_no_trailing_newline() {
    let mut out = Vec::new();
    let mut err = Vec::new();
    {
        let mut sink = TextSink::new(&mut out, &mut err, false);
        for ev in sample_stream() {
            sink.write(&ev).unwrap();
        }
    }
    assert_eq!(out, b"Hello");
    assert!(err.is_empty());
}

#[test]
fn text_sink_writes_errors_to_stderr_one_line() {
    let mut out = Vec::new();
    let mut err = Vec::new();
    {
        let mut sink = TextSink::new(&mut out, &mut err, false);
        sink.write(&Event::ContentDelta {
            index: 0,
            delta: Delta::TextDelta("answer".into()),
        })
        .unwrap();
        sink.write(&Event::Error(CanonicalError {
            kind: ErrorKind::Transport,
            message: "premature upstream EOF".into(),
            provider_detail: None,
            retry_after_seconds: None,
        }))
        .unwrap();
    }
    assert_eq!(out, b"answer");
    assert_eq!(err, b"premature upstream EOF\n");
}

/// A reasoning-then-answer stream: two thinking deltas, then the text answer
/// (the `bz "2+2" --thinking` shape of §5.3).
fn thinking_stream() -> Vec<Event> {
    vec![
        Event::ContentStart {
            index: 0,
            kind: ContentKind::Thinking {},
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

fn text_run(thinking: bool, stream: Vec<Event>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut err = Vec::new();
    {
        let mut sink = TextSink::new(&mut out, &mut err, thinking);
        for ev in stream {
            sink.write(&ev).unwrap();
        }
    }
    assert!(err.is_empty());
    out
}

#[test]
fn thinking_sink_emits_reasoning_then_one_separator_then_answer() {
    // `…reasoning…\n4`: the deltas concatenated, exactly one `\n` at the answer.
    assert_eq!(text_run(true, thinking_stream()), b"reason\n4");
}

#[test]
fn thinking_off_drops_thinking_deltas_and_injects_no_separator() {
    // Same stream, plain `--text`: reasoning drops, no separator — just the answer.
    assert_eq!(text_run(false, thinking_stream()), b"4");
}

#[test]
fn thinking_on_text_only_stream_injects_no_separator() {
    // The separator is owed only by a thinking delta; a text-only run emits none.
    assert_eq!(text_run(true, sample_stream()), b"Hello");
}

#[test]
fn raw_sink_writes_raw_verbatim_and_drops_everything_else() {
    let mut buf = Vec::new();
    {
        let mut sink = RawSink::new(&mut buf);
        sink.write(&Event::Raw(b"data: {\"x\":1}\n\n".to_vec()))
            .unwrap();
        // Non-`Raw` events (including the `End` `run` always emits) are dropped:
        // raw output carries the provider's own terminator, no appended `end`.
        sink.write(&Event::End).unwrap();
        sink.write(&Event::Raw(b"tail".to_vec())).unwrap();
    }
    assert_eq!(buf, b"data: {\"x\":1}\n\ntail");
}

#[test]
fn pump_clean_stream_exits_zero() {
    let mut buf = Vec::new();
    let mut sink = NdjsonSink::new(&mut buf);
    assert_eq!(pump(sample_stream(), &mut sink), 0);
}

#[test]
fn pump_last_error_wins() {
    let mut buf = Vec::new();
    let mut sink = NdjsonSink::new(&mut buf);
    let stream = vec![
        Event::Error(CanonicalError {
            kind: ErrorKind::Provider { status: 500 }, // → 70
            message: "first".into(),
            provider_detail: None,
            retry_after_seconds: None,
        }),
        Event::Error(CanonicalError {
            kind: ErrorKind::Auth, // → 77, the later error wins
            message: "second".into(),
            provider_detail: None,
            retry_after_seconds: None,
        }),
        Event::End,
    ];
    assert_eq!(pump(stream, &mut sink), 77);
}

/// A writer that fails every write with `BrokenPipe` (the Windows SIGPIPE path).
struct BrokenPipeWriter;

impl Write for BrokenPipeWriter {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::from(io::ErrorKind::BrokenPipe))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn pump_maps_broken_pipe_to_141() {
    let mut sink = NdjsonSink::new(BrokenPipeWriter);
    assert_eq!(pump(sample_stream(), &mut sink), 141);
}
