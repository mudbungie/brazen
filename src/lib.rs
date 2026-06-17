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
pub mod cli;
pub mod config;
pub mod os;
pub mod pipeline;
pub mod protocol;
pub mod registry;
pub mod run;
pub mod store;
pub mod testing;
pub mod transport;

pub use auth::login::{
    login, parse_login_args, BrowserLauncher, CodeReceiver, LoginArgs, LoginIo, Pacer,
};
pub use auth::{
    build_authorize_url, build_token_exchange_request, is_expired, parse_callback,
    parse_token_response, query_from_request_line, Auth, AuthCtx, AuthError, Callback, Grant,
    OAuth2Auth, OAuthConfig, Pkce, RedirectSpec, StaticSecretAuth, TokenResponse, SKEW,
};
pub use canonical::{
    CanonicalError, CanonicalRequest, Content, ContentKind, Delta, ErrorKind, Event, ExitClass,
    FinishReason, ImageSource, Message, Role, Tool, ToolChoice, Usage, EVENT_SCHEMA_VERSION,
};
pub use cli::{parse_args, Args, Flags};
pub use config::provider::{AuthId, HeaderScheme, HeaderSpec, ProtocolId, Provider};
pub use config::{
    config_path, defaults, dump_config, fill_absent, parse_config, partial_from_env, redact,
    ConfigError, EnvSnapshot, OutMode, PartialConfig, PartialProvider, ResolvedConfig,
};
pub use os::browser_argv;
pub use pipeline::{open_input, parse, pump, read_request, NdjsonSink, RawSink, Sink, TextSink};
pub use protocol::{
    DecodeState, Decoder, Frame, Framing, OpenBlock, Protocol, ProviderCtx, WireRequest,
};
pub use registry::Registry;
pub use run::run;
pub use store::{Clock, Cred, CredStore, Secret};
pub use transport::{Bytes, Timeouts, Transport, TransportResponse};
