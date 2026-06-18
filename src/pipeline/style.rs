//! The pretty-text `Style` capability (interactive-output spec §2–§3, §5): a pure
//! function of `(stdout_tty, &EnvSnapshot)` that decides the interactive skin and
//! owns every SGR escape and glyph. The lib stays tty-blind — the shim feeds the
//! one `stdout_tty` bool (sibling of the `Args.tty` stdin probe, §5.5) and ALL
//! policy lives here, table-tested to 100% with zero IO.

use crate::config::partial::OutMode;
use crate::config::EnvSnapshot;

/// The resolved output skin. `Plain` is the byte-for-byte current `TextSink`;
/// `Pretty` carries whether glyphs degrade to ASCII (a non-UTF-8 locale). One
/// predicate, total fallback, no half-states (spec §3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Plain,
    Pretty { ascii: bool },
}

/// The four SGR roles the skin uses (spec §5): only widely-safe codes — dim, bold,
/// and the three foreground colors, each closed by `\x1b[0m`.
#[derive(Clone, Copy)]
pub enum Sgr {
    Dim,
    Bold,
    Yellow,
    Green,
    Red,
}

impl Sgr {
    /// The opening escape for this role; `\x1b[0m` always closes.
    fn open(self) -> &'static str {
        match self {
            Sgr::Dim => "\x1b[2m",
            Sgr::Bold => "\x1b[1m",
            Sgr::Yellow => "\x1b[33m",
            Sgr::Green => "\x1b[32m",
            Sgr::Red => "\x1b[31m",
        }
    }
}

/// The three sigil-gutter glyphs (spec §5): tool, footer, error. Each resolves to a
/// UTF-8 glyph or its ASCII fallback under [`Style::Pretty { ascii: true }`].
#[derive(Clone, Copy)]
pub enum Glyph {
    Tool,
    Footer,
    Error,
}

impl Glyph {
    /// `(utf8, ascii)` for this gutter glyph.
    fn pair(self) -> (&'static str, &'static str) {
        match self {
            Glyph::Tool => ("⚙", "*"),
            Glyph::Footer => ("✓", "+"),
            Glyph::Error => ("✗", "x"),
        }
    }
}

impl Style {
    /// Resolve the skin from the stdout-isatty bool and the env (spec §3). Pretty is
    /// ON iff stdout is a tty, the mode is the default text projection, `NO_COLOR` is
    /// unset (any value ⇒ off, the de-facto convention), and `TERM` is set and not
    /// `"dumb"`. Otherwise `Plain` — the literal current `TextSink`. When pretty,
    /// glyphs degrade to ASCII unless the locale names UTF-8.
    pub fn resolve(stdout_tty: bool, output: OutMode, env: &EnvSnapshot) -> Style {
        let term_ok = matches!(env.get("TERM"), Some(t) if t != "dumb");
        let pretty =
            stdout_tty && output == OutMode::Text && env.get("NO_COLOR").is_none() && term_ok;
        if pretty {
            Style::Pretty {
                ascii: !utf8_locale(env),
            }
        } else {
            Style::Plain
        }
    }

    /// Does this skin paint chrome at all? `Plain` writes none (the pipe/script path).
    pub fn is_pretty(self) -> bool {
        matches!(self, Style::Pretty { .. })
    }

    /// Wrap `text` in an SGR role, closed by reset — or return it unchanged under
    /// `Plain` (the answer is never styled even on a tty, spec §4).
    pub fn paint(self, sgr: Sgr, text: &str) -> String {
        match self {
            Style::Plain => text.to_owned(),
            Style::Pretty { .. } => format!("{}{text}\x1b[0m", sgr.open()),
        }
    }

    /// The gutter glyph for this skin: UTF-8, or its ASCII fallback under a non-UTF-8
    /// locale. Only called on the pretty path, so `Plain` would never gutter — it
    /// returns the ASCII form defensively rather than branching the caller.
    pub fn glyph(self, g: Glyph) -> &'static str {
        let (utf8, ascii) = g.pair();
        match self {
            Style::Pretty { ascii: false } => utf8,
            _ => ascii,
        }
    }
}

/// Does the env name a UTF-8 locale? `LC_ALL` > `LC_CTYPE` > `LANG` (the POSIX
/// precedence); a value containing `UTF-8`/`utf8` (case-insensitive) is UTF-8.
/// Absent or non-UTF-8 ⇒ ASCII glyphs (spec §3 degradation).
fn utf8_locale(env: &EnvSnapshot) -> bool {
    let locale = env
        .get("LC_ALL")
        .or_else(|| env.get("LC_CTYPE"))
        .or_else(|| env.get("LANG"))
        .unwrap_or("");
    let lower = locale.to_ascii_lowercase();
    lower.contains("utf-8") || lower.contains("utf8")
}
