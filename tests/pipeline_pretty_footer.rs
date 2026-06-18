//! `PrettySink` footer detail (interactive-output spec §5) + the stderr-write-failure
//! paths: the finish/usage footer (cache counts only when non-zero, usage merged across
//! the partial events a provider reports in pieces, every `FinishReason` labelled), and
//! each chrome writer's `io::Error` propagation. Split from `pipeline_pretty.rs` to keep
//! each test file under the 300-line cap.

use std::io::{self, Write};

use brazen::{
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
            input: Some(10),
            output: Some(5),
            cache_read: Some(8),
            cache_write: Some(0), // zero is omitted — never a fabricated `0`
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
            input: Some(12),
            output: None,
            cache_read: None,
            cache_write: None,
        }),
        Event::Usage(Usage {
            input: None, // absent — must NOT erase the prior input:12
            output: Some(2),
            cache_read: None,
            cache_write: None,
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

/// A stderr that fails every write — to drive each chrome writer's `io::Error`
/// propagation (the `?` on the `writeln!`/`flush`), mirroring `BrokenPipeWriter`.
struct FailErr;
impl Write for FailErr {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("boom"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::other("boom"))
    }
}

/// Drive one chrome-producing event through a `PrettySink` whose stderr fails, asserting
/// the write error propagates out of `write` (the chrome `?` paths).
fn err_propagates(ev: Event) {
    let mut out = Vec::new();
    let (o, e): (&mut dyn Write, &mut dyn Write) = (&mut out, &mut FailErr);
    let mut sink = PrettySink::new(o, e, false, UTF8);
    assert!(sink.write(&ev).is_err());
}

#[test]
fn each_chrome_writer_propagates_a_failed_stderr() {
    // The footer (Finish), the error line, and the tool flush (ContentStop with an open
    // tool) each write to stderr — a failing stderr must surface as an `Err`, not a panic.
    err_propagates(Event::Finish {
        reason: FinishReason::Stop,
    });
    err_propagates(Event::Error(CanonicalError {
        kind: ErrorKind::Transport,
        message: "x".into(),
        provider_detail: None,
    }));
    // Open a tool block first, then close it on the failing sink so `flush_tool` writes.
    let mut out = Vec::new();
    let (o, e): (&mut dyn Write, &mut dyn Write) = (&mut out, &mut FailErr);
    let mut sink = PrettySink::new(o, e, false, UTF8);
    sink.write(&Event::ContentStart {
        index: 0,
        kind: ContentKind::ToolUse {
            id: "t".into(),
            name: "n".into(),
        },
    })
    .unwrap();
    sink.write(&Event::ContentDelta {
        index: 0,
        delta: Delta::JsonDelta("{}".into()),
    })
    .unwrap();
    assert!(sink.write(&Event::ContentStop { index: 0 }).is_err());
}

/// A stdout that succeeds the first `left` writes, then fails — lets a test reach a
/// branch (the one-shot separator) that only runs after an earlier successful write.
struct FailAfter {
    left: u32,
}
impl Write for FailAfter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.left == 0 {
            return Err(io::Error::other("boom"));
        }
        self.left -= 1;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn thinking_delta() -> Event {
    Event::ContentDelta {
        index: 0,
        delta: Delta::ThinkingDelta("r".into()),
    }
}

fn answer_delta() -> Event {
    Event::ContentDelta {
        index: 1,
        delta: Delta::TextDelta("a".into()),
    }
}

/// Build a single-instantiation `PrettySink` over `&mut dyn Write` writers, prime it with
/// all but the last event, and assert the last write fails (the stdout `?` paths).
fn out_fails(mut out: FailAfter, thinking: bool, evs: &[Event]) {
    let mut err = Vec::new();
    let (o, e): (&mut dyn Write, &mut dyn Write) = (&mut out, &mut err);
    let mut sink = PrettySink::new(o, e, thinking, UTF8);
    let (last, prime) = evs.split_last().unwrap();
    for ev in prime {
        sink.write(ev).unwrap();
    }
    assert!(sink.write(last).is_err());
}

#[test]
fn stdout_write_errors_propagate_on_every_answer_path() {
    // The plain answer write, the dim thinking write, and the one-shot separator write each
    // go to stdout — a failing stdout must surface as an `Err`, mirroring `TextSink`'s
    // building-block write paths (the answer channel is byte-identical, errors included).
    // The plain answer write fails on the first byte; the thinking write fails likewise.
    out_fails(FailAfter { left: 0 }, false, &[answer_delta()]);
    out_fails(FailAfter { left: 0 }, true, &[thinking_delta()]);
    // The separator branch runs only after a successful thinking write; let one write (the
    // thinking text) land, then fail the separator `\n` on the answer's first byte.
    let sep = [thinking_delta(), answer_delta()];
    out_fails(FailAfter { left: 1 }, true, &sep);
}
