//! Shared harness for the `bz login` flow tests (auth §7, §8): build `Args` + a
//! temp config, wire the injected control-plane seams (`BrowserLauncher`,
//! `CodeReceiver`, `Pacer`) and run `brazen::login` offline. A subdir module, so
//! cargo does not compile it as its own test binary.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use brazen::testing::FakeClock;
use brazen::{
    login, Args, BrowserLauncher, CodeReceiver, Cred, CredStore, EnvSnapshot, LoginIo, Pacer,
    Transport,
};

/// A full oauth provider row WITH a device endpoint and a scope (covers the device
/// flow, the browser flow, and `device_params`' scope arm).
pub const FULL: &str = r#"
[[provider]]
name = "claudeauth"
base_url = "https://api.anthropic.com"
protocol = "anthropic_messages"
auth = "oauth2"
api_header = { name = "Authorization", scheme = "bearer" }
oauth = { authorize_url = "https://auth.example/authorize", token_url = "https://auth.example/token", device_url = "https://auth.example/device", client_id = "cid", scope = "read" }
"#;

/// An oauth provider WITH a device endpoint but NO scope — covers the no-scope arm
/// of the device-authorization params (auth §7.3).
pub const DEVICE_NO_SCOPE: &str = r#"
[[provider]]
name = "noscope"
base_url = "https://api.anthropic.com"
protocol = "anthropic_messages"
auth = "oauth2"
api_header = { name = "Authorization", scheme = "bearer" }
oauth = { authorize_url = "https://auth.example/authorize", token_url = "https://auth.example/token", device_url = "https://auth.example/device", client_id = "cid" }
"#;

/// An oauth provider with NO device endpoint and NO scope — `bz login` without
/// `--browser` against it is a Config error (auth §7.1).
pub const NO_DEVICE: &str = r#"
[[provider]]
name = "nodev"
base_url = "https://api.anthropic.com"
protocol = "anthropic_messages"
auth = "oauth2"
api_header = { name = "Authorization", scheme = "bearer" }
oauth = { authorize_url = "https://auth.example/authorize", token_url = "https://auth.example/token", client_id = "cid" }
"#;

/// A plain api-key provider — no `oauth` block, so `bz login` can't resolve one.
pub const NO_OAUTH: &str = r#"
[[provider]]
name = "plain"
base_url = "https://api.example"
protocol = "openai_chat"
auth = "bearer"
api_header = { name = "Authorization", scheme = "bearer" }
"#;

/// A self-deleting temp config file.
pub struct TempFile(pub PathBuf);
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub fn temp(contents: &str) -> TempFile {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("brazen_login_{}_{}.toml", std::process::id(), n));
    fs::write(&path, contents).unwrap();
    TempFile(path)
}

/// All the knobs one login test varies.
pub struct Case<'a> {
    pub argv: &'a [&'a str],
    pub config: &'a str,
    pub tx: &'a dyn Transport,
    pub browser: &'a dyn BrowserLauncher,
    pub receiver: &'a dyn CodeReceiver,
    pub pacer: &'a dyn Pacer,
    pub now: u64,
    pub verifier: &'a str,
    pub state: &'a str,
}

/// Run `brazen::login` against a fresh store and return the exit code, captured
/// stderr, and the store (to assert the persisted cred).
pub fn run(case: Case) -> (u8, String, brazen::testing::MemoryCredStore) {
    let store = brazen::testing::MemoryCredStore::new();
    let (code, stderr) = run_store(&case, &store);
    (code, stderr, store)
}

/// Run `brazen::login` against an arbitrary store (e.g. a failing one).
pub fn run_store(case: &Case, store: &dyn CredStore) -> (u8, String) {
    let cfg = temp(case.config);
    let env = [("BRAZEN_CONFIG", cfg.0.to_str().unwrap_or_default())];
    let args = Args {
        argv: case.argv.iter().map(|s| s.to_string()).collect(),
        env: EnvSnapshot(
            env.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>(),
        ),
    };
    let clock = FakeClock::new(case.now);
    let mut stderr = Vec::new();
    let code = {
        let mut io = LoginIo {
            stderr: &mut stderr,
            transport: case.tx,
            store,
            clock: &clock,
            browser: case.browser,
            receiver: case.receiver,
            pacer: case.pacer,
            verifier: case.verifier,
            state: case.state,
        };
        login(&args, &mut io)
    };
    (code, String::from_utf8_lossy(&stderr).into_owned())
}

/// A `BrowserLauncher` whose `open` always fails — the launch-error path.
pub struct FailBrowser;
impl BrowserLauncher for FailBrowser {
    fn open(&self, _url: &str) -> io::Result<()> {
        Err(io::Error::other("no browser"))
    }
}

/// A `CodeReceiver` whose `await_query` always fails — the loopback-failure path.
pub struct FailReceiver;
impl CodeReceiver for FailReceiver {
    fn port(&self) -> u16 {
        7
    }
    fn await_query(&self) -> io::Result<String> {
        Err(io::Error::other("listener died"))
    }
}

/// A `CredStore` that fails every `put` — the login persist-failure path.
pub struct FailPutStore;
impl CredStore for FailPutStore {
    fn get(&self, _: &str) -> Option<Cred> {
        None
    }
    fn put(&self, _: &str, _: &Cred) -> io::Result<()> {
        Err(io::Error::other("disk full"))
    }
}
