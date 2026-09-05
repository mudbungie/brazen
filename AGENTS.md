# AGENTS.md — brazen

Conventions for anyone (human or agent) working in this repo.

## Workflow

- **Design first, implement second.** A capability begins as a spec in `specs/` (a living
  document, edited like code), then implementation follows.
- **Task tracking is `bl` (balls).** Run `bl prime --as <you>` at session start; `bl list`
  shows ready work. Claiming a task materializes a `work/<id>` worktree — **all edits happen
  there**. `bl close` delivers the worktree to `main` and runs the pre-commit gate.
- **Never edit `main` directly.** Always work in a `bl` worktree and let `bl close` deliver.
- **Never credit AI or tooling in commit messages.**

## Close gates

Three gates fire around delivery. The first two are git-native (they fire for
any committer, human or agent); the third only fires when Claude Code drives.

**1. Tests — `.githooks/pre-commit`, hard.** Runs on plain `git commit` and on
`bl close` delivery. Enforces:

- **No code file (`*.rs`) exceeds 300 lines.** Docs (`*.md`) and config (`*.toml`, …) are exempt.
  Enforced repo-wide by `make linecount` (folded into `make check`, scanning the tracked
  `git ls-files '*.rs'` set) — the cap (`300`) lives in exactly one place; the hook just
  runs `make check`.
- **No secret, identity or session artifact in the tracked tree.** `make leak-scan`
  (folded into `make check`) is the disclosure gate — see "The disclosure gate" below.
- **Full `make check`** (fmt-check + clippy `-D warnings` + the 300-line cap + the
  disclosure scan + 100% line coverage via `cargo llvm-cov --fail-under-lines 100`), once
  Rust sources exist. The Makefile
  is the single source of truth for *what* the gate is; the hook decides *when* it runs.
- **The commit MESSAGE, which `pre-commit` never sees** — `.githooks/commit-msg` runs the
  same scanner over it, because a token or a home path pasted into a message is disclosed
  exactly as a tracked file would be, and is harder to remove afterwards.
- Enable once per clone: `make hooks` (sets `core.hooksPath`; it seats both hooks).

**2. Publish the new tip — automatic, push + local install.** `bl close` delivers to
local `main`, and a `reference-transaction` hook (`.githooks/reference-transaction`)
then pushes `main` to origin **and installs `bz` from that tip into `~/.cargo/bin`**. It
is a reference-transaction hook, not post-commit, because delivery moves the ref by a
plumbing compare-and-swap — no `git commit` ever runs on `main`, so post-commit would
never fire. Nothing re-runs the tests here: main cannot advance until gate 1 passed, so
"main moved" already means "the suite passed".

- **Push** — capped at 10s (`timeout 10 git push origin main`) and non-fatal: if it fails
  or expires (offline, rejected), the hook warns on stderr and the delivery stands —
  recover with a manual `git push origin main`. The cap only bounds the attempt; the
  system stays convergent either way, since the next successful push carries any backlog.
- **Install** — `make install` (`cargo install --path . --locked --force`) run in a
  detached worktree at the new tip, parked at `.git/install-tree` with
  `CARGO_TARGET_DIR=target/` so the build stays warm (~10s). It builds from the *ref*, not
  from a checkout: a delivery touches no working tree, so the root checkout is stale at
  that moment and would install pre-merge code. It runs **detached** (log:
  `.git/install.log`, named on stderr) so a delivery never waits on a compile, under an
  `flock` that re-reads `refs/heads/main` — overlapping deliveries converge on the tip
  instead of racing to install each other's older commits. Without `flock` on PATH the
  hook warns and skips; run `make install` by hand.

To skip both (e.g. working fully offline), unset the hooks path for the clone
(`git config --unset core.hooksPath`) and push/install manually when ready.
Clones wired with `make hooks` get this free; a clone chaining local hooks via
`core.hooksPath` needs a one-line `reference-transaction` shim that execs
`.githooks/reference-transaction` (forwarding `$1` and stdin).

**3. Docs — advisory, Claude Code only.** A `PreToolUse` hook
(`.claude/settings.json` → `.claude/hooks/docs-reminder.sh`, needs `jq`) reminds
the agent to bring `specs/`, `README.md`, and `AGENTS.md` in line with the change
before a `bl close`. Non-blocking: the close proceeds regardless.

## The disclosure gate

`make leak-scan` (bl-39f1, ported from yog; the rust-bootstrap template). The rest of
the gate asks whether the tree is well-formed. This asks whether it **discloses**
something — and it matters here twice over: brazen holds credentials (0600 cred files,
OAuth access and refresh tokens, an ambient `claude_code` recipe reading the CLI's own
login), and it publishes to crates.io, where a version cannot be recalled.

- **`scripts/leak-rules.sh` is the one definition of what may not be committed** —
  private keys, vendor API tokens, credential assignments, routable IPv4/IPv6/MAC
  addresses, absolute paths under any home root on any platform, email addresses outside
  the reserved documentation space, dialogue behind a speaker label, agent-session
  artifacts, credential-shaped file paths, and **content no rule can read**. Nothing is
  restated here; read the table.
- **It reads index BLOBS, not the worktree.** `git checkout-index` materializes the index
  into a scratch tree and the scan reads that, so the bytes scanned are the bytes
  committed. A leak that was `git add`ed and then overwritten with a clean copy on disk
  is still caught.
- **Two directions, and the self-test runs first.** A leak gate does not die by being
  wrong; it dies by silently matching nothing after a pattern is edited and then passing
  everything forever. Every rule owns a fixture in `scripts/leak-fixtures/` where every
  non-comment line must be flagged **by that rule** and must carry `FIXTURE_MARKER`
  (`notreal`) — no regex can tell a real secret from a fabricated one, so the value says
  which it is. `clean.txt` / `clean-paths.txt` are the near-misses that must NOT be
  flagged, because a gate that cries wolf gets bypassed and a bypassed gate is no gate.
- **There is no allowlist and no per-rule path exemption.** Where this tree tripped a
  rule on the port, the rule was narrowed with its reason written into it — a reviewable
  exception — or the tree was fixed. Four such narrowings exist and each is marked
  `BRAZEN` in `leak-rules.sh`. Findings are truncated to 12 characters: a finding must
  LOCATE a leak, never reprint it into a terminal or a CI log.
- **A tracked binary needs a `<name>.provenance.md` beside it.** The scanner refuses what
  it cannot read; the escape is a document naming what the bytes are, where they came
  from and what claim they serve (`tests/fixtures/transport/foreign_clienthello.bin.provenance.md`
  is the one instance). Delete the provenance and the gate goes red.
- **Two scopes.** Bare, it scans the whole tracked tree — the right question for a commit
  hook. `--commit REV` scans what ONE commit publishes: the blobs it adds or rewrites,
  plus its MESSAGE, which lands in no file at all.
- **The task store is scanned too.** Ball bodies are published text on `balls/tasks`, a
  ref on this same remote, and the source gate never sees a byte of it. Prevention is the
  operator's machine-wide `bl-leak-gate` balls plugin, which runs
  `<project>/scripts/leak-scan.sh` before the store is pushed — the scanner's PRESENCE in
  this tree is the whole of the opt-in, so nothing here configures it. Detection is
  `.github/workflows/store-scan.yml`, daily and on dispatch over the published ref.
  Prevention is local and bypassable; enforcement is remote and late.

### What a commit hook cannot promise

It scans **one tree**. Old commits, other refs, pull-request and issue text, release
notes, Actions logs, and already-published crate versions are all outside it, and no hook
can reach them. They are a checklist run by a person once per publication, not a gate:
sweep the history before a first public push; delete refs that should never have been
pushed; read the packaged file list before publishing, because `cargo publish` is
irreversible and a yanked version stays downloadable.

**One item of that list HAS since become a gate (bl-e087).** `Cargo.toml` declares an
`include` **allowlist** — the crate's source outside the `#[cfg(test)]` corpus, the two
files the build embeds (`SKILL.md`, `data/defaults.toml`), and the files the registry
renders — and `tests/packaged_files.rs` reads the real `cargo package --list` and fails
on any path outside those classes, in both directions. An allowlist and not an `exclude`
because the two failure modes are not symmetric: a missing `include` entry costs a build,
which is loud and reversible, while a missing `exclude` entry costs a publication that
cannot be recalled — the manifest states that reasoning beside the key. **Auditing the
list is still yours**: the guard judges file CLASSES, never content, and a secret pasted
into `src/` is inside every class it rules in.

## Architecture north stars

- **Single source of truth.** The canonical model is authoritative; protocols derive from it.
  Don't store what you can compute.
  - **Carry the fact; never reconstruct it from a lossy proxy.** If a component already knows a
    fact (the transport knows the HTTP status), thread it through to whoever needs it rather than
    re-deriving it downstream from a stand-in (guessing the status back from `error.type`/`code`
    strings). The proxy is the smell — and a derivation that happens to be lossless for one
    provider (Anthropic's error types bijection with status) silently breaks for the next (OpenAI
    reuses `invalid_request_error` across 400/401). Fix: carry the value (`Frame.status:
    Option<u16>`) and map it once in a shared table (`ErrorKind::from_http_status`). A bool that
    is really "is this fact present" is often a lossy projection of the fact itself — widen it to
    carry the value. Reconstruction-from-strings is legitimate ONLY where the fact genuinely does
    not exist (a mid-stream error on a 2xx stream has no governing status).
- **Minimize and deepen the interface.** Components meet only at it, never pairwise.
- **Dissolve special cases** into the general path with empty inputs. A new flag/config/verb
  is a smell — prefer an existing explicit signal.
- **Severability.** Removing a capability should delete config, not edit core code.
- **If it can't be tested, it isn't built.**

See `specs/` for the architecture; start with spec `0001`.
