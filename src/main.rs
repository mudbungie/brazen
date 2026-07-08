//! `bz` — the brazen binary entry point (arch §1, §9.5, §10).
//!
//! The thin shim: restore SIGPIPE, snapshot the real argv/env, route the control
//! short-circuit flags (`--login` / `--list-models`, read via the lib's authoritative
//! `route`, §5.10.1) vs the data plane, wire the native impure impls (in [`native`])
//! behind their seams, and materialize the `u8` exit into a `process::ExitCode`.
//! This bin and [`native`] are coverage-excluded (Makefile `cov`): every native
//! impurity lives here — the rustls `HttpTransport` (the lone `ureq` user), the
//! credential file, the system clock, the browser/loopback/RNG — and the library
//! reaches 100% behind injection.

mod native;

use std::collections::BTreeMap;
use std::io::{self, Read};
use std::process::ExitCode;

use brazen::{
    count_tokens, route, Args, CodeReceiver, CountIo, EnvSnapshot, Host, ListIo, LoginIo, Route,
};

use native::{
    random_token, HttpTransport, LoopbackReceiver, RealPacer, SystemBrowserLauncher, SystemClock,
    XdgCredStore, XdgModelCache,
};

fn main() -> ExitCode {
    restore_sigpipe();
    let args = Args {
        argv: std::env::args().skip(1).collect(),
        env: EnvSnapshot(std::env::vars().collect::<BTreeMap<_, _>>()),
        // The one tty fact the pure lib can't observe (§5.5): snapshotted here next
        // to argv/env so `run` can turn a bare interactive invocation (no prompt, no
        // stdin request) into a usage hint. A pipe is `false`, so the scripted path
        // is unchanged. Also the reader pick below, so the probe runs once.
        tty: stdin_is_tty(),
        // The second isatty fact (interactive-output §2): whether STDOUT is a tty, so
        // `Style::resolve` can pick the pretty text skin. Sibling of `stdin_is_tty`; a
        // pipe/redirect is `false`, leaving the building-block stdout contract intact.
        stdout_tty: stdout_is_tty(),
    };
    // Route on the control flag, not `argv[0]` (§5.10.1): a leading bare word is always
    // a prompt, so `bz "login"`/`bz "list-models"` are valid prompts forever. `route`
    // re-uses the lib's `parse_args`, so the shim and lib can never disagree; a parse
    // error routes to `run`, which re-parses and surfaces the authoritative exit.
    let code = match route(&args.argv) {
        Route::Login => login(args),
        Route::ListModels => list_models(args),
        Route::CountTokens => count(args),
        Route::Run => run(args),
    };
    ExitCode::from(code)
}

/// The data plane: wire the native seams and call `brazen::run`.
fn run(args: Args) -> u8 {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let stderr = io::stderr();
    // A present positional prompt WINS and the lib never reads stdin (§5.5), so this
    // empty-reader swap matters only for the NO-positional case: bare `bz` typed at an
    // interactive tty would otherwise block forever on a stdin that never reaches EOF.
    // The shim treats an interactive stdin as **absent**: it hands the lib an empty
    // reader, so the no-positional read sees `Ok(0)` and `run` prints the friendly
    // bare-invocation usage (64) instead of blocking. A genuine pipe (non-tty) still
    // flows through and is parsed as a canonical request. The tty probe is an impurity
    // that, like `restore_sigpipe`, lives only in this shim.
    let mut empty = io::empty();
    let mut locked = stdin.lock();
    let reader: &mut dyn Read = if args.tty { &mut empty } else { &mut locked };
    // The seams live in locals so the `Host` references outlive the `run` call (a
    // struct-literal `&Transport::new()` would drop the temporary at the `let`'s end).
    let (transport, store, cache, clock) = (
        HttpTransport::new(),
        XdgCredStore::new(),
        XdgModelCache::new(),
        SystemClock,
    );
    let host = Host {
        transport: &transport,
        store: &store,
        cache: &cache,
        clock: &clock,
    };
    brazen::run(args, reader, &mut stdout.lock(), &mut stderr.lock(), &host)
}

/// The `--list-models` control flag (model-discovery §2): the sibling of `--login`
/// and the data plane — it shares the data-plane seams (the one models GET reuses the
/// `HttpTransport`/`XdgCredStore`/`SystemClock`, auth/refresh and all) but has its
/// own output shape, so it branches once here and never enters `run`'s request
/// pipeline. No interactive seams: it never blocks on a browser.
fn list_models(args: Args) -> u8 {
    let stdout = io::stdout();
    let stderr = io::stderr();
    let mut io = ListIo {
        stdout: &mut stdout.lock(),
        stderr: &mut stderr.lock(),
        transport: &HttpTransport::new(),
        store: &XdgCredStore::new(),
        cache: &XdgModelCache::new(),
        clock: &SystemClock,
    };
    brazen::list_models(&args, &mut io)
}

/// The `--count-tokens` control flag (architecture §5.10.1): the sibling of the data
/// plane and `--list-models` — it CONSUMES a request (so it takes the same tty-aware
/// stdin reader as `run`) and shares the data-plane seams for the one count round-trip
/// (the model seed resolves against the same `XdgModelCache`, read-only), but has its own
/// output shape, so it branches once here and never enters `run`'s pipeline.
fn count(args: Args) -> u8 {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let stderr = io::stderr();
    // The tty reader swap, exactly as `run` does it (§5.5): an interactive stdin is treated
    // as absent so a no-positional `bz --count-tokens` at a shell doesn't block forever.
    let mut empty = io::empty();
    let mut locked = stdin.lock();
    let reader: &mut dyn Read = if args.tty { &mut empty } else { &mut locked };
    let (transport, store, cache, clock) = (
        HttpTransport::new(),
        XdgCredStore::new(),
        XdgModelCache::new(),
        SystemClock,
    );
    let mut io = CountIo {
        stdout: &mut stdout.lock(),
        stderr: &mut stderr.lock(),
        transport: &transport,
        store: &store,
        cache: &cache,
        clock: &clock,
    };
    count_tokens(&args, reader, &mut io)
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
    let stdout = io::stdout();
    let stderr = io::stderr();
    let (transport, store, clock) = (HttpTransport::new(), XdgCredStore::new(), SystemClock);
    let (browser, pacer) = (SystemBrowserLauncher, RealPacer);
    let (verifier, state) = (random_token(), random_token());
    let mut stdout = stdout.lock();
    let mut stderr = stderr.lock();
    let mut io = LoginIo {
        stdout: &mut stdout,
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
/// `ExitClass::from_io` maps the `BrokenPipe` write error to the same 141 there (the
/// byte adapter's shared mapping, arch §5.1/§5.8).
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
/// reaches EOF, so a no-positional `bz` that reads stdin would block on it; the shim
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

/// Is stdout (fd 1) an interactive terminal (interactive-output §2)? The second
/// isatty fact, probed here as the sibling of `stdin_is_tty` — an impurity kept out
/// of the pure lib — and carried on `Args.stdout_tty` so `Style::resolve` can pick
/// the pretty text skin. Non-Unix never probes: a pipe-equivalent `false`, so the
/// skin stays off and the stdout contract is unchanged.
#[cfg(unix)]
fn stdout_is_tty() -> bool {
    // SAFETY: `isatty` is a read-only query on a file descriptor — no memory, no
    // threads, no state. The lib forbids unsafe; this is the shim's, like §5.8.
    unsafe { libc::isatty(libc::STDOUT_FILENO) == 1 }
}

#[cfg(not(unix))]
fn stdout_is_tty() -> bool {
    false
}
