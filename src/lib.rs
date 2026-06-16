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
//! Beyond the canonical model and error model (the dependency root), this crate
//! defines the *seams* the rest of the pipeline plugs into: the `Protocol`,
//! `Auth`, `Transport`, `CredStore`, and `Clock` traits, the data records they
//! exchange (`WireRequest`, `Provider`, `Cred`, …), and the `Registry` that
//! dispatches by id without ever matching a vendor name (§4 of the architecture).
//! It also holds the pure pipeline — input resolution, canonical-in parsing, and
//! the output projections + pump loop (§5). Concrete protocol/auth/transport
//! impls land via their own tasks; the shared test doubles live in [`testing`].

pub mod auth;
pub mod canonical;
pub mod config;
pub mod pipeline;
pub mod protocol;
pub mod registry;
pub mod store;
pub mod testing;
pub mod transport;

pub use auth::{Auth, AuthCtx, OAuthConfig, StaticSecretAuth};
pub use canonical::{
    CanonicalError, CanonicalRequest, Content, ContentKind, Delta, ErrorKind, Event, ExitClass,
    FinishReason, ImageSource, Message, Role, Tool, ToolChoice, Usage, EVENT_SCHEMA_VERSION,
};
pub use config::provider::{AuthId, HeaderScheme, HeaderSpec, ProtocolId, Provider};
pub use config::{
    config_path, defaults, dump_config, fill_absent, parse_config, partial_from_env, redact,
    resolve, ConfigError, EnvSnapshot, OutMode, PartialConfig, PartialProvider, ResolvedConfig,
};
pub use pipeline::{open_input, parse, pump, NdjsonSink, RawSink, Sink, TextSink};
pub use protocol::{
    DecodeState, Decoder, Frame, Framing, OpenBlock, Protocol, ProviderCtx, WireRequest,
};
pub use registry::Registry;
pub use store::{Clock, Cred, CredStore, Secret};
pub use transport::{Bytes, Transport, TransportResponse};
