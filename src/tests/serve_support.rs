//! Shared harness for the `bz --serve` suites (ingress.md §14): the masquerade
//! config, one `drive` that runs the accept loop to completion over scripted
//! in-memory connections (the scoped loop joins every connection before
//! returning, so response buffers are complete), and the HTTP request/response
//! literals. A subdirectory-style support module like `run_support`;
//! `#![allow(dead_code)]` because each suite uses a subset.
#![allow(dead_code)]

use crate::testing::{FakeClock, MemoryCredStore, ScriptedBind, ScriptedListener, Wrote};
use crate::tests::run_support::{args, temp, unused_stash, TempFile};
use crate::{serve, Bind, ServeConn, ServeIo, Transport};

/// The `Sync`-bounded cache alias the drivers share.
pub type ModelCacheSync = dyn crate::ModelCache + Sync;

/// A serve-ready config: `[ingress]` naming the dialect (+ `extra` lines), the
/// alias routing `gpt-4o` to anthropic, and the built-in openai row's `gpt-*`
/// prefix cleared so the alias is the ONE owner (ingress §6).
pub fn masq_cfg(extra: &str) -> TempFile {
    temp(&format!(
        r#"
api_key = "sk-test"

[ingress]
dialect = "openai_chat"
{extra}
[[provider]]
name = "anthropic"
model_aliases = {{ "gpt-4o" = "claude-x" }}

[[provider]]
name = "openai"
model_prefixes = []
"#,
    ))
}

/// Drive `serve` to completion over a scripted connection queue.
pub fn drive(
    cfg: &TempFile,
    conns: Vec<Box<dyn ServeConn>>,
    tx: &(dyn Transport + Sync),
    cache: &ModelCacheSync,
) -> (u8, String, String) {
    let bind = ScriptedBind::new(Box::new(ScriptedListener::new(conns)));
    drive_bound(cfg, &bind, tx, cache)
}

/// Drive `serve` against an explicit `Bind` (the bind-failure / bound-address tests).
pub fn drive_bound(
    cfg: &TempFile,
    bind: &dyn Bind,
    tx: &(dyn Transport + Sync),
    cache: &ModelCacheSync,
) -> (u8, String, String) {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let store = MemoryCredStore::new();
    let clock = FakeClock::new(1_700_000_000);
    let stash = unused_stash();
    let mut io = ServeIo {
        stdout: &mut stdout,
        stderr: &mut stderr,
        bind,
        transport: tx,
        store: &store,
        cache,
        clock: &clock,
        stash: &stash,
    };
    let code = serve(
        &args(&["--serve", "--config", cfg.0.to_str().unwrap()], &[]),
        &mut io,
    );
    (
        code,
        String::from_utf8_lossy(&stdout).into_owned(),
        String::from_utf8_lossy(&stderr).into_owned(),
    )
}

/// Serialize one HTTP/1.1 POST with a `content-length` body.
pub fn post(path: &str, body: &str, extra_headers: &str) -> Vec<u8> {
    format!(
        "POST {path} HTTP/1.1\r\ncontent-length: {}\r\n{extra_headers}\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// The aggregate-shape data request (`stream` absent, ingress §10).
pub const AGG: &str = r#"{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}]}"#;
/// The SSE-shape data request (`stream:true`).
pub const SSE: &str =
    r#"{"model":"gpt-4o","stream":true,"messages":[{"role":"user","content":"hi"}]}"#;

/// A [`MemConn`](crate::testing::MemConn) response buffer as a string.
pub fn wrote_str(wrote: &Wrote) -> String {
    String::from_utf8_lossy(&wrote.lock().unwrap()).into_owned()
}
