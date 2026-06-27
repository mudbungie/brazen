//! The interactive pretty `Sink` (interactive-output spec §4–§6): a strictly-additive
//! skin over [`TextSink`](super::sink::TextSink). The **stdout** path is identical to
//! `TextSink` — the answer bytes are byte-for-byte the building-block contract, and
//! `--thinking` reasoning stays on stdout (merely dim-wrapped). All human chrome —
//! tool-call lines, the finish/usage footer, styled errors — goes to **stderr** with a
//! colored sigil gutter. Constructed only when `Style::is_pretty()`; the plain path is
//! the unchanged `TextSink`.

use std::io::{self, Write};

use super::style::{Glyph, Sgr, Style};
use crate::canonical::{ContentKind, Delta, Event, FinishReason, Usage};

/// The pretty sink. `out`/`err` are the split channels; `style` owns every escape and
/// glyph; `pending_sep` is the one-shot `\n` the first answer byte owes after thinking
/// (the `TextSink` mechanism, §5.3). `tool` accumulates the name + streamed JSON args of
/// the open tool block; `usage` buffers token counts until `Finish` flushes the footer.
pub struct PrettySink<O: Write, E: Write> {
    out: O,
    err: E,
    style: Style,
    thinking: bool,
    pending_sep: bool,
    tool: Option<(String, String)>,
    usage: Usage,
}

impl<O: Write, E: Write> PrettySink<O, E> {
    pub fn new(out: O, err: E, thinking: bool, style: Style) -> Self {
        Self {
            out,
            err,
            style,
            thinking,
            pending_sep: false,
            tool: None,
            usage: Usage::default(),
        }
    }

    /// Flush the accumulated tool block as one stderr gutter line: a yellow `⚙` gutter,
    /// the name bold, the args dim. Cleared after; a `ContentStop` with no open tool
    /// (a text/thinking block) is a no-op.
    fn flush_tool(&mut self) -> io::Result<()> {
        let Some((name, args)) = self.tool.take() else {
            return Ok(());
        };
        writeln!(
            self.err,
            "{} {} {}",
            self.style.paint(Sgr::Yellow, self.style.glyph(Glyph::Tool)),
            self.style.paint(Sgr::Bold, &name),
            self.style.paint(Sgr::Dim, &args),
        )?;
        self.err.flush()
    }

    /// The finish/usage footer on `Finish` (spec §5): a green `✓` gutter, then a dim
    /// `stop · 312 in · 47 out` — cache counts appended only when present and non-zero.
    fn footer(&mut self, reason: &FinishReason) -> io::Result<()> {
        let mut line = finish_label(reason);
        for (count, label) in [
            (self.usage.input_tokens, "in"),
            (self.usage.output_tokens, "out"),
            (self.usage.cache_read_tokens, "cache_r"),
            (self.usage.cache_write_tokens, "cache_w"),
        ] {
            if let Some(n) = count.filter(|n| *n > 0) {
                line.push_str(&format!(" · {n} {label}"));
            }
        }
        writeln!(
            self.err,
            "{} {}",
            self.style
                .paint(Sgr::Green, self.style.glyph(Glyph::Footer)),
            self.style.paint(Sgr::Dim, &line),
        )?;
        self.err.flush()
    }
}

impl<O: Write, E: Write> super::sink::Sink for PrettySink<O, E> {
    fn write(&mut self, ev: &Event) -> io::Result<()> {
        match ev {
            // Open a tool block: start accumulating its name + args for the stderr line.
            Event::ContentStart {
                kind: ContentKind::ToolUse { name, .. },
                ..
            } => {
                self.tool = Some((name.clone(), String::new()));
                Ok(())
            }
            // Tool argument fragments accumulate onto the open tool line (dropped if no
            // tool block is open — a JsonDelta only ever rides a tool block).
            Event::ContentDelta {
                delta: Delta::JsonDelta(frag),
                ..
            } => {
                if let Some((_, args)) = &mut self.tool {
                    args.push_str(frag);
                }
                Ok(())
            }
            // `--thinking` only (spec §6): the reasoning on stdout, dim-wrapped, arming
            // the one-shot separator the answer owes. Plain `--text` leaves the guard
            // false, so a thinking delta falls through to `_` and drops, as `TextSink`.
            Event::ContentDelta {
                delta: Delta::ThinkingDelta(text),
                ..
            } if self.thinking => {
                self.out
                    .write_all(self.style.paint(Sgr::Dim, text).as_bytes())?;
                self.pending_sep = true;
                self.out.flush()
            }
            // The answer: byte-identical to `TextSink` — the one-shot separator, then the
            // raw text, never styled (the building-block contract, spec §4).
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
            // Close a block: a tool block flushes its stderr line; text/thinking no-op.
            Event::ContentStop { .. } => self.flush_tool(),
            // Usage arrives in pieces (a provider may report input at message_start and
            // output at the close, §3.6): merge each present counter so the footer holds
            // the full picture, never the last partial alone.
            Event::Usage(usage) => {
                merge(&mut self.usage.input_tokens, usage.input_tokens);
                merge(&mut self.usage.output_tokens, usage.output_tokens);
                merge(&mut self.usage.cache_read_tokens, usage.cache_read_tokens);
                merge(&mut self.usage.cache_write_tokens, usage.cache_write_tokens);
                Ok(())
            }
            Event::Finish { reason } => self.footer(reason),
            // A styled error on stderr (spec §5); the exit code is `pump`'s, untouched.
            // Flush any open tool block FIRST, so a mid-stream-truncated tool call reads
            // `⚙ name {partial` *then* the red `✗` (tool-being-built, then the failure):
            // the streaming-drop/EOF paths (`run::respond::stream`) emit no `ContentStop`
            // to close the block.
            Event::Error(err) => {
                self.flush_tool()?;
                writeln!(
                    self.err,
                    "{} {}",
                    self.style.paint(Sgr::Red, self.style.glyph(Glyph::Error)),
                    err.message,
                )?;
                self.err.flush()
            }
            // The universal safety net: `End` fires on EVERY run (`respond::stream` chains
            // it after the drop/EOF outcomes), so a bare-EOF truncation with NO `Error`
            // event still surfaces its open tool block. A no-op once `ContentStop`/`Error`
            // already flushed — the happy path is unchanged.
            Event::End => self.flush_tool(),
            _ => Ok(()),
        }
    }
}

/// Fold a freshly-reported counter into the accumulator: a present `next` overwrites,
/// an absent one leaves the prior value (so a later partial Usage never erases an
/// earlier counter it does not carry).
fn merge(slot: &mut Option<u32>, next: Option<u32>) {
    if next.is_some() {
        *slot = next;
    }
}

/// The footer's finish-reason label (spec §5). A refusal names its category so the
/// human sees *why* it stopped; every other reason is its bare token.
fn finish_label(reason: &FinishReason) -> String {
    match reason {
        FinishReason::Stop => "stop".to_owned(),
        FinishReason::Length => "length".to_owned(),
        FinishReason::ToolUse => "tool_use".to_owned(),
        FinishReason::StopSequence => "stop_sequence".to_owned(),
        FinishReason::Pause => "pause".to_owned(),
        FinishReason::Refusal { category, .. } => format!("refusal: {category}"),
        FinishReason::Other(reason) => reason.clone(),
    }
}
