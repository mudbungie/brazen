//! The canonical model (§3): one request type and one event vocabulary are
//! authoritative; every protocol is a lossy projection onto them and back.

pub mod error;
pub mod event;
pub mod request;
mod request_de;

pub use error::{CanonicalError, ErrorKind, ExitClass};
pub use event::{ContentKind, Delta, Event, FinishReason, Usage, EVENT_SCHEMA_VERSION};
pub use request::{CanonicalRequest, Content, ImageSource, Message, Role, Tool, ToolChoice};
