//! `Style::resolve` matrix (interactive-output spec §3): the activation predicate
//! is a pure function of `(stdout_tty, OutMode, env)`, so the whole tty × mode ×
//! NO_COLOR × TERM × locale space is a table test with zero IO. Pretty is ON iff
//! stdout is a tty AND mode is text AND NO_COLOR is unset AND TERM is set ≠ dumb;
//! its `ascii` flag is the inverse of a UTF-8 locale.

use std::collections::BTreeMap;

use brazen::{EnvSnapshot, Glyph, OutMode, Sgr, Style};

/// Build an `EnvSnapshot` from literal `(key, value)` pairs.
fn env(pairs: &[(&str, &str)]) -> EnvSnapshot {
    EnvSnapshot(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<BTreeMap<_, _>>(),
    )
}

/// A sane interactive baseline: a real TERM and a UTF-8 locale.
fn xterm_utf8() -> Vec<(&'static str, &'static str)> {
    vec![("TERM", "xterm-256color"), ("LANG", "en_US.UTF-8")]
}

#[test]
fn pretty_on_when_tty_text_no_nocolor_real_term() {
    assert_eq!(
        Style::resolve(true, OutMode::Text, &env(&xterm_utf8())),
        Style::Pretty { ascii: false }
    );
}

#[test]
fn plain_when_stdout_is_not_a_tty() {
    // The pipe/redirect path — the building-block contract. Pretty never activates.
    assert_eq!(
        Style::resolve(false, OutMode::Text, &env(&xterm_utf8())),
        Style::Plain
    );
}

#[test]
fn plain_for_json_and_raw_even_on_a_tty() {
    // The machine contracts are never prettified — they don't even construct this
    // sink, but the predicate also refuses them defensively.
    for mode in [OutMode::Ndjson, OutMode::Raw] {
        assert_eq!(
            Style::resolve(true, mode, &env(&xterm_utf8())),
            Style::Plain
        );
    }
}

#[test]
fn no_color_set_to_any_value_forces_plain() {
    // The de-facto convention: presence, not truthiness. Empty and "0" both disable.
    for val in ["1", "", "0", "false"] {
        let mut pairs = xterm_utf8();
        pairs.push(("NO_COLOR", val));
        assert_eq!(
            Style::resolve(true, OutMode::Text, &env(&pairs)),
            Style::Plain,
            "NO_COLOR={val:?} must force plain"
        );
    }
}

#[test]
fn term_dumb_or_unset_forces_plain() {
    // TERM=dumb is the canonical "no capabilities" terminal; an absent TERM is the
    // same — no real terminal type to trust.
    assert_eq!(
        Style::resolve(
            true,
            OutMode::Text,
            &env(&[("TERM", "dumb"), ("LANG", "en_US.UTF-8")])
        ),
        Style::Plain
    );
    assert_eq!(
        Style::resolve(true, OutMode::Text, &env(&[("LANG", "en_US.UTF-8")])),
        Style::Plain
    );
}

#[test]
fn non_utf8_locale_degrades_glyphs_to_ascii() {
    // Pretty is still ON (a tty with color), but glyphs fall back to ASCII.
    for pairs in [
        vec![("TERM", "xterm")],                // no locale at all
        vec![("TERM", "xterm"), ("LANG", "C")], // the POSIX C locale
        vec![("TERM", "xterm"), ("LANG", "en_US.ISO-8859-1")],
    ] {
        assert_eq!(
            Style::resolve(true, OutMode::Text, &env(&pairs)),
            Style::Pretty { ascii: true },
            "{pairs:?} should be ASCII-degraded pretty"
        );
    }
}

#[test]
fn lc_all_outranks_lc_ctype_outranks_lang() {
    // LC_ALL wins: a UTF-8 LC_ALL keeps glyphs even with a C LANG.
    assert_eq!(
        Style::resolve(
            true,
            OutMode::Text,
            &env(&[("TERM", "xterm"), ("LC_ALL", "en_US.UTF-8"), ("LANG", "C")])
        ),
        Style::Pretty { ascii: false }
    );
    // LC_CTYPE wins over LANG when LC_ALL is absent.
    assert_eq!(
        Style::resolve(
            true,
            OutMode::Text,
            &env(&[
                ("TERM", "xterm"),
                ("LC_CTYPE", "C"),
                ("LANG", "en_US.UTF-8")
            ])
        ),
        Style::Pretty { ascii: true }
    );
}

#[test]
fn is_pretty_reflects_the_variant() {
    assert!(Style::Pretty { ascii: false }.is_pretty());
    assert!(Style::Pretty { ascii: true }.is_pretty());
    assert!(!Style::Plain.is_pretty());
}

#[test]
fn plain_paints_and_gutters_nothing() {
    // `paint`/`glyph` are total over any `Style`: under `Plain` the text passes through
    // unstyled (no SGR even were it called) and the glyph falls back to ASCII.
    assert_eq!(Style::Plain.paint(Sgr::Bold, "x"), "x");
    assert_eq!(Style::Plain.glyph(Glyph::Tool), "*");
}

#[test]
fn pretty_paints_each_sgr_role_closed_by_reset() {
    let s = Style::Pretty { ascii: false };
    assert_eq!(s.paint(Sgr::Dim, "a"), "\x1b[2ma\x1b[0m");
    assert_eq!(s.paint(Sgr::Bold, "a"), "\x1b[1ma\x1b[0m");
    assert_eq!(s.paint(Sgr::Yellow, "a"), "\x1b[33ma\x1b[0m");
    assert_eq!(s.paint(Sgr::Green, "a"), "\x1b[32ma\x1b[0m");
    assert_eq!(s.paint(Sgr::Red, "a"), "\x1b[31ma\x1b[0m");
}
