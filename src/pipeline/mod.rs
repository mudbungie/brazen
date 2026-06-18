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
pub use parse::parse;
pub use pretty::PrettySink;
pub use sink::{pump, NdjsonSink, RawSink, Sink, TextSink};
pub use style::{Glyph, Sgr, Style};
