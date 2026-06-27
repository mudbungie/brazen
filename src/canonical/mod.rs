//! The canonical model (§3): one request type and one event vocabulary are
//! authoritative; every protocol is a lossy projection onto them and back.

pub mod error;
pub mod event;
pub mod model;
pub mod request;
mod request_de;

pub use error::{CanonicalError, ErrorKind, ExitClass};
pub use event::{ContentKind, Delta, Event, FinishReason, Usage};
// CLI-unreachable: the handshake version is read internally via the leaf path; this
// root re-export feeds only the `#[cfg(test)]` lib prelude (arch §9.8).
#[cfg(test)]
pub(crate) use event::EVENT_SCHEMA_VERSION;
pub use model::{select_model, Model, Provenance};
pub use request::{CanonicalRequest, Content, ImageSource, Message, Role, Tool, ToolChoice};
