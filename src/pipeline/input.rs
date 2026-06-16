//! Input resolution (§5.5): a real pipe and `--input FILE` arrive as the SAME
//! `Box<dyn Read>` — the distinction dies at construction and never becomes a
//! downstream branch. A file's EOF and a closed pipe's EOF are both `Ok(0)`.
//! `read_request` is the one funnel for both request channels — a positional
//! prompt (argv) and a canonical request (stdin) — into one `CanonicalRequest`.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use crate::canonical::{CanonicalError, CanonicalRequest, Content, Message, Role};
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

/// Resolve the request from its two channels (§5.5): a positional `prompt` (argv)
/// or a canonical request on `reader` (stdin / `--input`). A present prompt **wins
/// and `reader` is never read** — the POSIX filter idiom: a program reads stdin
/// only when it needs it, and an unread pipe is the *writer's* concern (it gets
/// `EPIPE`/`SIGPIPE` if it keeps writing), exactly like `head`. So `bz "hi"` never
/// blocks on a tty and never has to probe one. The pick is explicit — the
/// positional *is* the signal — so there is no silent sniffing of stdin. A prompt
/// builds `CanonicalRequest{messages:[User Text(prompt)]}`; config/flags fill
/// `system`/`model`/gen-params later via `fill_absent`. No prompt → `parse` the
/// canonical bytes off `reader`.
pub fn read_request(
    prompt: Option<&str>,
    reader: &mut dyn Read,
) -> Result<CanonicalRequest, CanonicalError> {
    match prompt {
        Some(prompt) => Ok(CanonicalRequest {
            messages: vec![Message {
                role: Role::User,
                content: vec![Content::Text(prompt.to_owned())],
            }],
            ..Default::default()
        }),
        None => parse(reader),
    }
}
