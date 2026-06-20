+++
title = "Hold off on auto-publish: gate release-plz's crates.io publish behind a manual workflow_dispatch (push to main never releases; publishing is a deliberate click)"
created = 1781931558
updated = 1781931558
priority = 1
tags = ["impl"]
+++
User: 'Hold off before we actually click release, but set the table.' Pushing main currently triggers release-plz's release (publish) job. Make publishing a deliberate manual action.

## Change (release-plz.yml)
- Triggers: `push: [main]` AND `workflow_dispatch`.
- release-pr job: runs on push (+ dispatch) — keeps the version/CHANGELOG release PR fresh (harmless table-setting, no publish).
- release (publish) job: add `if: github.event_name == 'workflow_dispatch'` so it ONLY runs on a manual 'Run workflow' click. A push to main never publishes.
- Comment clearly: pushing main = CI validates; publishing = Actions -> Release-plz -> Run workflow (needs CARGO_REGISTRY_TOKEN).

## Docs
Update README §Releasing: pushing main does NOT publish; the deliberate click is a manual workflow_dispatch run once CARGO_REGISTRY_TOKEN is set.

## Close gate
workflows parse; make check green.