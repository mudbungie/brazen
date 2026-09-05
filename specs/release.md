# Release process — the test ladder, the release gate, and the human steps

> **Living document.** Edited like code. This spec owns the **process**: which checks run when, what a
> failure means, and what a human must do around a publish. It does **not** own what any check *does* —
> the `Makefile` is the single source of truth for that, and the provider table in
> `tests/live_conformance.rs` is the single source of truth for which providers exist live. Prose here
> that restates a target's internals is a bug in this document.
> **Derives from:** [Architecture & I/O Contract](architecture.md), [Auth](auth.md) — §10 (the live OAuth rows).

---

## 1. Purpose & scope

brazen has release **automation** (`.github/workflows/release-plz.yml`: release PR → publish on green CI
→ binaries → branch prune) and had no release **process**: nothing said what must be true before a
version ships. The gap is not theoretical. `make check` — the commit gate and the CI gate — is
exclusively **offline**; every check that touches a real provider is `#[ignore]`d and env-gated, and no
workflow runs any of it. bl-f90e shipped a reasoning encoder that burned thinking tokens no caller could
see, while the live case named `reasoning-summary` — the exact test for it — sat RED and unrun (bl-1ad0)
and the offline suite was 100% green throughout, because it asserted what the code did rather than what
the dialect requires.

**In scope:** the ladder and the rule that places a check on it (§2); how the release gate runs, when,
and on what (§3); what a live failure means (§4); which providers are in scope and what a skip costs
(§5); the human steps (§6); the version-bump decision (§7); what this does not solve (§8); rejected
alternatives (§9).

**Out of scope:** the content of any target (`Makefile`), the wiring of `make release-check` and any
workflow change (bl-3dc4 implements what this document decides), and each suite's own assertions (the
suite's module doc owns those).

---

## 2. The ladder

**Decision — one rule places every check: a check runs at the *most frequent* rung it can run at
unattended, for free, hermetically, and deterministically; the first property it lacks demotes it one
rung.** There is no per-check debate and no taste involved: the properties are facts about the check.

| Rung | Fires on | Runs | Why here |
|---|---|---|---|
| **Commit** | `git commit`, and `bl close` delivery (`.githooks/pre-commit`) | `make check` | Hermetic, free, deterministic, seconds. Nothing cheaper catches it, so it runs at the highest frequency available. |
| **Merge to main** | push to `main`, and every PR (`.github/workflows/ci.yml`) | `make check` again on a clean runner, plus the supply-chain audit, the MSRV build, and the per-target build/test matrix | Still hermetic and deterministic, but it needs *hardware this repo does not have locally* (seven runners) and an advisory DB of the day. Not free in wall-clock, so it demotes from commit to merge. |
| **Release** | a human, on `main`'s tip, unattached to a particular release PR (§3) | the offline gate plus every live suite, including the spend-gated cases | Costs money, needs **personal** credentials, and asserts against services that answer nondeterministically. Fails three of the four properties, so it lands on the rarest rung — which is also the rung whose blast radius (an immutable crates.io version plus tagged binaries) justifies the cost. |

Credentials and gates, by rung: the commit and merge rungs need **none** — that is what "hermetic" means
here, and it is why they can run in CI and in a fork. The release rung needs `BRAZEN_LIVE=1` plus the
spend opt-in `BRAZEN_LIVE_FUZZ_SPEND=1`, a stored `openai-chatgpt` cred, whatever API keys the box holds
for the keyed rows, and a reachable local Ollama for the keyless one. Each suite already names its own
gate in its module doc; the release rung adds no new gate spellings.

**Decision — the release gate is exactly the live suites that run unattended from one command; an
interactive check cannot be a gate.** `tests/oauth_smoke.rs` opens a browser for consent and needs an
operator-supplied `oauth2` config row, so it is not on the ladder at all: it is a change-triggered manual
check, run when the generic OAuth code path changes, and its non-interactive circuits (revoked access,
revoked refresh, silent refresh) are covered on the release rung by `tests/live_oauth_openai.rs`.

---

## 3. The release gate

**Decision — the release gate is a human-run `make release-check` on a credentialed workstation, and it
runs on `main`'s tip.** There is no manual tag step to run "pre-tag" against: release-plz creates the
tag *during* publish.

Running against `main`'s tip rather than the release PR's head is sound **because the release PR's diff
is the version bump and the changelog only** — a precondition the auto-merge below now enforces
mechanically (guard 3: every changed file is one of `Cargo.toml`, `Cargo.lock`, `CHANGELOG.md`, or the
pull request is skipped), where this document could previously only state it.

**Amendment (bl-9d21) — the gate no longer has a pre-merge moment, because there is no longer a human
merge.** This section used to read "immediately before the release PR is merged", on the reasoning that
"the merge of the release PR is the only human control point in the pipeline, so that merge is the
moment the gate defends". The standing operator ruling of 2026-09-03 removed that control point:
`.github/workflows/release-automerge.yml` merges the release PR on a green CI verdict, because the
decision the pull request asked for was already made by the work that landed on `main`, and the publish
is gated *after* the merge on the same CI verdict. A release therefore waits on no hand at all.

What follows from that, and what does not:

- **The gate keeps its rung and loses its appointment.** It is still the rarest rung, still human-run,
  still the only place the live suites run; it is simply no longer pinned to one release. Run it on
  `main`'s tip on whatever cadence the credentialed box affords.
- **A red live suite is a ball, not a release hold** (§4 already says a live failure is a defect to
  file). Nothing holds a release now except CI, so a version that shipped between a red run and its fix
  is superseded by the next version — which is the only remedy an immutable registry ever had (§6.4:
  a published version is never re-published).
- **Anyone who does want to hold a specific release marks the pull request a DRAFT.** The auto-merge
  workflow's guard 4 already refuses a draft, so the hold costs no new flag, label, or mechanism, and
  un-drafting releases it on the next wake-up (release-plz refreshes the pull request on every push to
  `main`, and each refresh runs CI on it). This is the escape hatch for §6's steps, and it is opt-in:
  the default is that the machine merges.

**Decision (superseded by the amendment above) — the record of the run was a comment on the release PR
carrying the gate's roster and result; merging without it was the deviation.** With no human merge there
is no signoff to withhold and a comment gates nothing, so the record of a live run is the ball filed for
whatever it found, or nothing when it found nothing. No new file, flag, or marker replaces it.

The rungs below the release gate still hold at release time: the publish job is gated on a green CI
`workflow_run` and cannot ship an untested commit. The release gate adds the layer CI structurally
cannot reach; it does not replace it.

---

## 4. What a live failure means

A naive "all green or no ship" is unlandable here — a live suite can disagree for three unrelated
reasons — and its real-world outcome is a maintainer quietly weakening an assertion at release time.
So the answer is decided ahead of time, per case, in code.

**Decision — every live assertion is *deterministic* unless the suite declares that case
model-discretion, and an undeclared case is therefore blocking.** Default-deny is the whole point:
classification is a property of the case, authored when the case is authored, never a judgement made
under release pressure.

**Decision — the declaration is a field on the case, and the suite applies the retry itself; the
release gate never classifies.** The declaration lives in `tests/live_support/determinism.rs` and is
spelled on every case, so that enum is the single source of truth for which cases are discretion —
this document deliberately does not enumerate them, for the same reason §5 does not enumerate
providers. The retry runs where the declaration is, because the alternative is a second
classification list inside `scripts/release-check.sh`: two homes for one fact, drifting the first
time a case is added (bl-959b). What reaches the gate is therefore a case that has *already* spent
its budget; the gate quotes the suite's own discretion lines into the roster and decides nothing.

- **Deterministic → BLOCKING.** Exit codes, auth outcomes, whether the service accepted or rejected the
  wire shape, the presence of the canonical event grammar, the surfaced-error wording matrix, `--raw`
  passthrough purity, the unsupported-key strip. A failure is either a defect in brazen or a real change
  in the upstream dialect; both must be resolved in the tree before the version ships. **A tripwire that
  flips (a mandated 400 that starts answering 200) is MOVED to the acceptance set, never deleted** — the
  suite must keep guarding a silent re-imposition. This generalizes the drift policy already written
  into `tests/live_fuzz_openai.rs`.
- **Model-discretion → bounded retry, then advisory with explicit signoff.** Cases whose truth depends on
  what the model *chose* to emit — a reasoning summary the model may skip, a tool the model may decline
  to call, text a thinking budget may starve. **Decision — a declared-discretion case is re-run up to
  three times; passing any run is green, and failing all three permits the release only with a signoff
  in the release PR comment that names the case, the provider and model, and files a `bl` ball.** The
  re-runs are mechanical (the suite's own attempt budget, above); only the signoff and the ball are
  human, because neither is a thing a test runner can author. The ball is not optional politeness: an unrun-and-red live case is precisely what bl-1ad0 found, and the
  filed ball is the mechanism that stops one from rotting unnoticed again.
- **Neither — the check never ran.** A 429, a 5xx, a dead network, an expired credential: the service
  told us nothing about brazen. **Decision — a non-run is a SKIP, never a pass and never a failure**, and
  it is governed by §5 like any other skip.

---

## 5. Provider scope and the skip roster

**Decision — the providers in scope are exactly the rows of the table in `tests/live_conformance.rs` —
the ones carrying a `BRAZEN_LIVE_*_MODEL` override var — and this document deliberately does not
enumerate them, because the table is the single source of truth for which providers exist live.** A list
here would drift the first time a row lands.

Detection is already per-row data (a keyless TCP probe, a stored cred, or a named env key), and a row
that cannot authenticate on this box is skipped **with its reason printed** — never silently truncated.
Two rulings turn that suite behavior into release policy:

**Decision — zero providers exercised is a FAILED release gate, not a clean no-op.** The conformance
suite treats a credential-less box as a green no-op, which is right for a developer running it casually
and catastrophic for a release: "green" would mean "nothing was asked".

**Decision — a release may ship with skipped providers, but a provider whose row or protocol code changed
in the release window (`git log v<prev>..HEAD`) must have RUN; the roster of what ran and what skipped
goes in the release PR comment, so an unproven provider ships named rather than assumed.** Skip tolerance
is thereby tied to an existing explicit signal — the diff — instead of a new policy knob.

---

## 6. The human steps

Four. Since bl-9d21 the release PR merges itself on green CI (§3), so the first three no longer sit in
a window before a merge — they are things a person does to `main`, in this order, and a person who wants
them to precede a *particular* release marks that pull request a draft first (§3).

1. **Run the release gate** on `main`'s tip (§3, §5). File what it finds; there is no pull request
   comment to sign.
2. **Curate the changelog.** release-plz stages the next entry by dumping commit subjects; `CHANGELOG.md`
   is hand-authored, and the dump is its *input*, not its output. **Decision — a curated entry has one
   line per user-visible change in Keep-a-Changelog categories, each naming its ball id, and drops every
   commit with no user-visible effect (chore, docs, test-only) rather than listing it.** **Decision —
   curation is the LAST action before the merge, and `main` is frozen from the moment it starts**: every
   push to `main` refreshes the release PR from a new branch, which can discard a hand edit made on the
   old one. **Amended by bl-9d21 — that freeze is no longer the default, because the merge no longer
   waits.** Writing the prose into `## [Unreleased]` as the work lands, previously "an option, not an
   obligation", is now the shape that works unattended: prose on `main` rides the next refresh into the
   release PR. Curating on the pull request itself still works, and still needs the freeze — draft the
   pull request for the duration (§3) so the machine does not merge out from under the edit.
3. **Check the proposed version** against §7 and correct it on the release PR if the rule was missed.
   Correcting a version bump on a pull request that merges itself means drafting it first (§3); the
   cheaper correction is on `main`, where the next refresh picks it up.
4. **Verify the artifacts after publish.** The Release for `v<version>` must carry one archive per target
   in the binaries matrix (the workflow is the source of truth for that list), the tag must exist, and
   the version must be on crates.io. **Decision — a missing or failed target is backfilled with the
   existing `workflow_dispatch` `binaries_tag` input, never by re-publishing**; a published version is
   immutable, and the publish job is idempotent precisely so a re-run cannot help.

Attachment verification is necessarily *post*-publish: the binaries job needs the Release to exist before
it can upload to it. That ordering is fixed by GitHub, so no gate can front-run it (§8).

---

## 7. The version bump

The default is **patch**, and it is right: the release PR should always propose the smallest step. A
bigger bump is opted into by a standalone `[minor]` / `[major]` bracketed token in any commit landing in
the release window (the matching rules live in `release-plz.toml`).

**Decision — the bump keys on brazen's five public contracts: the `bz` CLI surface (flags, exit codes),
the canonical `--json` event contract, the config-file schema, the Rust library API, and the ingress
masquerade surface — `[major]` for a break in any of them, `[minor]` for an addition to any of them (a
new provider row, flag, event kind, or exported item), and patch for everything else.** Below `0.1.0`
cargo treats every bump as incompatible anyway, so the marker communicates to *humans* today; keeping the
discipline now is what makes the numbers mean something at 1.0.

**Decision — the marker rides the commit that causes it, because its author is the only person who
reliably knows whether the change breaks anything; if it was missed, the releaser corrects the version
directly on the release PR, which is the last authoritative statement of the version before publish.**
After publish there is no correction — the fix is the next release.

---

## 8. What this does not solve

- **Bus factor of one.** The release gate needs a workstation holding personal OAuth/SSO creds and
  several API keys; today exactly one person has that box, so releases are blocked on one human. The
  degradation path is already the design, not a feature to add: a maintainer with *partial* credentials
  can still ship, because the roster (§5) makes the unproven providers explicit rather than silent. The
  escape from bus-factor-one is a second human following the same documented credential recipes
  (`auth.md`), not a secrets-gated CI job (§9).
- **The services move after the gate runs.** The live gate proves the version was correct against the
  services *at release time*; an upstream dialect change the next day is a new ball and a new patch
  release, not a retroactive gate failure. Naming this is what keeps the gate from being blamed for
  drift it cannot observe.
- **The gate tests the tree, not the artifact.** Nothing here executes the published crates.io tarball or
  a downloaded prebuilt binary; §6.4 verifies that the artifacts *exist*. The standing mitigation is
  dogfooding: the `reference-transaction` hook installs `bz` from `main`'s tip on every advance, so the
  maintainer's box runs release code continuously.
- **Live coverage is single-platform.** The gate runs on one Linux box; Windows and macOS are proven
  build-and-test-green by the CI matrix and are never exercised against a live provider. Accepted: the
  native surface is deliberately tiny and the wire path is platform-independent.
- **Spend.** The gate costs real tokens per release, bounded by the suites' own small token caps. The
  spend opt-in stays a separate signal so a routine local live run remains free.

---

## 9. Rejected alternatives

- **A secrets-gated live job in CI.** Rejected structurally, not "for now": the two highest-value rows
  authenticate with personal OAuth creds that **rotate on use** and expire, so a repo secret cannot hold
  one — the refresh would have to write back into the secret. Reconsider only if per-provider service
  accounts with static credentials ever exist.
- **A manual `workflow_dispatch` live job.** Rejected: it moves *who presses the button*, not *where the
  credentials live*, so it inherits the whole problem above and adds a workflow.
- **A pre-*tag* human run.** Rejected as a non-existent moment: release-plz tags during publish, so there
  is no pre-tag window. Adopted in its corrected form — pre-*merge* of the release PR (§3).
- **Live suites at the merge-to-main rung.** Rejected by §2's rule on three counts (cost per merge,
  nondeterminism, credentials): it would keep `main` red for reasons that are not defects, which trains
  everyone to ignore the gate.
- **"All green or no ship."** Rejected: unlandable against model discretion, and its actual outcome is a
  silently weakened assertion. Replaced by declared discretion, bounded retry, and a recorded signoff
  that costs a ball (§4).
- **A signoff file, a `[release-ok]` commit marker, or a new config flag.** Rejected: new mechanism where
  an existing artifact — the release PR — already carries a durable, per-release, human-owned record.
- **Blocking publish on binary attachment.** Rejected as impossible ordering: the binaries upload to a
  Release that publish must create first (§6.4).
