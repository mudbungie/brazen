//! The pure pipeline (§5): input resolution, canonical-in parsing, and the
//! output projections + pump loop. Every piece is a pure function of its bytes
//! and the injected writer — no clock, no creds, no network — so the whole
//! module is table-tested from literals and golden streams.

pub mod input;
pub mod parse;
pub mod pretty;
pub mod sink;
pub mod style;

pub use input::{open_input, read_request};
pub use pretty::PrettySink;
pub use sink::{NdjsonSink, RawSink, Sink, TextSink};
pub use style::Style;

// `pump` is the byte adapter `run` drives `generate`'s events through (§5.1) —
// crate-internal, never published, so a `pub(crate)` re-export, not `pub`.
pub(crate) use sink::pump;

// CLI-unreachable. `parse` runs in-line inside `read_request`; `Glyph`/`Sgr` are read
// via their leaf paths in `style`/`pretty`. Reached only by the `#[cfg(test)]` lib
// prelude — gated so they are neither published nor dead code in release (§9.8).
#[cfg(test)]
pub(crate) use parse::parse;
#[cfg(test)]
pub(crate) use style::{Glyph, Sgr};
