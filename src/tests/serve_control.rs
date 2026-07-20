//! `bz --serve` pre-loop control plumbing (ingress.md §6, §7; arch §5.10.1):
//! the missing/invalid `[ingress]` table (78), an unknown configured dialect
//! (78), a bind failure (69) naming the address, the resolved `listen` address
//! reaching the bind seam, the `--help`/`--version` probes and flag errors
//! short-circuiting before any bind, and `route` dispatching `--serve` while
//! the bare-prompt namespace survives.

use std::io;

use crate::testing::{
    FakeClock, MemoryCredStore, MemoryModelCache, MockTransport, ScriptedBind, ScriptedListener,
};
use crate::tests::run_support::{args, temp, unused_stash, BASIC};
use crate::tests::serve_support::*;
use crate::{serve, Bind, Listener, ServeIo};

/// A `Bind` that always refuses — the port-already-held shape.
struct FailBind;
impl Bind for FailBind {
    fn bind(&self, _: core::net::SocketAddr) -> io::Result<Box<dyn Listener>> {
        Err(io::Error::new(io::ErrorKind::AddrInUse, "address in use"))
    }
}

#[test]
fn serve_without_an_ingress_table_is_config_78() {
    let cfg = temp("api_key = \"sk\"\n");
    let (code, _, err) = drive(
        &cfg,
        vec![],
        &MockTransport::ok(vec![BASIC]),
        &MemoryModelCache::new(),
    );
    assert_eq!(code, 78);
    assert!(
        err.contains("`--serve` needs an `[ingress]` table"),
        "{err}"
    );
}

#[test]
fn an_unknown_configured_dialect_is_config_78() {
    let cfg = temp("[ingress]\ndialect = \"smoke_signals\"\n");
    let (code, _, err) = drive(
        &cfg,
        vec![],
        &MockTransport::ok(vec![BASIC]),
        &MemoryModelCache::new(),
    );
    assert_eq!(code, 78);
    assert!(
        err.contains("unknown ingress dialect `smoke_signals`"),
        "{err}"
    );
}

#[test]
fn a_bind_failure_is_transport_69_naming_the_address() {
    let cfg = masq_cfg("listen = \"127.0.0.1:19\"\n");
    let (code, _, err) = drive_bound(
        &cfg,
        &FailBind,
        &MockTransport::ok(vec![BASIC]),
        &MemoryModelCache::new(),
    );
    assert_eq!(code, 69);
    assert!(err.contains("cannot bind 127.0.0.1:19"), "{err}");
}

#[test]
fn serve_binds_the_resolved_listen_address() {
    let cfg = masq_cfg("listen = \"127.0.0.1:9911\"\n");
    let bind = ScriptedBind::new(Box::new(ScriptedListener::new(vec![])));
    let (code, _, _) = drive_bound(
        &cfg,
        &bind,
        &MockTransport::ok(vec![BASIC]),
        &MemoryModelCache::new(),
    );
    assert_eq!(code, 0, "an empty accept queue is a clean shutdown");
    assert_eq!(bind.bound().unwrap().to_string(), "127.0.0.1:9911");
    // And the double refuses a second bind (its listener is gone).
    assert!(bind.bind("127.0.0.1:9911".parse().unwrap()).is_err());
}

#[test]
fn the_probes_and_flag_errors_short_circuit_like_every_entry() {
    let cfg = masq_cfg("");
    let bind = ScriptedBind::new(Box::new(ScriptedListener::new(vec![])));
    let mut io_out = Vec::new();
    let mut io_err = Vec::new();
    let store = MemoryCredStore::new();
    let clock = FakeClock::new(0);
    let cache = MemoryModelCache::new();
    let tx = MockTransport::ok(vec![BASIC]);
    let stash = unused_stash();
    for (argv, code, needle) in [
        (vec!["--serve", "--help"], 0, "USAGE"),
        (vec!["--serve", "--skill"], 0, "agent skill card"),
        (vec!["--serve", "--version"], 0, "bz "),
        (vec!["--serve", "--nope"], 64, ""),
        (vec!["--serve", "--list-models"], 64, ""),
    ] {
        io_out.clear();
        io_err.clear();
        let mut io = ServeIo {
            stdout: &mut io_out,
            stderr: &mut io_err,
            bind: &bind,
            transport: &tx,
            store: &store,
            cache: &cache,
            clock: &clock,
            stash: &stash,
        };
        let mut full = argv.clone();
        let path = cfg.0.to_str().unwrap();
        full.extend(["--config", path]);
        let got = serve(&args(&full, &[]), &mut io);
        assert_eq!(got, code, "{argv:?}");
        assert!(
            String::from_utf8_lossy(&io_out).contains(needle),
            "{argv:?}: {}",
            String::from_utf8_lossy(&io_out)
        );
    }
    assert!(bind.bound().is_none(), "no probe or error ever bound");
}

#[test]
fn route_dispatches_serve_and_the_prompt_namespace_survives() {
    assert!(matches!(
        crate::route(&["--serve".into()]),
        crate::Route::Serve
    ));
    // A bare word is ALWAYS a prompt (§5.10.1) — "serve" included, forever.
    assert!(matches!(crate::route(&["serve".into()]), crate::Route::Run));
}
