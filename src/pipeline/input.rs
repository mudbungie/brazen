//! Input resolution (§5.5): a real pipe and `--input FILE` arrive as the SAME
//! `Box<dyn Read>` — the distinction dies at construction and never becomes a
//! downstream branch. A file's EOF and a closed pipe's EOF are both `Ok(0)`.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

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
