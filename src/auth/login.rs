//! `bz --login` — the quarantined control plane (auth §7), a control short-circuit
//! flag (§5.10.1), never an `argv[0]` verb. The ONLY interactive surface, deliberately
//! out of the data plane so `run` never blocks on a browser. Device flow (RFC 8628,
//! default) and AuthCode + loopback (RFC 8252, `--browser`) both end in one
//! `store.put(provider, &Cred::OAuth2 …)`. The interactive impurities are injected —
//! `BrowserLauncher`, `CodeReceiver`, and the device-poll `Pacer` — so the whole flow
//! is offline-testable; the RNG `verifier`/`state` are supplied by the `bz` bin (auth
//! §7.2). Pure logic lives in [`wire`](super::wire).

use std::io::{self, Write};

use super::flows::{browser_flow, device_flow};
use super::{auth_error, OAuthConfig};
use crate::canonical::{CanonicalError, ErrorKind};
use crate::cli::{Args, Flags};
use crate::config::{config_path, defaults, partial_from_env, read_config_file, ResolvedConfig};

/// Open `url` in the user's browser (auth §7.2). Real impl `Command::spawn`s the
/// `browser_argv`; the fake records the argv and never execs.
pub trait BrowserLauncher {
    fn open(&self, url: &str) -> io::Result<()>;
}

/// Capture the loopback redirect (auth §7.2, §10.1). `bind` binds the listener on
/// `127.0.0.1` at the requested `port` (`None` ⇒ an OS-assigned ephemeral port, RFC
/// 8252 §7.3) and returns the ACTUALLY-bound port, which `browser_flow` substitutes
/// into the `redirect_uri` — single-sourcing the port through the receiver whether
/// fixed or ephemeral. `await_query` then blocks until the redirect arrives and
/// returns its raw `code=…&state=…` query, which `parse_callback` validates.
pub trait CodeReceiver {
    fn bind(&self, port: Option<u16>) -> io::Result<u16>;
    fn await_query(&self) -> io::Result<String>;
}

/// Pace the device-flow poll loop (auth §7.3): the real bin sleeps `secs`; the test
/// fake records the interval and returns instantly — so the whole flow runs offline
/// with no real time. A control-plane concern only, kept off the data-plane `Clock`.
pub trait Pacer {
    fn wait(&self, secs: u64);
}

/// The injected control-plane seams + RNG for one `bz --login` (auth §7.2).
pub struct LoginIo<'a> {
    /// The discovery sink: `bz --login --help`/`--version` self-describe HERE (stdout,
    /// exit 0), the same short-circuit the data plane and `list-models` honor. The
    /// flow's own progress/result lines stay on `stderr` (stdout is for the cred-less
    /// discovery output alone — there is no machine-readable login payload).
    pub stdout: &'a mut dyn Write,
    pub stderr: &'a mut dyn Write,
    pub transport: &'a dyn crate::transport::Transport,
    pub store: &'a dyn crate::store::CredStore,
    pub clock: &'a dyn crate::store::Clock,
    pub browser: &'a dyn BrowserLauncher,
    pub receiver: &'a dyn CodeReceiver,
    pub pacer: &'a dyn Pacer,
    /// PKCE verifier and CSRF state — random tokens minted by the bin (auth §7.2).
    pub verifier: &'a str,
    pub state: &'a str,
}

/// Run `bz --login` and return the POSIX exit code (auth §7). Resolves the provider's
/// `OAuthConfig`, runs the selected flow, and persists the resulting `Cred::OAuth2`.
/// Any failure is written to STDERR and mapped to its exit (login failure → 77,
/// unresolvable / no-oauth / no-device-endpoint provider → 78, bad flag → 64).
pub fn login(args: &Args, io: &mut LoginIo) -> u8 {
    match run_login(args, io) {
        // `Some(provider)` is a completed login; `None` is a `--help`/`--version`
        // short-circuit (already written to stdout) — both exit 0, neither errors.
        Ok(Some(provider)) => {
            let _ = writeln!(io.stderr, "logged in to `{provider}`");
            0
        }
        Ok(None) => 0,
        Err(e) => {
            let _ = writeln!(io.stderr, "{}", e.message);
            e.exit_code()
        }
    }
}

fn run_login(args: &Args, io: &mut LoginIo) -> Result<Option<String>, CanonicalError> {
    let flags = crate::cli::parse_args(&args.argv)?;
    // The SAME discovery short-circuit as the data plane and `--list-models` (§5.5):
    // `bz --login --help`/`--version` print the one shared doc to stdout and exit 0
    // BEFORE resolving a provider — a probe answers even with no provider/config.
    if flags.help {
        crate::run::emit(io.stdout, crate::run::HELP);
        return Ok(None);
    }
    if flags.version {
        crate::run::emit(io.stdout, crate::run::VERSION_LINE);
        return Ok(None);
    }
    let browser = flags.browser;
    // The provider rides the SAME `--provider`/configured-provider fold as a normal run
    // (§5.10.1); none-resolved is the usual 78. The resolved row NAMES the cred (the
    // store key + the success line), the one home for the provider name (auth §7.1).
    let (provider, cfg) = resolve_oauth(flags, args)?;
    let cred = if browser {
        browser_flow(&cfg, io)?
    } else {
        device_flow(&cfg, io)?
    };
    io.store.put(&provider, &cred).map_err(persist_failed)?;
    Ok(Some(provider))
}

/// Resolve the provider from the flag layer and return its NAME + `OAuthConfig` (auth
/// §7.1). The flags route the SAME four-layer fold as a normal run (`--provider`, else
/// a configured provider; neither → `NoProvider`/78); a resolved row with no `oauth`
/// block is a Config error (→78).
fn resolve_oauth(flags: Flags, args: &Args) -> Result<(String, OAuthConfig), CanonicalError> {
    let file = read_config_file(&config_path(flags.config_path, &args.env))?;
    let env = partial_from_env(&args.env).map_err(CanonicalError::from)?;
    let merged = flags.config.or(env).or(file).or(defaults());
    let resolved: ResolvedConfig = merged.into_resolved(None).map_err(CanonicalError::from)?;
    let name = resolved.provider.name.clone();
    let oauth = resolved.provider.oauth.ok_or_else(|| {
        config_err(format!(
            "provider `{name}` has no `oauth` config; add an `oauth` block to its row"
        ))
    })?;
    Ok((name, oauth))
}

/// A config error (→78): no oauth row / no device endpoint.
pub(super) fn config_err(message: String) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Config,
        message,
        provider_detail: None,
    }
}

/// A failure to persist the new credential (→77).
fn persist_failed(e: io::Error) -> CanonicalError {
    auth_error(&format!("could not persist credential: {e}"))
}
