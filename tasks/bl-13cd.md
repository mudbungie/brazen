+++
title = 'Documentation pass: README rewrite (Install + top quickstart `bz "question"`, trim the ~150-line Status wall), crate-level docs.rs docs + fix the 16 rustdoc broken links, reconcile all specs + AGENTS.md to the single-crate reality'
created = 1781927085
updated = 1781928064
claimant = "Dialectic"
priority = 2
tags = ["docs"]

[[blockers]]
id = "bl-c1e2"
on = "claim"
+++
Depends on collapse (bl-c1e2). User flagged this explicitly (#7 'huge oversight, needs a full documentation pass', #8, #9).

## README (it IS the crates.io + docs.rs landing page via readme=README.md)
- Add `## Install` near top: `cargo install brazen` (yields `bz`), + prebuilt-binary note once bl-6b51 lands.
- Add a 3-line quickstart immediately under the tagline: e.g. `export ANTHROPIC_API_KEY=… ; bz "What is the capital of France?"` — the headline bz "question" one-shot (bl-ce84) appears NOWHERE currently.
- Trim/relocate the 154-line 'Early implementation' Status wall (README lines 14–167) into a 'What works today' bullet list; move design-log narrative to specs/ or the release-plz CHANGELOG.
- Reconcile the lib-vs-bin framing: it's one crate `brazen` installing `bz`. Remove the §Releasing two-crate publish recipe (release-plz owns releasing now).
- Add badges (crates.io / docs.rs / license), homepage/documentation manifest fields (optional).

## docs.rs (lib target still publishes docs)
- src/lib.rs crate-level //! docs: add a line that brazen is the engine behind the `bz` CLI and its lib API is not yet a stability contract.
- Fix the 16 rustdoc warnings (broken intra-doc links: parse_callback, fill_absent, ambiguous crate::run, links-to-private-item, redundant link at transport.rs:23). Run `cargo doc --no-deps` clean.

## specs + AGENTS
- Sweep every 'two crates'/'bz crate'/'shim crate'/'workspace member' assertion in specs/architecture.md, config.md, auth.md, model-discovery.md and AGENTS.md so no doc lies about the topology. (architecture.md §9.5/§10 are amended in bl-c1e2; this is the rest.)

## Close gate
make check green; cargo doc --no-deps emits 0 warnings.