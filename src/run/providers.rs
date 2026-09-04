//! The effective provider table (config §6.1): the `bz --list-providers` control
//! flag — the LOCAL sibling of `--list-models`. `--list-models` answers "what models
//! does provider X serve?" over one GET; this answers "which X's exist, and can I
//! reach them?" over ZERO round-trips. It exists because `--dump-config` deliberately
//! drops the defaults operand (config §6) and so can never show the embedded rows;
//! this listing keeps that operand because it writes no file.
//!
//! It is the one listing that is **offline by construction**: [`ProvidersIo`] carries
//! `stdout`/`stderr`/`CredStore` and no `Transport` at all, so "makes no network call"
//! is a property of the type, not of the code's discipline.

use std::io::Write;

use serde::Serialize;

use crate::auth::{fetch_cred, AuthCtx, CredSource};
use crate::canonical::{CanonicalError, ErrorKind};
use crate::config::provider::{AuthId, Provider};
use crate::config::{config_path, defaults, partial_from_env, read_config_file, OutMode};
use crate::registry::Registry;
use crate::store::{CredStore, Secret};

/// The injected seams + writers for one `bz --list-providers` (config §6.1), the
/// deliberately-thin sibling of `ListIo`. There is no `Transport`, no `Clock`, and no
/// `ModelCache`: the verb reads the merged config, the credential store, and any row's
/// `ambient` file, and that is the whole of its reach.
pub struct ProvidersIo<'a> {
    pub stdout: &'a mut dyn Write,
    pub stderr: &'a mut dyn Write,
    pub store: &'a dyn CredStore,
}

/// One listed row (config §6.1) — the SINGLE projection both output shapes render, so
/// the text table and the `--json` object can never drift into naming different facts.
/// `protocol`/`auth` are `String` rather than the `ProtocolId`/`AuthId` keys because
/// the rendered spelling must come from ONE place ([`spelling`], serde's own rename)
/// whichever shape prints it.
#[derive(Serialize)]
struct Row {
    name: String,
    protocol: String,
    auth: String,
    /// Does THIS row take `--reasoning`? The dialect's own [`Tuning`](crate::protocol::Tuning) declaration
    /// AND the row's `unsupported_body_keys` decline — computed from the row, never
    /// stored (config §6.1). Every shipped dialect projects effort, so the
    /// interesting bit is the per-row decline.
    effort: bool,
    /// Does THIS row take `--tier`? The same pair, over the dialect's `service_tier`
    /// spelling (providers.md §6.2): the OpenAI family and Anthropic have one,
    /// Google/Ollama/claude_code narrow it away.
    priority: bool,
    /// Which HEADLESS sign-in this row serves, if any (config §6.1): the
    /// `device.style` spelling (`rfc8628` / `codex`), `None` for a row that declares
    /// no `device` block and so can only be signed in through `--browser`. The STYLE
    /// rather than a bool, because the fact has a value and a consumer above brazen
    /// reads it as one — a `true` would be the lossy projection of it. Computed off
    /// the row like every other column: delete the block, lose the capability.
    device: Option<String>,
    credential: &'static str,
}

/// Run `bz --list-providers` and return the POSIX exit code (config §6.1). Reuses the
/// full flag parser and the SAME fold a run does — `flags.or(env).or(file).or(defaults())`
/// — then completes EVERY row instead of routing to one, so the built-in floor is
/// visible. Rows print in `providers` order, which IS routing priority (arch §4.3.1):
/// the head is the row a bare `bz "q"` reaches. Always exit 0 on a config that
/// completes; a malformed file / bad env scalar / incomplete row is the usual 78.
pub fn list_providers(args: &crate::cli::Args, io: &mut ProvidersIo) -> u8 {
    match run_list(args, io) {
        Ok(code) => code,
        Err(e) => {
            let _ = writeln!(io.stderr, "{}", e.message);
            e.exit_code()
        }
    }
}

fn run_list(args: &crate::cli::Args, io: &mut ProvidersIo) -> Result<u8, CanonicalError> {
    let flags = crate::cli::parse_args(&args.argv)?;
    // The discovery short-circuits ride the SAME flag layer and the SAME doc as the
    // data plane (arch §5.5): `bz --list-providers --help`/`--version` self-describe to
    // stdout and exit 0 BEFORE any config read — a probe must answer with a broken one.
    if flags.help {
        return Ok(super::emit(io.stdout, super::HELP));
    }
    if flags.skill {
        return Ok(super::emit(io.stdout, super::SKILL));
    }
    if flags.version {
        return Ok(super::emit(io.stdout, super::VERSION_LINE));
    }
    let file = read_config_file(&config_path(flags.config_path, &args.env))?;
    let env = partial_from_env(&args.env).map_err(CanonicalError::from)?;
    // The defaults operand STAYS — the one difference from `--dump-config`, and the
    // whole reason this verb exists (config §6.1): the dump omits it so a later
    // brazen's better default still reaches the operator, which costs the dump every
    // built-in row. A listing writes no file, so it pays no such price.
    let merged = flags.config.or(env).or(file).or(defaults());
    // The output shape is the SAME resolved fact the data plane folds, not the `--json`
    // flag alone: `BRAZEN_OUTPUT=ndjson` and a config-file `output = "ndjson"` select
    // the object form too, exactly as they do for `--list-models` (model-discovery §2).
    let json = merged.output == Some(OutMode::Ndjson);
    // The inline key is provider-AGNOSTIC (config §3.4) — it is not a row field, so it
    // is read once here and applied to every keyed row, exactly as a run would.
    let inline = merged.api_key.clone();
    let rows: Vec<Row> = merged
        .into_rows()?
        .iter()
        .map(|p| row(p, inline.as_ref(), io.store))
        .collect();
    print_rows(io.stdout, &rows, json).map_err(write_failed)?;
    Ok(0)
}

/// Project one completed row into its listed facts (config §6.1). `base_url` and
/// the row's body are deliberately absent: this verb names rows, `--list-models` reads
/// one row's models, and `--dump-config` carries the operator's row bodies. The two
/// TUNING facts are of the row-naming kind — "can I point `--reasoning`/`--tier` at
/// this row?" — and they are COMPUTED here, never stored: the dialect declares its
/// projection ([`Tuning`](crate::protocol::Tuning), beside its own `encode`) and the row declines by naming the
/// canonical key in `unsupported_body_keys`, which is the same pair `strip_unsupported`
/// enforces on the data plane. So the answer cannot disagree with what a run does, and
/// a consumer above brazen deletes its own copy of the protocol table.
fn row(provider: &Provider, inline: Option<&Secret>, store: &dyn CredStore) -> Row {
    let tuning = Registry::builtin().protocol(provider.protocol).tuning();
    Row {
        name: provider.name.clone(),
        protocol: spelling(&provider.protocol),
        auth: spelling(&provider.auth),
        effort: takes(provider, tuning.effort, "reasoning"),
        priority: takes(provider, tuning.priority, "service_tier"),
        device: provider
            .oauth
            .as_ref()
            .and_then(|o| o.device.as_ref())
            .map(|d| spelling(&d.style)),
        credential: credential(provider, inline, store),
    }
}

/// One tuning fact: the dialect projects the knob AND this row does not decline it
/// (config §4.1.1). Two data reads, no policy — deleting the row's
/// `unsupported_body_keys` entry restores the capability with no code edit.
fn takes(provider: &Provider, projected: bool, canonical_key: &str) -> bool {
    projected
        && !provider
            .unsupported_body_keys
            .iter()
            .any(|k| k == canonical_key)
}

/// The config spelling of a registry key (`openai_chat`, `api_key`), read from the SAME
/// serde rename `config.toml` parses through (arch §4.2) — never a second table beside
/// it, which would be two representations of one fact and free to drift.
fn spelling<T: Serialize>(id: &T) -> String {
    serde_json::to_value(id)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// The credential this row WOULD use — `resolved_secret`'s answer minus the network
/// (auth §3.1, §5.5), computed through the very same `fetch_cred`, so the column can
/// never disagree with what a run does. Token EXPIRY is deliberately not reported: a
/// stale OAuth token is refresh's business (auth §7.1), and an `expired` state would
/// make the listing look like a health check it is not.
fn credential(provider: &Provider, inline: Option<&Secret>, store: &dyn CredStore) -> &'static str {
    if provider.auth == AuthId::None {
        return "not required";
    }
    // `inline_key` is `StaticSecretAuth`'s bypass ONLY (auth §3.1): the `OAuth2` impl
    // never reads it, so reporting it on an oauth row would be a lie the run refutes.
    if provider.auth != AuthId::OAuth2 && inline.is_some() {
        return "inline";
    }
    let ctx = AuthCtx {
        store_key: &provider.name,
        inline_key: None,
        api_header: provider.api_header.as_ref(),
        oauth: provider.oauth.as_ref(),
        ambient: provider.ambient.as_ref(),
    };
    match fetch_cred(store, &ctx) {
        Some(f) if f.source == CredSource::Owned => "stored",
        Some(_) => "ambient",
        None => "missing",
    }
}

/// Print the listing (config §6.1): `--json` the one `{"providers":[…]}` object
/// (serde-direct, like the event stream), else the six columns space-padded to the
/// widest value, one row per line, no header — greppable and `awk`-able, the same
/// "one line per listed thing" shape `--list-models` prints. An empty table prints
/// nothing and exits 0: the loop over zero rows, not an empty-listing branch.
///
/// The two booleans render as ONE `tuning` column naming the knobs the row accepts
/// (`effort,priority`, `-` for none) rather than two bare `true`/`false` columns:
/// same facts, one line each, and `grep priority` is the question an operator asks.
/// It sits BEFORE `credential` because `credential` is the one column whose value can
/// contain a space (`not required`), so it stays last and whitespace-splitting a line
/// keeps working — and `device` sits beside it for the same reason.
fn print_rows(out: &mut dyn Write, rows: &[Row], json: bool) -> std::io::Result<()> {
    if json {
        let obj = serde_json::json!({ "providers": rows });
        return writeln!(out, "{obj}");
    }
    let tunings: Vec<String> = rows.iter().map(tuning_cell).collect();
    let devices: Vec<String> = rows.iter().map(device_cell).collect();
    let (name, protocol, auth) = (
        width(rows, |r| &r.name),
        width(rows, |r| &r.protocol),
        width(rows, |r| &r.auth),
    );
    let tuning = tunings.iter().map(String::len).max().unwrap_or(0);
    let device = devices.iter().map(String::len).max().unwrap_or(0);
    for ((r, t), d) in rows.iter().zip(&tunings).zip(&devices) {
        writeln!(
            out,
            "{:name$}  {:protocol$}  {:auth$}  {:tuning$}  {:device$}  {}",
            r.name, r.protocol, r.auth, t, d, r.credential
        )?;
    }
    Ok(())
}

/// The `tuning` cell: the accepted knobs by name, comma-joined in the column's own
/// order, `-` when the row accepts neither — the text rendering of the two booleans
/// the `--json` shape carries, never a second computation of them.
fn tuning_cell(r: &Row) -> String {
    let named: Vec<&str> = [("effort", r.effort), ("priority", r.priority)]
        .into_iter()
        .filter_map(|(name, yes)| yes.then_some(name))
        .collect();
    if named.is_empty() {
        return "-".to_owned();
    }
    named.join(",")
}

/// The `device` cell: the headless flow this row serves by name, `-` when it serves
/// none — the text rendering of the `--json` shape's `device`, never a second read of
/// the row.
fn device_cell(r: &Row) -> String {
    r.device.clone().unwrap_or_else(|| "-".to_owned())
}

/// The widest value of one column, the padding every row aligns to.
fn width(rows: &[Row], field: impl Fn(&Row) -> &String) -> usize {
    rows.iter().map(|r| field(r).len()).max().unwrap_or(0)
}

/// A stdout write failure for the listing → `Transport` (→69), the same pre-sink
/// mapping `--list-models` uses for its own listing write.
fn write_failed(e: std::io::Error) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Transport,
        message: format!("failed to write provider list: {e}"),
        provider_detail: None,
        retry_after_seconds: None,
    }
}
