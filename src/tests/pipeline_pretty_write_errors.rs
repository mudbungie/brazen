//! `PrettySink` writer-failure propagation (interactive-output spec §5): each chrome
//! writer's stderr `io::Error` and each answer-path stdout `io::Error` must surface as
//! an `Err` out of `write`, never a panic. The footer/flush goldens live in
//! `pipeline_pretty_footer`. Per-file harness copy (the shared `UTF8` instantiation).

use std::io::{self, Write};

use crate::{
    CanonicalError, ContentKind, Delta, ErrorKind, Event, FinishReason, PrettySink, Sink, Style,
};

const UTF8: Style = Style::Pretty { ascii: false };

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
        retry_after_seconds: None,
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
