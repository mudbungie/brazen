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

// CLI-unreachable. The data plane drives the sink incrementally in `run::respond`
// (frame-by-frame, streaming); the `pump` batch driver and the `parse` free fn are
// reached only by the `#[cfg(test)]` lib prelude (`parse` runs in-line inside
// `read_request`; `Glyph`/`Sgr` are read via their leaf paths in `style`/`pretty`).
// Gated so they are neither in the public surface nor dead code in release (§9.8).
#[cfg(test)]
pub(crate) use parse::parse;
#[cfg(test)]
pub(crate) use sink::pump;
#[cfg(test)]
pub(crate) use style::{Glyph, Sgr};
