//! The call envelope, black-box (arch §5.10.5): the real `bz --envelope` binary, fed
//! one JSON object on stdin, against the same simulated provider as the direct run.
//!
//! For each command line a far caller may send, the enveloped run's stdout, stderr and
//! exit code are BYTE-IDENTICAL to `bz <args>` fed the same stdin — the model call
//! included, and that one is also shown to stream (its first event leaves `bz` while
//! the provider's response is still in flight). A flag outside the set is refused (64)
//! before the configuration is read. Configuration arrives by ENVIRONMENT here, as it
//! does on a real executing box: `--config` is exactly a flag an envelope may not carry.

#[allow(dead_code)]
#[path = "live_support/exec.rs"]
mod exec;
#[allow(dead_code)]
mod sim_support;

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};

use sim_support::{config_for, fixture, serve_gated, FakeProvider, Sim, PROVIDERS};

/// A canonical request for the keyless sim row (the fake replays its fixture whatever
/// is asked).
const REQUEST: &str = r#"{"model":"llama-sim","messages":[{"role":"user","content":[{"type":"text","text":"say hi"}]}]}"#;

fn ollama() -> &'static Sim {
    let p = &PROVIDERS[4];
    assert_eq!(p.name, "ollama");
    p
}

fn envelope(args: &[&str], stdin: &str) -> String {
    serde_json::json!({ "args": args, "stdin": stdin }).to_string()
}

/// One `bz` run under a box-local environment: `config` as `$BRAZEN_CONFIG`, and
/// fresh credential/cache roots so no run sees another's (or the developer's) state.
fn bz(argv: &[&str], stdin: &str, config: &str) -> (i32, String, String) {
    let home = tempfile::tempdir().expect("temp home");
    let root = home.path().to_string_lossy().into_owned();
    let argv: Vec<String> = argv.iter().map(|s| (*s).to_owned()).collect();
    let env = [
        ("BRAZEN_CONFIG", config),
        ("HOME", root.as_str()),
        ("XDG_DATA_HOME", root.as_str()),
        ("XDG_CACHE_HOME", root.as_str()),
        ("XDG_CONFIG_HOME", root.as_str()),
    ];
    exec::run_bz_in(&argv, stdin, &env)
}

/// Run `args` both ways and require the three streams' bytes and the exit to match.
fn same_both_ways(args: &[&str], stdin: &str, config: &str) -> (i32, String, String) {
    let direct = bz(args, stdin, config);
    let enveloped = bz(&["--envelope"], &envelope(args, stdin), config);
    assert_eq!(
        enveloped,
        direct,
        "`bz {}` differs enveloped",
        args.join(" ")
    );
    direct
}

#[test]
fn a_model_call_is_byte_identical_enveloped() {
    let p = ollama();
    let server = FakeProvider::serve(p.content_type, fixture(p.fixture));
    let cfg = config_for(p, &server.base_url());
    let cfg = cfg.path().to_string_lossy();
    let (code, out, err) = same_both_ways(&["--json", "--provider", "ollama"], REQUEST, &cfg);
    assert_eq!(code, 0, "stderr: {err}\nstdout: {out}");
    assert!(out.ends_with("{\"type\":\"end\"}\n"), "{out}");
}

#[test]
fn the_listings_and_the_sign_in_are_byte_identical_enveloped() {
    let p = ollama();
    let tags = br#"{"models":[{"name":"llama-sim"}]}"#.to_vec();
    let server = FakeProvider::serve("application/json", tags);
    let cfg = config_for(p, &server.base_url());
    let cfg = cfg.path().to_string_lossy();

    let (code, out, err) = same_both_ways(&["--list-providers", "--json"], "", &cfg);
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains("ollama"), "{out}");

    let (code, out, err) = same_both_ways(&["--login", "--json"], "", &cfg);
    assert_eq!(code, 0, "stderr: {err}");
    assert!(!out.is_empty());

    let listing = ["--list-models", "--provider", "ollama", "--json"];
    let (code, out, err) = same_both_ways(&listing, "", &cfg);
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains("llama-sim"), "{out}");

    // The sign-in itself, both flows: this row has no OAuth block, so each is refused —
    // identically, and without a browser or a network call to make the test impure.
    for login in [
        &["--login", "--provider", "ollama"][..],
        &["--login", "--provider", "ollama", "--browser"],
    ] {
        let (code, out, err) = same_both_ways(login, "", &cfg);
        assert_eq!(code, 78, "stderr: {err}");
        assert!(out.is_empty());
        assert!(err.contains("no `oauth` config"), "{err}");
    }
}

#[test]
fn an_enveloped_model_call_streams_before_the_response_ends() {
    let p = ollama();
    let body = fixture(p.fixture);
    let cut = body.iter().position(|b| *b == b'\n').expect("a first line") + 1;
    let (server, gate, verdict) =
        serve_gated(p.content_type, body[..cut].to_vec(), body[cut..].to_vec());
    let cfg = config_for(p, &server.base_url());
    let home = tempfile::tempdir().expect("temp home");

    let mut child = Command::new(env!("CARGO_BIN_EXE_bz"))
        .arg("--envelope")
        .env("BRAZEN_CONFIG", cfg.path())
        .env("HOME", home.path())
        .env("XDG_DATA_HOME", home.path())
        .env("XDG_CACHE_HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn `bz`");
    let request = envelope(&["--json", "--provider", "ollama"], REQUEST);
    child
        .stdin
        .take()
        .expect("bz stdin")
        .write_all(request.as_bytes())
        .expect("write envelope");

    // The provider has sent ONE line and is holding the rest. An event line read here
    // can only have been written mid-response; only then is the rest released.
    let mut stdout = BufReader::new(child.stdout.take().expect("bz stdout"));
    let mut first = String::new();
    stdout.read_line(&mut first).expect("read the first event");
    assert!(first.contains("\"type\":\"message_start\""), "{first}");
    gate.send(()).expect("release the rest");

    let mut rest = String::new();
    stdout.read_to_string(&mut rest).expect("read the rest");
    assert!(rest.ends_with("{\"type\":\"end\"}\n"), "{rest}");
    assert!(child.wait().expect("wait for `bz`").success());
    assert!(
        verdict.recv().expect("server verdict"),
        "the first event arrived only after the provider gave up waiting: output was buffered"
    );
}

#[test]
fn a_refused_flag_exits_64_before_the_configuration_is_read() {
    // A config that cannot be parsed: any run that reaches the config layer fails 78.
    let mut broken = tempfile::NamedTempFile::new().expect("temp config");
    broken
        .write_all(b"this is not = [toml")
        .expect("write config");
    let broken = broken.path().to_string_lossy().into_owned();

    let (code, _, _) = bz(&["--list-providers", "--json"], "", &broken);
    assert_eq!(code, 78, "the control: this config is fatal once read");

    for refused in [
        &["--dump-config"][..],
        &["--config", "elsewhere.toml", "--list-providers"],
        &["--json", "--provider", "ollama", "--file", "notes.txt"],
        &["--json", "--api-key=sk-sim-dummy"],
        &["--serve"],
        &["say hi"],
    ] {
        let (code, out, err) = bz(&["--envelope"], &envelope(refused, REQUEST), &broken);
        assert_eq!(code, 64, "{refused:?}: {err}");
        assert!(out.is_empty(), "{refused:?} wrote stdout: {out}");
        assert!(err.contains("may not be sent in an envelope"), "{err}");
    }

    let (code, out, err) = bz(&["--envelope"], "{\"args\": \"--json\"}", &broken);
    assert_eq!((code, out.as_str()), (64, ""));
    assert!(err.starts_with("malformed envelope"), "{err}");
}
