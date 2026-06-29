+++
title = "Implement -f/--file repeatable content attach: each file becomes a text Content part in the user message; the reframe bl-9daf pointed to (distinct from --input-as-list)"
created = 1782719446
updated = 1782719519
claimant = "Plane"
priority = 2
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
tags = ["feature"]
+++
User decision (2026-06-29): 'meet people where they are' when they have a couple of files to feed in. This is the CLEAN reframe bl-9daf surfaced — distinct from the (correctly wontfixed) '--input as a list of canonical-request DOCUMENTS'. LOCKED surface: `-f` / `--file <path>`, REPEATABLE (accumulate, NOT last-wins).

DESIGN (living doc first, then implement):
- CLI: -f/--file -> flags.files: Vec<PathBuf> (accumulating). Parse in cli.rs.
- Composition: read each file -> a Content::Text part; the user message = [file1 text, file2 text, ..., positional-prompt text]. Files are CONTEXT preceding the prompt, in ONE user message. Build in run/mod.rs request construction (read_request).
- Interaction rules (attack + document in architecture.md §5.5, reconciling the bl-9daf decision — that decision STANDS for --input; this is the separate content-attach answer):
  * positional prompt + files -> one user message (file parts then prompt).
  * files only, no prompt (bare) -> one user message of just the file parts.
  * a genuine piped CANONICAL REQUEST on stdin (no positional) + files -> pick the least-surprising rule and DOCUMENT it (e.g. append file Text parts to the request's user turn, or refuse 'can't combine --file with a piped canonical request'); decide, don't leave it accidental.
  * positional prompt present => stdin unread (the existing XOR invariant holds).
- Errors: a missing/unreadable file -> exit 66 (EX_NOINPUT), consistent with --input. A non-UTF8 file -> a clean error (text parts are UTF-8). Images/binaries are OUT OF SCOPE for v1 (note as a possible follow-up: detect image -> Content::Image).
- Confirm: -f composes with --reasoning/--system/--model (orthogonal).

CONSTRAINTS: 100% line coverage; every *.rs <= 300 lines; make check green. Tests: repeatable parsing; composition order; bare+files; prompt+files; file-not-found->66; non-UTF8->error; multi-file.

FILE SCOPE: src/cli.rs, src/run/mod.rs (+ a small helper module if needed to stay under 300), specs/architecture.md §5.5, tests. NOTE: a concurrent lane (the --reasoning task) is editing src/cli.rs and specs/architecture.md §3.1+§5.3 — fold main + resolve (keep BOTH flag arms / BOTH spec sections) at close.