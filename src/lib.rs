#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]
//! `brazen` — the pure, fully-tested core of a stateless LLM adapter.
//!
//! This crate holds the canonical model (the single source of truth every
//! provider/protocol projects to and from) and the traits behind which all
//! impurity (network, clock, credentials, browser) is injected. The `bz`
//! binary owns the native impls; the library reaches 100% coverage on its own
//! because nothing here touches IO.
//!
//! Landed so far: the canonical request/event types and the error model
//! (§3.1–§3.3, §8), plus the pure pipeline — input resolution, canonical-in
//! parsing, and the output projections + pump loop (§5 of
//! `specs/architecture.md`).

pub mod canonical;
pub mod pipeline;

pub use canonical::{
    CanonicalError, CanonicalRequest, Content, ContentKind, Delta, ErrorKind, Event, ExitClass,
    FinishReason, ImageSource, Message, Role, Tool, ToolChoice, Usage, EVENT_SCHEMA_VERSION,
};
pub use pipeline::{open_input, parse, pump, NdjsonSink, RawSink, Sink, TextSink};
