//! The self-describing stdout short-circuits (arch §5.5): the one `--help` screen,
//! the `--version` line, their shared write-and-flush [`emit`], and the
//! `--dump-config` printer. Each wins BEFORE any config/network and exits 0 — a
//! probe must answer even with a broken config or no provider — and each is the
//! SINGLE doc for every entry (`run`, `--list-models`, `--login`), so the screens
//! can never drift apart.

use std::io::Write;

use crate::canonical::ExitClass;
use crate::config::{dump_config, EnvSnapshot, PartialConfig};

/// `--dump-config` (config §6): resolve the layers minus defaults, print the TOML
/// to stdout, exit 0. A bad env scalar surfaces as 78 on stderr (the same dump
/// re-runs the env projection, where the failure is reachable).
pub(super) fn dump(
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    flags: PartialConfig,
    env: &EnvSnapshot,
    file: PartialConfig,
) -> u8 {
    match dump_config(flags, env, file) {
        Ok(toml) => match stdout
            .write_all(toml.as_bytes())
            .and_then(|()| stdout.flush())
        {
            Ok(()) => ExitClass::Ok.code(),
            Err(io) => ExitClass::from_io(&io).code(),
        },
        Err(e) => super::fail_early(stderr, e.into()),
    }
}

/// Print a fixed discovery document (`--help` / `--version`) to stdout, exit 0 —
/// the shared write-and-flush of the two self-describing short-circuits (§5.5),
/// mirroring [`dump`]'s stdout half: a broken stdout maps through `from_io` (so
/// `--help | head` is SIGPIPE/141, never a silent 0). `pub(crate)` so the control
/// flags (`--list-models`, `--login`) honor the SAME short-circuit with the SAME doc —
/// one help screen, not three.
pub(crate) fn emit(stdout: &mut dyn Write, doc: &str) -> u8 {
    match stdout
        .write_all(doc.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Ok(()) => ExitClass::Ok.code(),
        Err(io) => ExitClass::from_io(&io).code(),
    }
}

/// The crate's own version (`Cargo.toml`'s `[package] version`, via Cargo's
/// compile-time env var). Re-exported at the crate root as `brazen::VERSION` so a
/// downstream that links `brazen` reads the linked crate's version DIRECTLY — the
/// linked crate is the source of truth — instead of mirroring the pin by parsing a
/// manifest. The sibling [`VERSION_LINE`] is the `bz` CLI's rendering of this same
/// fact; `concat!` takes only literals, so both derive from `env!("CARGO_PKG_VERSION")`
/// rather than one from the other.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The `--version` line: the package version (Cargo's, the single source) + newline.
/// `pub(crate)` so every entry's `--version` prints the one line (see [`emit`]).
pub(crate) const VERSION_LINE: &str = concat!("bz ", env!("CARGO_PKG_VERSION"), "\n");

/// The `--skill` document: the embedded agent-facing skill card — richer than the
/// terse `--help` synopsis, with a worked command for every capability. A LITERAL
/// file (`SKILL.md`, at the repo root beside `README.md` so it is directly readable
/// and includable on its own), compiled in via `include_str!` so the shipped binary
/// carries it with no runtime file to find — the same single-source pattern as the
/// bundled `defaults.toml`. Emitted verbatim through the shared [`emit`] short-circuit,
/// so `--skill` is a discovery probe of the same family as `--help`/`--version`.
pub(crate) const SKILL: &str = include_str!("../../SKILL.md");

/// The `--help` document and the friendly bare-invocation hint (§5.5): one screen —
/// synopsis, the input model (positional prompt XOR a canonical request on stdin),
/// the control short-circuit flags, the flag list, and the exit-code table (§8). Kept
/// tight and POSIX-conventional; the single source for EVERY entry's `--help` stdout
/// (`run`, `--list-models`, `--login`), the bare-on-tty stderr usage, and login's usage.
pub(crate) const HELP: &str = concat!(
    "bz ",
    env!("CARGO_PKG_VERSION"),
    " — a stateless LLM adapter: one request, one round-trip, one POSIX exit.\n",
    "\n",
    "USAGE:\n",
    "    bz [FLAGS] \"PROMPT\"        one-shot: the positional prompt is the request\n",
    "    echo '{…}' | bz [FLAGS]    pipe a canonical request (JSON) on stdin instead\n",
    "    bz --login --provider <id> [--browser]   |   bz --list-models [--provider <id>]\n",
    "\n",
    "The request arrives exactly one way: a positional PROMPT (argv) XOR a canonical\n",
    "request on stdin. A prompt wins and stdin is not read. A leading bare word is\n",
    "ALWAYS a prompt — control operations are flags, never verbs. Output is a\n",
    "projection chosen by flag; the default is plain text.\n",
    "\n",
    "CONTROL (each replaces the data-plane run with a control action, then exits):\n",
    "    --login              obtain and store an OAuth/SSO credential for --provider\n",
    "                         (the one interactive surface; never entered by the data\n",
    "                         plane). Default: the headless device flow (shows a code to\n",
    "                         enter on another device). --browser: the loopback browser\n",
    "                         flow (opens a URL, captures the redirect).\n",
    "    --list-models        one GET: list the resolved provider's models\n",
    "    --count-tokens       one round-trip: provider-accurate input-token count of the\n",
    "                         request (read as for a run). Providers with no count endpoint\n",
    "                         decline (78); output is {\"input_tokens\":N} (--json) else N.\n",
    "    --serve              serve the [ingress] masquerade over HTTP (the route path\n",
    "                         picks the codec: OpenAI/Anthropic clients reach any provider);\n",
    "                         runs until SIGINT/SIGTERM\n",
    "    --dump-config        print the merged config as TOML, exit 0\n",
    "    --skill              print the fuller skill doc (worked examples), exit 0\n",
    "    --help, -h           print this help, exit 0\n",
    "    --version, -V        print the version, exit 0\n",
    "\n",
    "FLAGS:\n",
    "    --provider <id>      provider row id (else routed from the model)\n",
    "    -m, --model <id>     model id; a partial/absent id resolves against the cache\n",
    "    --api-key <key>      inline credential (else the credential store / env)\n",
    "    --system <text>      leading system prompt\n",
    "    --max-tokens <n>     generation cap\n",
    "    --temperature <f>    sampling temperature\n",
    "    --top-p <f>          nucleus sampling\n",
    "    --stream/--no-stream stream the response (default) or fold one JSON body\n",
    "    --thinking           include reasoning/thinking output (text mode)\n",
    "    --text               human-readable text (default)\n",
    "    --json               the full NDJSON canonical event stream\n",
    "    --raw                pass bytes through verbatim, provider-native both ways\n",
    "    --in <dialect>       read ONE client-dialect request (openai_chat) from stdin\n",
    "                         and write the dialect response to stdout (SSE if it asks\n",
    "                         stream:true) — the one-shot ingress filter\n",
    "    -f, --file <path>    attach a file's text as context (repeatable; before the prompt)\n",
    "    --input <file>       read the request from a file instead of stdin\n",
    "    --config <file>      use this config file (else the default search path)\n",
    "    --timeout <s>        abort on N seconds of upstream silence (connect/headers/between chunks)\n",
    "\n",
    "EXIT CODES (sysexits):\n",
    "    0    success (incl. a provider refusal — a 200)\n",
    "    64   usage: bad/unknown flag, malformed stdin request\n",
    "    66   --input file missing or unreadable\n",
    "    69   transport error, upstream 4xx (incl. 429), premature EOF\n",
    "    70   upstream 5xx (retryable)\n",
    "    77   auth: 401/403, missing credentials, login/refresh failure\n",
    "    78   config: no/unknown/ambiguous provider or model, bad config\n",
    "    130/141/143  interrupted by signal (SIGINT/SIGPIPE/SIGTERM)\n",
);
