//! The argv grammar around the positional prompt (arch §5.5, §13.7): the first
//! operand ends option parsing (multi-word prompts join through EOF), flags after
//! the prompt are inert text, and the `--` terminator's spellings. Each flag's
//! own parsing lives in `cli_args`. Pure over a `&[String]`.

use crate::{parse_args, OutMode};

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_multi_word_unquoted_prompt_joins_through_eof() {
    // The first operand stops option parsing; everything through EOF is the prompt,
    // operands joined by ONE space (§5.5/§13.7) — so `bz what is 2+2` needs no quotes.
    let f = parse_args(&argv(&["what", "is", "2+2"])).unwrap();
    assert_eq!(f.prompt.as_deref(), Some("what is 2+2"));
    assert!(f.config.output.is_none());
}

#[test]
fn options_before_the_prompt_are_honored_but_after_it_are_inert_text() {
    // Options-before-prompt (POSIX Guideline 9): `--json` BEFORE the prompt selects
    // JSON; the SAME flag AFTER the prompt starts is inert prompt text.
    let before = parse_args(&argv(&["--json", "q"])).unwrap();
    assert_eq!(before.config.output, Some(OutMode::Ndjson));
    assert_eq!(before.prompt.as_deref(), Some("q"));
    let after = parse_args(&argv(&["q", "--json"])).unwrap();
    assert_eq!(after.prompt.as_deref(), Some("q --json"));
    assert!(after.config.output.is_none(), "post-prompt flag is text");
}

#[test]
fn dashes_after_the_prompt_starts_are_inert_text() {
    // Once the prompt begins, no token is an option — a `-`/`--`/word after it is all
    // joined into the prompt verbatim, never parsed (so no unknown-flag error either).
    let f = parse_args(&argv(&["hello", "--nope", "--", "-x"])).unwrap();
    assert_eq!(f.prompt.as_deref(), Some("hello --nope -- -x"));
    assert!(f.config.output.is_none());
}

#[test]
fn double_dash_ends_option_parsing() {
    // After `--`, a `-`-leading word is the prompt, not a flag.
    let f = parse_args(&argv(&["--", "--json"])).unwrap();
    assert_eq!(f.prompt.as_deref(), Some("--json"));
    assert!(f.config.output.is_none());
}

#[test]
fn double_dash_tail_joins_through_eof() {
    // The `--` tail is the prompt through EOF, joined by one space — a leading-dash
    // multi-word prompt is reachable only via the options terminator.
    let f = parse_args(&argv(&["--", "--weird", "and", "more"])).unwrap();
    assert_eq!(f.prompt.as_deref(), Some("--weird and more"));
}

#[test]
fn bare_double_dash_leaves_no_positional() {
    // `bz --` with nothing after it: an empty tail is no prompt at all, so the run
    // falls through to the stdin/bare path (prompt stays None).
    let f = parse_args(&argv(&["--"])).unwrap();
    assert!(f.prompt.is_none());
}

#[test]
fn lone_dash_is_a_positional() {
    let f = parse_args(&argv(&["-"])).unwrap();
    assert_eq!(f.prompt.as_deref(), Some("-"));
}
