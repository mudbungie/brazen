//! Output projections and the pump loop (§5.1–§5.4, §5.6–§5.8).
//!
//! `Sink` is the single output seam: `write` is called once per `Event`, in
//! order, and MUST flush before returning — no event is buffered across calls,
//! so backpressure is the kernel's blocking write (§5.7). Each projection owns
//! how an `Event` lands on the wire; the `pump` driver is mode-agnostic and is
//! the only place the exit code is computed.

use std::io::{self, Write};
use std::path::Path;

use super::image_file::ImageBlocks;
use crate::canonical::{ContentKind, Delta, Event, ExitClass};

/// The one output surface (§5.1). Implementors flush before returning.
pub trait Sink {
    fn write(&mut self, ev: &Event) -> io::Result<()>;
}

/// `--json`: one canonical `Event` per line, `\n`-terminated, flushed
/// immediately (§5.2). NDJSON is serde's direct serialization of `Event` — no
/// second schema. `Event::Raw` never reaches here (it is `--raw`-only and
/// `serde(skip)`), so the serialization is infallible on our own owned type.
pub struct NdjsonSink<W: Write> {
    w: W,
}

impl<W: Write> NdjsonSink<W> {
    pub fn new(w: W) -> Self {
        Self { w }
    }
}

impl<W: Write> Sink for NdjsonSink<W> {
    fn write(&mut self, ev: &Event) -> io::Result<()> {
        // The one permitted internal infallibility (§5.2): the `expect` is on
        // our own owned `Event`, never external input.
        #[allow(clippy::expect_used)]
        let mut buf = serde_json::to_vec(ev).expect("Event is infallibly serializable");
        buf.push(b'\n');
        self.w.write_all(&buf)?;
        self.w.flush()
    }
}

/// `--text` (default): emit only `ContentDelta::TextDelta` bytes, concatenated,
/// no framing and no injected trailing newline (§5.3). Thinking/tool/usage/
/// start/finish/end events drop from stdout (they still drive the exit code).
/// `Event::Error` is written to stderr (its `message`, one line) so a mid-stream
/// failure is never silent — text mode suppresses event lines from *stdout*, not
/// from the user (§5.9). The terminator is EOF on stdout, never an `end` line.
///
/// `--thinking` is the same projection with `thinking` set (§5.3): `ThinkingDelta`
/// text is also emitted, *before* the answer, then a single `\n` separator at the
/// first answer byte — so `bz "2+2" --thinking` → `…reasoning…\n4`. The separator
/// is the lone structure text mode injects, and it fires exactly once: a thinking
/// delta arms `pending_sep`, the first following `TextDelta` spends it. A run with
/// no thinking never arms it and so injects nothing. Deleting the field, the guard,
/// and the `pending_sep` line removes `--thinking` whole — it severs cleanly.
///
/// A model-RETURNED `Image` block is a FILE under `dir`, its path one line on stderr
/// (architecture §5.3, bl-0987): `images` accumulates the base64 per open block and
/// writes at the block's stop; stdout is untouched. The terminal (`Error`/`End`)
/// drops an open block rather than writing a truncated image.
pub struct TextSink<O: Write, E: Write> {
    out: O,
    err: E,
    thinking: bool,
    pending_sep: bool,
    images: ImageBlocks,
}

impl<O: Write, E: Write> TextSink<O, E> {
    pub fn new(out: O, err: E, thinking: bool, dir: &Path) -> Self {
        Self {
            out,
            err,
            thinking,
            pending_sep: false,
            images: ImageBlocks::new(dir),
        }
    }
}

impl<O: Write, E: Write> Sink for TextSink<O, E> {
    fn write(&mut self, ev: &Event) -> io::Result<()> {
        match ev {
            // `--thinking` only: emit the reasoning and arm the one-time separator
            // owed before the answer. Plain `--text` leaves the guard false, so a
            // thinking delta falls through to `_` and drops (the default behavior).
            Event::ContentDelta {
                delta: Delta::ThinkingDelta(text),
                ..
            } if self.thinking => {
                self.out.write_all(text.as_bytes())?;
                self.pending_sep = true;
                self.out.flush()
            }
            Event::ContentDelta {
                delta: Delta::TextDelta(text),
                ..
            } => {
                if self.pending_sep {
                    self.out.write_all(b"\n")?;
                    self.pending_sep = false;
                }
                self.out.write_all(text.as_bytes())?;
                self.out.flush()
            }
            Event::ContentStart {
                index,
                kind: ContentKind::Image { media_type },
            } => {
                self.images.start(*index, media_type);
                Ok(())
            }
            Event::ContentDelta {
                index,
                delta: Delta::ImageDelta(frag),
            } => {
                self.images.delta(*index, frag);
                Ok(())
            }
            // An image block's stop writes the file and names it — the bare path, one
            // line on stderr (PLAIN prints no chrome, but the path is where the answer
            // went, not chrome). A text/tool stop writes nothing.
            Event::ContentStop { index } => match self.images.stop(*index)? {
                Some(path) => {
                    writeln!(self.err, "{}", path.display())?;
                    self.err.flush()
                }
                None => Ok(()),
            },
            Event::Error(err) => {
                self.images.drop_all();
                writeln!(self.err, "{}", err.message)?;
                self.err.flush()
            }
            Event::End => {
                self.images.drop_all();
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// `--raw`: transport bytes already became `Event::Raw` chunks; write them
/// verbatim, flushing per chunk (§5.4). The provider's own terminator stands —
/// brazen appends no `end`, so every non-`Raw` event (including the single
/// `Event::End` `run` always emits) drops here. This is what lets `run` write
/// `End` unconditionally while raw output carries no end token.
pub struct RawSink<W: Write> {
    w: W,
}

impl<W: Write> RawSink<W> {
    pub fn new(w: W) -> Self {
        Self { w }
    }
}

impl<W: Write> Sink for RawSink<W> {
    fn write(&mut self, ev: &Event) -> io::Result<()> {
        if let Event::Raw(bytes) = ev {
            self.w.write_all(bytes)?;
            self.w.flush()?;
        }
        Ok(())
    }
}

/// Drive the typed event stream to the sink and compute the exit code (§5.1, §8):
/// the **byte adapter** half of `run` — it serializes [`generate`](crate::generate)'s
/// `Iterator<Item = Event>` through the mode's sink. Mode-agnostic; the sink owns the
/// projection. The events arrive LAZILY (the iterator pulls a transport chunk per
/// step), so this stays incremental — never collecting the stream. `Event::Error` is
/// in-band and does NOT stop the loop (partial-response-then-error is representable);
/// each error sets the exit from its own `kind`, so a later error overrides an earlier
/// one (**last-error-wins**), and the trailing `End` (which `generate` appends) carries
/// no exit. A write error stops the loop and maps via `from_io`: `BrokenPipe` → exit
/// **141** (the Windows SIGPIPE path, §5.8; on Unix the signal kills us first), anything
/// else → 69.
pub(crate) fn pump<I: IntoIterator<Item = Event>>(events: I, sink: &mut dyn Sink) -> u8 {
    let mut exit = ExitClass::Ok.code();
    for ev in events {
        if let Event::Error(err) = &ev {
            exit = err.exit_code();
        }
        if let Err(io_err) = sink.write(&ev) {
            return ExitClass::from_io(&io_err).code();
        }
    }
    exit
}
