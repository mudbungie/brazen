//! `bz login` — the quarantined control plane (auth §7). The ONLY interactive
//! surface, deliberately out of the data plane so `run` never blocks on a browser.
//! Device flow (RFC 8628, default) and AuthCode + loopback (RFC 8252, `--browser`)
//! both end in one `store.put(provider, &Cred::OAuth2 …)`. The interactive
//! impurities are injected — `BrowserLauncher`, `CodeReceiver`, and the device-poll
//! `Pacer` — so the whole flow is offline-testable; the RNG `verifier`/`state` are
//! supplied by the `bz` bin (auth §7.2). Pure logic lives in [`wire`](super::wire).

use std::io::{self, Write};

use super::flows::{browser_flow, device_flow};
use super::{auth_error, OAuthConfig};
use crate::canonical::{CanonicalError, ErrorKind};
use crate::cli::Args;
use crate::config::{
    config_path, defaults, partial_from_env, read_config_file, PartialConfig, ResolvedConfig,
};

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
/// returns its raw `code=…&state=…` query, which [`parse_callback`] validates.
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

/// The parsed `bz login` argv (auth §7): a provider name and the `--browser` flow
/// selector (else the headless Device flow).
pub struct LoginArgs {
    pub provider: String,
    pub browser: bool,
}

/// The injected control-plane seams + RNG for one `bz login` (auth §7.2).
#[allow(clippy::struct_excessive_bools)]
pub struct LoginIo<'a> {
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

/// Parse `bz login <provider> [--browser]` from the argv AFTER the `login` verb. A
/// second positional or an unknown flag is a usage error (→64).
pub fn parse_login_args(argv: &[String]) -> Result<LoginArgs, CanonicalError> {
    let mut provider = None;
    let mut browser = false;
    for arg in argv {
        if arg == "--browser" {
            browser = true;
        } else if arg.starts_with('-') {
            return Err(usage(format!("unknown `bz login` flag `{arg}`")));
        } else if provider.is_some() {
            return Err(usage("usage: bz login <provider> [--browser]"));
        } else {
            provider = Some(arg.clone());
        }
    }
    let provider = provider.ok_or_else(|| usage("usage: bz login <provider> [--browser]"))?;
    Ok(LoginArgs { provider, browser })
}

/// Run `bz login` and return the POSIX exit code (auth §7). Resolves the provider's
/// `OAuthConfig`, runs the selected flow, and persists the resulting `Cred::OAuth2`.
/// Any failure is written to STDERR and mapped to its exit (login failure → 77,
/// missing device endpoint / no oauth row → 78, bad argv → 64).
pub fn login(args: &Args, io: &mut LoginIo) -> u8 {
    match run_login(args, io) {
        Ok(provider) => {
            let _ = writeln!(io.stderr, "logged in to `{provider}`");
            0
        }
        Err(e) => {
            let _ = writeln!(io.stderr, "{}", e.message);
            e.exit_code()
        }
    }
}

fn run_login(args: &Args, io: &mut LoginIo) -> Result<String, CanonicalError> {
    let la = parse_login_args(args.argv.get(1..).unwrap_or(&[]))?;
    let cfg = resolve_oauth(&la.provider, args)?;
    let cred = if la.browser {
        browser_flow(&cfg, io)?
    } else {
        device_flow(&cfg, io)?
    };
    io.store.put(&la.provider, &cred).map_err(persist_failed)?;
    Ok(la.provider)
}

/// Resolve the provider row by name and return its `OAuthConfig` (auth §7.1). The
/// name routes the SAME four-layer fold as a normal run; a row with no `oauth`
/// block is a Config error (→78).
fn resolve_oauth(provider: &str, args: &Args) -> Result<OAuthConfig, CanonicalError> {
    let selector = PartialConfig {
        provider: Some(provider.to_owned()),
        ..PartialConfig::default()
    };
    let file = read_config_file(&config_path(None, &args.env))?;
    let env = partial_from_env(&args.env).map_err(CanonicalError::from)?;
    let merged = selector.or(env).or(file).or(defaults());
    let resolved: ResolvedConfig = merged.into_resolved(None).map_err(CanonicalError::from)?;
    resolved.provider.oauth.ok_or_else(|| {
        config_err(format!(
            "provider `{provider}` has no `oauth` config; add an `oauth` block to its row"
        ))
    })
}

/// A usage error (→64): a malformed `bz login` invocation.
fn usage(message: impl Into<String>) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Usage,
        message: message.into(),
        provider_detail: None,
    }
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
