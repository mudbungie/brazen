//! Input resolution (§5.5): a real pipe and `--input FILE` arrive as the SAME
//! `Box<dyn Read>` — the distinction dies at construction and never becomes a
//! downstream branch. A file's EOF and a closed pipe's EOF are both `Ok(0)`.
//! `read_request` is the one funnel for both request channels — a positional
//! prompt (argv) and a canonical request (stdin) — into one `CanonicalRequest`.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

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

/// Read the `-f`/`--file` attachments into ordered `Content::Text` parts (§5.5).
/// Each named file's whole contents become one text part; the parts precede the
/// prompt in the one user message. One `fs::read_to_string` per path folds **all
/// three** failure modes a text attachment can have into one `io::Error` — a
/// missing file, an unreadable file, AND a non-UTF-8 file (a text part is UTF-8) —
/// returned with the offending path so the caller maps it to exit **66**
/// (`EX_NOINPUT`), like `--input`. Empty `paths` yields no parts: a run with no
/// `-f` is just this general path with nothing to read (no special case).
pub fn read_files(paths: &[PathBuf]) -> Result<Vec<Content>, (PathBuf, io::Error)> {
    paths
        .iter()
        .map(|p| {
            fs::read_to_string(p)
                .map(Content::Text)
                .map_err(|e| (p.clone(), e))
        })
        .collect()
}

/// Resolve the request from its channels (§5.5): the positional `prompt` (argv),
/// the `-f` `files` content-attach parts, and a canonical request on `reader`
/// (stdin / `--input`). A present prompt **wins and `reader` is never read** — the
/// POSIX filter idiom: a program reads stdin only when it needs it, and an unread
/// pipe is the *writer's* concern (`EPIPE`/`SIGPIPE` on its next write), exactly
/// like `head`. So `bz "hi"` never blocks on a tty and never has to probe one. The
/// pick is explicit — the positional *is* the signal. The user message is the file
/// parts (context) followed by the prompt, in argv order: `[files…, Text(prompt)]`;
/// config/flags fill `system`/`model`/gen-params later via `fill_absent`.
///
/// With **no prompt**: files-present builds a user message of just the file parts —
/// *unless* a canonical request is also piped (non-empty `reader`), which has no
/// single merge with loose attachments (§5.5), so it is **refused** (exit 64) rather
/// than guessed. Files-absent is the unchanged canonical channel: `parse(reader)`.
pub fn read_request(
    prompt: Option<&str>,
    files: Vec<Content>,
    reader: &mut dyn Read,
) -> Result<CanonicalRequest, CanonicalError> {
    match prompt {
        Some(prompt) => Ok(user_message(push_text(files, prompt))),
        None if files.is_empty() => parse(reader),
        None => {
            // A pre-assembled request on stdin can't merge with loose file parts (§5.5).
            // Whitespace-tolerant emptiness: an absent/all-whitespace stdin is "no
            // request" (files-only); any non-whitespace byte is a request → refuse.
            let mut buf = Vec::new();
            reader.read_to_end(&mut buf).map_err(read_err)?;
            if buf.iter().any(|b| !b.is_ascii_whitespace()) {
                Err(cannot_combine())
            } else {
                Ok(user_message(files))
            }
        }
    }
}

/// Append the positional prompt as the trailing `Content::Text` part — files are
/// context, the prompt is last (§5.5).
fn push_text(mut parts: Vec<Content>, prompt: &str) -> Vec<Content> {
    parts.push(Content::Text(prompt.to_owned()));
    parts
}

/// Wrap content parts as a single `User` message (the constructor's one turn).
fn user_message(content: Vec<Content>) -> CanonicalRequest {
    CanonicalRequest {
        messages: vec![Message {
            role: Role::User,
            content,
        }],
        ..Default::default()
    }
}

/// `-f` plus a piped canonical request — refused, a usage error (exit 64, §5.5).
fn cannot_combine() -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Usage,
        message: "cannot combine --file with a canonical request on stdin \
                  (put the file contents in the request's messages instead)"
            .to_owned(),
        provider_detail: None,
        retry_after_seconds: None,
    }
}

/// A stdin read failure while probing for a piped request → an input error (64).
fn read_err(e: io::Error) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::ParseInput,
        message: format!("failed to read stdin: {e}"),
        provider_detail: None,
        retry_after_seconds: None,
    }
}
