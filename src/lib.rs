#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]
// The compiler half of the §9.8 interface-parity invariant: a public signature may not
// expose a private type. `tests/interface_parity.rs` enforces the other half (no public
// type is unreachable from an entry point); together they pin the surface exactly.
#![deny(private_interfaces, private_bounds)]
//! `brazen` — the engine behind the `bz` command, and the pure, fully-tested core
//! of a stateless LLM adapter.
//!
//! `cargo install brazen` builds the `bz` binary (this crate's `[[bin]]`); the
//! library is that binary's engine, published alongside it in case it is useful as
//! a dependency. **Its API is not yet a stability contract** — pin an exact version
//! if you build on it.
//!
//! This crate holds the canonical model (the single source of truth every
//! provider/protocol projects to and from) and the traits behind which all
//! impurity (network, clock, credentials, browser) is injected. The `bz` binary
//! and `src/native/` own the native impls; the library reaches 100% coverage on its
//! own because nothing here touches IO — a boundary `tests/purity.rs` keeps real now
//! that the bin and the library share one crate.
//!
//! Beyond the canonical model and error model (the dependency root), this crate
//! defines the *seams* the rest of the pipeline plugs into: the `Protocol`,
//! `Auth`, `Transport`, `CredStore`, and `Clock` traits, the data records they
//! exchange (`WireRequest`, `Provider`, `Cred`, …), and the `Registry` that
//! dispatches by id without ever matching a vendor name (§4 of the architecture).
//! It also holds the pure pipeline — input resolution, canonical-in parsing, and
//! the output projections + pump loop (§5). Concrete protocol/auth/transport
//! impls land via their own tasks; the shared test doubles live in [`testing`].

// Modules are PRIVATE (crate-visible, never `pub mod`): the public surface is
// re-exported below by hand, so nothing leaks via a module path. See arch §9.8 — the
// public lib API is *exactly* the capability set the `bz` CLI exposes (bidirectional
// exclusive parity), enforced by `tests/interface_parity.rs`.
mod auth;
mod canonical;
mod cli;
mod config;
mod os;
mod pipeline;
mod protocol;
mod registry;
mod run;
mod store;
mod transport;

// The in-lib test doubles + the relocated unit/integration suite. Both are
// `#[cfg(test)]`: they exist only in the test build, so they never widen the
// published surface and never become dead code in the release binary (§9.8).
#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;

// ---- The public library surface: the typed interface + what drives it ----
// The interface is the typed I/O — a `CanonicalRequest` in, an `Event` stream out (the
// canonical model, §3) — exposed through the `generate` entry point, plus the seams and
// config that drive it; the byte `bz` CLI is one serialization of exactly this (§9.8,
// bl-b4a9). Modules are private; this `pub use` block is the whole surface, and it is
// EXACTLY the type-closure of the entry points below — `tests/interface_parity.rs`
// derives that closure mechanically and asserts equality, so no type leaks or orphans.
pub use auth::login::{login, BrowserLauncher, CodeReceiver, LoginIo, Pacer};
pub use auth::{query_from_request_line, OAuthConfig, RedirectSpec};
pub use canonical::{
    CanonicalError, CanonicalRequest, Content, ContentKind, Delta, ErrorKind, Event, FinishReason,
    ImageSource, Message, Model, Role, Tool, ToolChoice, Usage, EVENT_SCHEMA_VERSION,
};
pub use cli::Args;
pub use config::provider::{AuthId, HeaderScheme, HeaderSpec, ProtocolId, Provider};
pub use config::{EnvSnapshot, OutMode, ResolvedConfig};
pub use os::browser_argv;
pub use protocol::{Method, WireRequest};
pub use run::{generate, list_models, run, Host, ListIo};
pub use store::{
    parse_ambient, AmbientFormat, AmbientSpec, Clock, Cred, CredStore, ModelCache, Secret,
};
pub use transport::{Bytes, Timeouts, Transport, TransportResponse};

// ---- Test-only internal prelude (NOT part of the public surface) ----
// The relocated in-crate tests (`src/tests/`) exercise internals NOT on the interface:
// the pure parsers/encoders, the config fold, the OAuth wire builders, the registry, the
// sinks. Re-exporting them at the crate root as `pub(crate)` under `#[cfg(test)]` keeps
// the tests ergonomic (`crate::Foo`) WITHOUT publishing them — `pub(crate)` is invisible
// to `cargo public-api`/external consumers and `#[cfg(test)]` strips it from every
// non-test build. So test layout never drives the surface (§9.8).
#[cfg(test)]
pub(crate) use auth::login::parse_login_args;
#[cfg(test)]
pub(crate) use auth::{
    build_authorize_url, build_token_exchange_request, is_expired, parse_callback,
    parse_token_response, Auth, AuthCtx, AuthError, Grant, NoAuth, OAuth2Auth, Pkce,
    StaticSecretAuth, TokenResponse,
};
#[cfg(test)]
pub(crate) use canonical::{select_model, ExitClass, Provenance};
#[cfg(test)]
pub(crate) use cli::parse_args;
#[cfg(test)]
pub(crate) use config::{
    config_path, defaults, dump_config, fill_absent, lead_with_preamble, parse_config,
    partial_from_env, redact, strip_unsupported, ConfigError, PartialConfig, PartialProvider,
};
#[cfg(test)]
pub(crate) use pipeline::{
    open_input, parse, pump, read_request, Glyph, NdjsonSink, PrettySink, RawSink, Sgr, Sink,
    Style, TextSink,
};
#[cfg(test)]
pub(crate) use protocol::{DecodeState, Frame, Framing, OpenBlock, Protocol, ProviderCtx};
#[cfg(test)]
pub(crate) use registry::Registry;
