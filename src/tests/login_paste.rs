//! `bz --login` — the pasted-code sign-in (auth §7.7): the headless login of a row
//! with no device endpoint, as TWO command lines. The first prints the authorize URL
//! on stdout and parks the verifier + state; the second redeems what the human
//! copied. Nothing listens, no browser is launched, and until the second line
//! succeeds no request is sent and the row's own credential is untouched.

use crate::testing::{
    FakeBrowserLauncher, FakeCodeReceiver, FakePacer, MemoryCredStore, MockTransport,
};
use crate::tests::login_support::{run_store_io, Case, FailPutStore, NO_DEVICE, PASTE, REDIRECT};
use crate::{pending_key, Cred, CredStore, Secret};

const TOKEN: &[u8] =
    br#"{"access_token":"at-paste-example","refresh_token":"rt","expires_in":3600}"#;

/// One command line against `store`: (exit, stdout, stderr, token-endpoint bodies).
fn line(
    argv: &[&str],
    config: &str,
    store: &dyn CredStore,
    now: u64,
    secrets: (&str, &str),
) -> (u8, String, String, Vec<String>) {
    let tx = MockTransport::ok(vec![TOKEN]);
    let browser = FakeBrowserLauncher::new();
    let receiver = FakeCodeReceiver::new(1, "");
    let pacer = FakePacer::new();
    let case = Case {
        argv,
        config,
        tx: &tx,
        browser: &browser,
        receiver: &receiver,
        pacer: &pacer,
        now,
        verifier: secrets.0,
        state: secrets.1,
    };
    let (code, stdout, stderr) = run_store_io(&case, store);
    assert!(browser.opened().is_empty(), "no browser is ever launched");
    let sent = tx
        .requests()
        .iter()
        .map(|r| String::from_utf8_lossy(&r.body).into_owned())
        .collect();
    (code, stdout, stderr, sent)
}

/// The first command line for `row`, with the secrets every test parks.
fn begin(row: &str, config: &str, store: &dyn CredStore) -> (u8, String, String) {
    let (code, stdout, stderr, sent) = line(
        &["--login", "--provider", row],
        config,
        store,
        100,
        ("the-verifier", "STATE123"),
    );
    assert!(sent.is_empty(), "printing the URL sends nothing");
    (code, stdout, stderr)
}

/// The second command line at `now`, minted with DIFFERENT secrets — so a pass
/// proves the parked verifier and state are the ones used.
fn finish(
    row: &str,
    config: &str,
    store: &dyn CredStore,
    pasted: &str,
    now: u64,
) -> (u8, String, Vec<String>) {
    let (code, stdout, stderr, sent) = line(
        &["--login", "--provider", row, "--code", pasted],
        config,
        store,
        now,
        ("another-verifier", "ANOTHER"),
    );
    assert!(stdout.is_empty());
    (code, stderr, sent)
}

#[test]
fn the_first_line_prints_the_url_and_parks_the_verifier_beside_the_credential() {
    let held = Cred::OAuth2 {
        access_token: Secret::new("still-here"),
        refresh_token: Secret::new("rt"),
        expires_at: 9_999,
        scope: None,
        account_id: None,
    };
    let store = MemoryCredStore::with("nodev", held.clone());
    let (code, stdout, stderr) = begin("nodev", NO_DEVICE, &store);

    assert_eq!(code, 0);
    // stdout is the URL and nothing else — PKCE S256, the row's loopback redirect
    // string with no port (the row fixes none), the minted state.
    assert!(stdout.starts_with("https://auth.example/authorize?response_type=code&client_id=cid"));
    assert!(stdout.ends_with('\n'));
    assert_eq!(stdout.lines().count(), 1);
    assert!(stdout.contains("redirect_uri=http%3A%2F%2F127.0.0.1%2Fcallback&"));
    assert!(stdout.contains("state=STATE123"));
    assert!(stdout.contains("code_challenge_method=S256"));
    assert!(!stdout.contains("the-verifier"));
    // stderr tells the human what to copy and how to finish; nobody is logged in.
    assert!(stderr.contains("address bar"));
    assert!(stderr.contains("bz --login --provider nodev --code"));
    assert!(stderr.contains("within 10 minutes"));
    assert!(!stderr.contains("logged in"));
    // The credential the row already held is untouched; the verifier is parked
    // under the pending key with an ABSOLUTE lapse.
    assert_eq!(store.get("nodev"), Some(held));
    assert_eq!(
        store.get(&pending_key("nodev")),
        Some(Cred::Pending {
            verifier: Secret::new("the-verifier"),
            state: "STATE123".to_owned(),
            expires_at: 700,
        })
    );
}

#[test]
fn the_redirect_the_url_names_is_row_data() {
    // A code-display redirect, named by the row: the page shows the code.
    let store = MemoryCredStore::new();
    let (code, stdout, stderr) = begin("pasterow", PASTE, &store);
    assert_eq!(code, 0);
    assert!(stdout.contains("redirect_uri=https%3A%2F%2Fauth.example%2Fcode%2Fcallback&"));
    assert!(stderr.contains("copy the code the page then shows"));
    assert!(!stderr.contains("address bar"));
    // A fixed loopback redirect, byte-exact with its port.
    let (code, stdout, stderr) = begin("openaichat", REDIRECT, &store);
    assert_eq!(code, 0);
    assert!(stdout.contains("redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback&"));
    assert!(stdout.contains("codex_cli_simplified_flow=true"));
    assert!(stderr.contains("`http://localhost:1455/auth/callback`"));
}

#[test]
fn a_store_that_cannot_park_fails_the_first_line_before_printing() {
    let (code, stdout, stderr) = begin("nodev", NO_DEVICE, &FailPutStore);
    assert_eq!(code, 77);
    assert!(stdout.is_empty());
    assert!(stderr.contains("could not park the sign-in: disk full"));
}

#[test]
fn the_second_line_redeems_the_code_against_the_parked_verifier() {
    let store = MemoryCredStore::new();
    begin("pasterow", PASTE, &store);
    let (code, stderr, sent) = finish("pasterow", PASTE, &store, " CODE1#STATE123\n", 699);

    assert_eq!(code, 0, "{stderr}");
    assert!(stderr.contains("logged in to `pasterow`"));
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("grant_type=authorization_code"));
    assert!(sent[0].contains("code=CODE1&"));
    assert!(sent[0].contains("redirect_uri=https%3A%2F%2Fauth.example%2Fcode%2Fcallback"));
    assert!(sent[0].contains("code_verifier=the-verifier"));
    match store.get("pasterow") {
        Some(Cred::OAuth2 {
            access_token,
            expires_at,
            ..
        }) => {
            assert_eq!(access_token.expose(), "at-paste-example");
            assert_eq!(expires_at, 699 + 3600);
        }
        other => panic!("expected a stored OAuth2 credential, got {other:?}"),
    }
}

#[test]
fn a_bare_code_and_a_pasted_address_are_read_too() {
    let store = MemoryCredStore::new();
    begin("nodev", NO_DEVICE, &store);
    let (code, _, sent) = finish("nodev", NO_DEVICE, &store, "BARECODE", 101);
    assert_eq!(code, 0);
    assert!(sent[0].contains("code=BARECODE&"));
    assert!(sent[0].contains("redirect_uri=http%3A%2F%2F127.0.0.1%2Fcallback"));
    // The whole address the browser was sent to, fragment and all — and its bare
    // query. Still inside the window, so the same parked verifier serves again.
    for pasted in [
        "http://127.0.0.1/callback?code=FROMURL&state=STATE123#frag",
        "state=STATE123&code=FROMURL",
    ] {
        let (code, _, sent) = finish("nodev", NO_DEVICE, &store, pasted, 102);
        assert_eq!(code, 0, "{pasted}");
        assert!(sent[0].contains("code=FROMURL&"), "{pasted}");
    }
}

#[test]
fn a_paste_that_is_not_this_sign_ins_is_refused_before_any_exchange() {
    let store = MemoryCredStore::new();
    begin("nodev", NO_DEVICE, &store);
    for (pasted, reason) in [
        ("CODE#WRONGSTATE", "belongs to a different sign-in"),
        (
            "http://127.0.0.1/callback?code=C&state=WRONG",
            "`state` mismatch",
        ),
        (
            "http://127.0.0.1/callback?error=access_denied&state=STATE123",
            "authorization denied: access_denied",
        ),
        ("", "no code was pasted"),
        ("#STATE123", "no code was pasted"),
    ] {
        let (code, stderr, sent) = finish("nodev", NO_DEVICE, &store, pasted, 101);
        assert_eq!(code, 77, "{pasted}");
        assert!(stderr.contains(reason), "{pasted} → {stderr}");
        assert!(sent.is_empty(), "{pasted}");
        assert!(store.get("nodev").is_none());
    }
}

#[test]
fn nothing_parked_and_a_lapsed_window_both_name_the_restart() {
    let store = MemoryCredStore::new();
    let (code, stderr, sent) = finish("nodev", NO_DEVICE, &store, "CODE", 101);
    assert_eq!(code, 77);
    assert!(stderr.contains("no sign-in is waiting for a code for `nodev`"));
    assert!(stderr.contains("start again with `bz --login --provider nodev`"));
    assert!(sent.is_empty());

    begin("nodev", NO_DEVICE, &store);
    // The lapse is the instant itself (`>=`): one second earlier still redeems.
    let (code, stderr, sent) = finish("nodev", NO_DEVICE, &store, "CODE", 700);
    assert_eq!(code, 77);
    assert!(stderr.contains("lapsed (10 minutes)"));
    assert!(stderr.contains("start again with `bz --login --provider nodev`"));
    assert!(sent.is_empty());
}

#[test]
fn starting_again_leaves_only_the_newest_url_finishable() {
    let store = MemoryCredStore::new();
    begin("pasterow", PASTE, &store);
    // A second start — another seat, or the same one again — re-parks.
    let (code, ..) = line(
        &["--login", "--provider", "pasterow"],
        PASTE,
        &store,
        200,
        ("second-verifier", "SECOND"),
    );
    assert_eq!(code, 0);
    let (code, stderr, sent) = finish("pasterow", PASTE, &store, "OLD#STATE123", 201);
    assert_eq!(code, 77);
    assert!(stderr.contains("belongs to a different sign-in"));
    assert!(sent.is_empty());
    let (code, _, sent) = finish("pasterow", PASTE, &store, "NEW#SECOND", 201);
    assert_eq!(code, 0);
    assert!(sent[0].contains("code_verifier=second-verifier"));
}

#[test]
fn a_code_beside_browser_is_a_usage_error() {
    let store = MemoryCredStore::new();
    let (code, _, stderr, sent) = line(
        &["--login", "--provider", "nodev", "--code=C", "--browser"],
        NO_DEVICE,
        &store,
        100,
        ("v", "s"),
    );
    assert_eq!(code, 64);
    assert!(stderr.contains("--code finishes a pasted-code sign-in"));
    assert!(sent.is_empty());
}
