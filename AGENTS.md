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
- **Full `make check`** (fmt-check + clippy `-D warnings` + the 300-line cap + 100% line
  coverage via `cargo llvm-cov --fail-under-lines 100`), once Rust sources exist. The Makefile
  is the single source of truth for *what* the gate is; the hook decides *when* it runs.
- Enable once per clone: `make hooks` (sets `core.hooksPath`).

**2. Merge to origin — manual, by design.** `bl close` delivers to **local** `main`
only. Pushing is a deliberate step: `git push origin main`. No auto-push hook.

**3. Docs — advisory, Claude Code only.** A `PreToolUse` hook
(`.claude/settings.json` → `.claude/hooks/docs-reminder.sh`, needs `jq`) reminds
the agent to bring `specs/`, `README.md`, and `AGENTS.md` in line with the change
before a `bl close`. Non-blocking: the close proceeds regardless.

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
