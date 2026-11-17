//! The row's static headers as ONE query (config §4.3.2): `beta_headers`, overlaid
//! by `header_commands` — a header whose value is the trimmed stdout of an
//! operator command, run per request over the transport seam's subprocess target.
//! A command that yields nothing usable overlays nothing, so the same-named
//! `beta_headers` entry stands: the general path with an empty overlay.

use crate::config::ResolvedConfig;
use crate::protocol::{Envelope, ExecSpec, WireRequest};
use crate::transport::Transport;

/// Every static header this row sends, shared by generation, `--raw`,
/// `--list-models` and `--count-tokens`. Names match case-insensitively, so a
/// command REPLACES the `beta_headers` entry it shares a name with.
pub(crate) fn row_headers(
    cfg: &ResolvedConfig,
    transport: &dyn Transport,
) -> Vec<(String, String)> {
    let mut wire = WireRequest::default();
    for (name, value) in &cfg.provider.beta_headers {
        wire.set_header(name, value);
    }
    for (name, argv) in &cfg.provider.header_commands {
        if let Some(value) = output(argv, cfg, transport) {
            wire.set_header(name, &value);
        }
    }
    wire.headers
}

/// One command's header value, or `None` for anything that is not one: an empty
/// argv, a failed spawn, a broken or non-UTF-8 stdout, empty output, or a control
/// character (a newline would be a second header).
fn output(argv: &[String], cfg: &ResolvedConfig, transport: &dyn Transport) -> Option<String> {
    let (program, args) = argv.split_first()?;
    let wire = WireRequest {
        timeouts: cfg.timeouts(),
        exec: Some(ExecSpec {
            program: program.clone(),
            args: args.to_vec(),
            envelope: Envelope::Body,
        }),
        ..WireRequest::default()
    };
    let mut bytes = Vec::new();
    for chunk in transport.send(wire).ok()?.body {
        bytes.extend(chunk.ok()?);
    }
    let value = String::from_utf8(bytes).ok()?.trim().to_owned();
    (!value.is_empty() && !value.chars().any(char::is_control)).then_some(value)
}
