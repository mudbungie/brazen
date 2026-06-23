+++
title = "Fully-automated, build-gated release: publish automatically when CI succeeds on main (workflow_run gate), not a manual click — keep workflow_dispatch as a manual override"
created = 1782197514
updated = 1782197515
claimant = "Dialectic"
priority = 1
tags = ["impl"]
+++
User: 'fully automated github pipelines based on successful builds.' Replaces the hold-off manual-only gate (bl-c26a) with automation gated on a green build.

## release-plz.yml restructure
- Triggers: push [main] (maintain the Release PR) + workflow_run of CI 'completed' on main (the build gate) + workflow_dispatch (manual override).
- release-pr job: if github.event_name=='push' -> command: release-pr (version+CHANGELOG PR; never publishes).
- release (publish) job: if (workflow_run && conclusion=='success') OR workflow_dispatch -> command: release. Checkout the CI-passed sha (github.event.workflow_run.head_sha, fallback github.sha). release-plz release is idempotent (no-ops unless main has an unpublished version), so running on every green main build is safe; it actually publishes only after a Release PR merge bumps the version.
- workflows:["CI"] must match ci.yml name: CI.

## Flow (hands-off)
feature PR merged -> CI green -> Release PR auto-maintained; merge the Release PR -> CI green -> auto-publish to crates.io + tag + GitHub Release -> release-binaries.yml attaches bz binaries. The Release-PR merge is the human control point; the token is the enable switch. First release: 0.0.1 (already on main, unpublished) auto-publishes on the next green main build once CARGO_REGISTRY_TOKEN is set.

## Docs
README §Releasing: rewrite from manual-click to automated build-gated flow; note that setting CARGO_REGISTRY_TOKEN arms auto-publish.

## Close gate
YAML parses; make check green.