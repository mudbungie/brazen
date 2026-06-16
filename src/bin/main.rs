//! `bz` — the brazen binary entry point (arch §1, §9.5, §10).
//!
//! The thin shim: restore SIGPIPE, snapshot the real argv/env, dispatch the `login`
//! control plane vs the data plane, wire the native impure impls (in [`native`])
//! behind their seams, and materialize the `u8` exit into a `process::ExitCode`.
//! The whole `src/bin/` shim is coverage-excluded (Makefile `cov`): every native
//! impurity lives here — the rustls `HttpTransport`, the credential file, the
//! system clock, the browser/loopback/RNG — and the library reaches 100% behind
//! injection.

mod native;

use std::collections::BTreeMap;
use std::io;
use std::process::ExitCode;

use brazen::{Args, CodeReceiver, EnvSnapshot, LoginIo};

use native::{
    random_token, HttpTransport, LoopbackReceiver, NullReceiver, RealPacer, SystemBrowserLauncher,
    SystemClock, XdgCredStore,
};

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
    brazen::run(
        args,
        &mut stdin.lock(),
        &mut stdout.lock(),
        &mut stderr.lock(),
        &HttpTransport::new(),
        &XdgCredStore::new(),
        &SystemClock,
    )
}

/// The control plane: wire the native interactive seams + the OS RNG and call
/// `brazen::login`. The loopback receiver is bound only for the `--browser` flow;
/// the headless device flow uses no socket (a `NullReceiver`).
fn login(args: Args) -> u8 {
    // Bind the loopback listener up front (only for `--browser`) so it outlives the
    // `LoginIo` that borrows it; the device flow never touches a socket.
    let loopback = if args.argv.iter().any(|a| a == "--browser") {
        match LoopbackReceiver::bind() {
            Ok(receiver) => receiver,
            Err(e) => {
                eprintln!("could not bind loopback listener: {e}");
                return 77;
            }
        }
    } else {
        return device_login(args);
    };
    dispatch_login(args, &loopback)
}

/// Headless device flow: no loopback receiver.
fn device_login(args: Args) -> u8 {
    dispatch_login(args, &NullReceiver)
}

/// Wire the remaining native seams + OS RNG and run the login flow against the
/// chosen `receiver`.
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
