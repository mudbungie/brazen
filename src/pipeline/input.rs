//! Input resolution (§5.5): a real pipe and `--input FILE` arrive as the SAME
//! `Box<dyn Read>` — the distinction dies at construction and never becomes a
//! downstream branch. A file's EOF and a closed pipe's EOF are both `Ok(0)`.
//! `read_request` is the one funnel for both request channels — a positional
//! prompt (argv) and a canonical request (stdin) — into one `CanonicalRequest`.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use crate::canonical::{CanonicalError, CanonicalRequest, Content, ErrorKind, Message, Role};
use crate::pipeline::parse::parse;

/// Open the request byte source. `Some(path)` is `--input FILE` (a "simulated
/// pipe"); `None` is stdin. Both yield one `Box<dyn Read>` so parse is blind to
/// the source (§5.5). A missing/unreadable `--input FILE` surfaces as the open
/// `io::Error`, which the caller maps to exit **66** (`EX_NOINPUT`,
/// `ExitClass::NoInput`) — distinct from malformed *content* (64).
pub fn open_input(path: Option<&Path>) -> io::Result<Box<dyn Read>> {
    Ok(match path {
        Some(path) => Box::new(File::open(path)?),
        None => Box::new(io::stdin().lock()),
    })
}

/// Resolve the request from its two mutually-exclusive channels (§5.5): a
/// positional `prompt` (argv) XOR a canonical request on `reader` (stdin /
/// `--input`). A prompt builds `CanonicalRequest{messages:[User Text(prompt)]}`;
/// config/flags fill `system`/`model`/gen-params later via `fill_absent`. Both
/// channels present is a usage error (exit 64) — never a silent pick — so the
/// prompt path still drains `reader` to prove it is empty (a closed/EOF pipe is
/// the common agent case). The drain assumes `reader` reaches EOF; an interactive
/// tty never does, so the `bz` shim hands this an empty reader when stdin is a tty
/// (§5.5) — the tty probe is an impurity kept out of this pure lib. No prompt →
/// `parse` the canonical bytes.
pub fn read_request(
    prompt: Option<&str>,
    reader: &mut dyn Read,
) -> Result<CanonicalRequest, CanonicalError> {
    match prompt {
        Some(prompt) => {
            let mut buf = Vec::new();
            reader.read_to_end(&mut buf).map_err(|e| CanonicalError {
                kind: ErrorKind::ParseInput,
                message: format!("failed to read stdin: {e}"),
                provider_detail: None,
            })?;
            if !buf.is_empty() {
                return Err(CanonicalError {
                    kind: ErrorKind::ParseInput,
                    message: "a positional prompt and a stdin request are mutually exclusive"
                        .to_owned(),
                    provider_detail: None,
                });
            }
            Ok(CanonicalRequest {
                messages: vec![Message {
                    role: Role::User,
                    content: vec![Content::Text(prompt.to_owned())],
                }],
                ..Default::default()
            })
        }
        None => parse(reader),
    }
}
