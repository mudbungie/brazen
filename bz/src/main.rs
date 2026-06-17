//! `bz` — the brazen binary entry point (arch §1, §9.5, §10).
//!
//! The thin shim: restore SIGPIPE, snapshot the real argv/env, dispatch the `login`
//! control plane vs the data plane, wire the native impure impls (in [`native`] and
//! [`transport`]) behind their seams, and materialize the `u8` exit into a
//! `process::ExitCode`. The whole `bz` crate is coverage-excluded (Makefile `cov`):
//! every native impurity lives here — the rustls `HttpTransport` (the lone `ureq`
//! user), the credential file, the system clock, the browser/loopback/RNG — and the
//! library reaches 100% behind injection.

mod native;
mod transport;

use std::collections::BTreeMap;
use std::io::{self, Read};
use std::process::ExitCode;

use brazen::{Args, CodeReceiver, EnvSnapshot, LoginIo};

use native::{
    random_token, LoopbackReceiver, RealPacer, SystemBrowserLauncher, SystemClock, XdgCredStore,
};
use transport::HttpTransport;

fn main() -> ExitCode {
    restore_sigpipe();
    let args = Args {
        argv: std::env::args().skip(1).collect(),
        env: EnvSnapshot(std::env::vars().collect::<BTreeMap<_, _>>()),
    };
    let code = if args.argv.first().map(String::as_str) == Some("login") {
        login(args)
    } else {
        run(args)
    };
    ExitCode::from(code)
}

/// The data plane: wire the native seams and call `brazen::run`.
fn run(args: Args) -> u8 {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let stderr = io::stderr();
    // An interactive tty never reaches EOF, so the positional-prompt drain that
    // enforces the prompt-XOR-stdin rule (§5.5) would block `bz "hi"` typed at a
    // shell forever. The shim treats an interactive stdin as **absent**: it hands
    // the lib an empty reader, so the drain sees `Ok(0)` and builds the prompt
    // request. A genuine pipe (non-tty) still flows through, so the XOR usage
    // error (64) holds for the real piped-stdin-plus-prompt case. The tty probe
    // is an impurity that, like `restore_sigpipe`, lives only in this shim.
    let mut empty = io::empty();
    let mut locked = stdin.lock();
    let reader: &mut dyn Read = if stdin_is_tty() {
        &mut empty
    } else {
        &mut locked
    };
    brazen::run(
        args,
        reader,
        &mut stdout.lock(),
        &mut stderr.lock(),
        &HttpTransport::new(),
        &XdgCredStore::new(),
        &SystemClock,
    )
}

/// The control plane: wire the native interactive seams + the OS RNG and call
/// `brazen::login`. The loopback receiver is constructed UNBOUND and shared by both
/// flows: `browser_flow` binds it on the row's redirect port once config resolves
/// (auth §10.1), and the device flow never touches it — so there is no flow split
/// here. A bind failure surfaces from inside the flow as an auth error (→77).
fn login(args: Args) -> u8 {
    let receiver = LoopbackReceiver::new();
    dispatch_login(args, &receiver)
}

/// Wire the remaining native seams + OS RNG and run the login flow against the
/// `receiver`.
fn dispatch_login(args: Args, receiver: &dyn CodeReceiver) -> u8 {
    let stderr = io::stderr();
    let (transport, store, clock) = (HttpTransport::new(), XdgCredStore::new(), SystemClock);
    let (browser, pacer) = (SystemBrowserLauncher, RealPacer);
    let (verifier, state) = (random_token(), random_token());
    let mut stderr = stderr.lock();
    let mut io = LoginIo {
        stderr: &mut stderr,
        transport: &transport,
        store: &store,
        clock: &clock,
        browser: &browser,
        receiver,
        pacer: &pacer,
        verifier: &verifier,
        state: &state,
    };
    brazen::login(&args, &mut io)
}

/// Restore SIGPIPE to `SIG_DFL` (arch §5.8): Rust sets `SIG_IGN`, which would turn
/// a closed-stdout write into a `BrokenPipe` error instead of letting the kernel
/// kill us with the signal (exit 141, like `cat | head`). Windows has no SIGPIPE —
/// `pump` maps its `BrokenPipe` write error to the same 141 there.
#[cfg(unix)]
fn restore_sigpipe() {
    // SAFETY: a single libc call at startup, before any thread is spawned; it only
    // resets a signal disposition. The lib forbids unsafe — this is the shim's.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn restore_sigpipe() {}

/// Is stdin (fd 0) an interactive terminal (arch §5.5)? An interactive tty never
/// reaches EOF, so the lib's positional-prompt drain would block on it; the shim
/// probes here — an impurity, the sibling of `restore_sigpipe` — and treats a tty
/// as absent input. Non-Unix never probes (no tty hang in scope): stdin is always
/// treated as present, the prior behavior.
#[cfg(unix)]
fn stdin_is_tty() -> bool {
    // SAFETY: `isatty` is a read-only query on a file descriptor — no memory, no
    // threads, no state. The lib forbids unsafe; this is the shim's, like §5.8.
    unsafe { libc::isatty(libc::STDIN_FILENO) == 1 }
}

#[cfg(not(unix))]
fn stdin_is_tty() -> bool {
    false
}
