//! The call envelope (arch §5.10.5): `bz --envelope` takes its command line as one
//! JSON object on stdin and hands back an ordinary argv + stdin — but only for the
//! closed flag set a far caller may send. Every other flag is refused (64) naming the
//! flag and never its value; a malformed envelope is a usage error; a command line
//! that is not an envelope passes through with its stdin UNREAD.

use std::collections::BTreeMap;
use std::io::{self, Cursor, Read};

use crate::{unwrap_envelope, Args, EnvSnapshot};

/// A reader that panics when touched — proof a path never reads stdin.
struct Untouched;
impl Read for Untouched {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        panic!("stdin must not be read");
    }
}

/// A reader that fails — the unreadable-envelope path.
struct Broken;
impl Read for Broken {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("boom"))
    }
}

fn args(argv: &[&str]) -> Args {
    Args {
        argv: argv.iter().map(|s| (*s).to_owned()).collect(),
        env: EnvSnapshot(BTreeMap::from([(
            "BRAZEN_MODEL".to_owned(),
            "m".to_owned(),
        )])),
        tty: true,
        stdout_tty: true,
    }
}

/// Open `body` as the envelope on stdin: the inner (argv, stdin), or (exit, stderr).
fn open(body: &str) -> Result<(Vec<String>, String), (u8, String)> {
    let mut err = Vec::new();
    match unwrap_envelope(args(&["--envelope"]), &mut Cursor::new(body), &mut err) {
        Ok((inner, sent)) => {
            assert!(err.is_empty(), "an accepted envelope is silent");
            // The environment is the box's own and is carried untouched; the inner
            // stdin is the envelope's string, so it is never a tty; stdout is as probed.
            assert_eq!(inner.env.get("BRAZEN_MODEL"), Some("m"));
            assert!(!inner.tty);
            assert!(inner.stdout_tty);
            let sent = String::from_utf8(sent.expect("envelope mode yields a stdin")).unwrap();
            Ok((inner.argv, sent))
        }
        Err(code) => Err((code, String::from_utf8(err).unwrap())),
    }
}

fn envelope(words: &[&str], stdin: Option<&str>) -> String {
    let mut doc = serde_json::json!({ "args": words });
    if let Some(s) = stdin {
        doc["stdin"] = s.into();
    }
    doc.to_string()
}

#[test]
fn a_command_line_that_is_not_an_envelope_passes_through_unread() {
    for argv in [
        &[][..],
        &["--json", "--provider", "r"],
        // Only a LEADING `--envelope` is the mode: after `--` it is prompt text, and
        // behind another flag it is the grammar's ordinary unknown flag (64).
        &["--", "--envelope"],
        &["--json", "--envelope"],
    ] {
        let mut err = Vec::new();
        let (out, sent) = unwrap_envelope(args(argv), &mut Untouched, &mut err).unwrap();
        assert_eq!(out.argv, argv);
        assert!(out.tty, "the tty fact is untouched outside the mode");
        assert!(sent.is_none());
        assert!(err.is_empty());
    }
    let unknown = crate::parse_args(&["--json".to_owned(), "--envelope".to_owned()]).unwrap_err();
    assert_eq!(unknown.exit_code(), 64);
}

#[test]
fn each_allowed_command_line_opens_to_exactly_itself() {
    let request = "{\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}\n";
    for (words, stdin) in [
        (&["--json", "--provider", "r"][..], Some(request)),
        (&["--list-providers", "--json"], None),
        (&["--login", "--json"], None),
        (&["--list-models", "--provider", "r", "--json"], None),
        (&["--login", "--provider", "r"], None),
        (&["--login", "--provider=r", "--browser"], None),
        // `--provider` consumes its value, whatever it looks like — as `parse_args` does.
        (&["--provider", "--config", "--json"], None),
        (&["--provider"], None),
        (&[], Some("")),
    ] {
        let (argv, sent) = open(&envelope(words, stdin)).expect("allowed");
        assert_eq!(argv, words);
        assert_eq!(sent, stdin.unwrap_or(""), "bytes in are the bytes piped");
    }
}

#[test]
fn every_flag_outside_the_set_is_refused_by_name_and_never_by_value() {
    // Every other flag `parse_args` knows, in both value spellings where it takes one.
    for (words, named) in [
        (&["--config", "SECRETVALUE"][..], "flag `--config`"),
        (&["--config=SECRETVALUE"], "flag `--config`"),
        (&["--dump-config"], "flag `--dump-config`"),
        (&["--input", "SECRETVALUE"], "flag `--input`"),
        (&["--file", "SECRETVALUE"], "flag `--file`"),
        (&["-f", "SECRETVALUE"], "flag `-f`"),
        (&["--api-key=SECRETVALUE"], "flag `--api-key`"),
        (&["--base-url", "SECRETVALUE"], "flag `--base-url`"),
        (&["--serve"], "flag `--serve`"),
        (&["--in", "openai_chat"], "flag `--in`"),
        (&["--raw"], "flag `--raw`"),
        (&["--raw=in"], "flag `--raw`"),
        (&["--count-tokens"], "flag `--count-tokens`"),
        (&["--text"], "flag `--text`"),
        (&["--thinking"], "flag `--thinking`"),
        (&["--stream"], "flag `--stream`"),
        (&["--no-stream"], "flag `--no-stream`"),
        (&["--image"], "flag `--image`"),
        (&["--usage-fold-thinking"], "flag `--usage-fold-thinking`"),
        (&["--model", "m"], "flag `--model`"),
        (&["-m", "m"], "flag `-m`"),
        (&["--system", "SECRETVALUE"], "flag `--system`"),
        (&["--max-tokens", "1"], "flag `--max-tokens`"),
        (&["--temperature", "1"], "flag `--temperature`"),
        (&["--top-p", "1"], "flag `--top-p`"),
        (&["--reasoning", "low"], "flag `--reasoning`"),
        (&["--tier", "priority"], "flag `--tier`"),
        (&["--timeout", "1"], "flag `--timeout`"),
        (&["--skill"], "flag `--skill`"),
        (&["--help"], "flag `--help`"),
        (&["-h"], "flag `-h`"),
        (&["--version"], "flag `--version`"),
        (&["-V"], "flag `-V`"),
        (&["--envelope"], "flag `--envelope`"),
        (&["--no-such-flag"], "flag `--no-such-flag`"),
        (&["--"], "flag `--`"),
        // A refused flag hiding behind allowed ones, and a prompt in any position.
        (
            &["--json", "--provider", "r", "--config", "SECRETVALUE"],
            "flag `--config`",
        ),
        (&["--json", "--", "SECRETVALUE"], "flag `--`"),
        (&["SECRETVALUE"], "a positional prompt"),
        (&["--json", "SECRETVALUE"], "a positional prompt"),
        (&["-"], "flag `-`"),
    ] {
        let (code, err) = open(&envelope(words, Some("SECRETVALUE"))).expect_err("refused");
        assert_eq!(code, 64, "{words:?}");
        assert!(err.starts_with(named), "{words:?} → {err}");
        assert!(err.contains("may not be sent in an envelope (allowed: --json, --provider, --list-providers, --list-models, --login, --browser)"), "{err}");
        assert!(
            !err.contains("SECRETVALUE"),
            "a refusal never echoes a value: {err}"
        );
        assert!(err.ends_with('\n'));
    }
}

#[test]
fn a_malformed_envelope_is_a_usage_error_that_echoes_nothing() {
    for body in [
        "",
        "not json SECRETVALUE",
        "[\"--json\"]",
        "{}",
        "{\"stdin\":\"SECRETVALUE\"}",
        "{\"args\":\"--json\"}",
        "{\"args\":[1]}",
        "{\"args\":[],\"stdin\":7}",
        "{\"args\":[],\"SECRETVALUE\":1}",
        "{\"args\":[]} SECRETVALUE",
    ] {
        let (code, err) = open(body).expect_err("malformed");
        assert_eq!(code, 64, "{body}");
        assert!(
            err.starts_with("malformed envelope at line "),
            "{body} → {err}"
        );
        assert!(!err.contains("SECRETVALUE"), "{err}");
    }
}

#[test]
fn the_flag_stands_alone_and_an_unreadable_stdin_is_a_usage_error() {
    // Anything beside the flag is refused BEFORE stdin is read.
    let mut err = Vec::new();
    let beside = args(&["--envelope", "--config", "x"]);
    assert_eq!(
        unwrap_envelope(beside, &mut Untouched, &mut err).err(),
        Some(64)
    );
    let err = String::from_utf8(err).unwrap();
    assert!(
        err.starts_with("`--envelope` takes no other argument"),
        "{err}"
    );

    let mut err = Vec::new();
    let alone = args(&["--envelope"]);
    assert_eq!(
        unwrap_envelope(alone, &mut Broken, &mut err).err(),
        Some(64)
    );
    let err = String::from_utf8(err).unwrap();
    assert_eq!(err, "cannot read the envelope from stdin: boom\n");
}
