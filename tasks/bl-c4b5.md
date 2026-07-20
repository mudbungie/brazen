+++
title = "Move data/skill.md to repo-root SKILL.md so it can be surfaced/included directly"
created = 1784522876
updated = 1784522876
priority = 5
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
Rename data/skill.md -> SKILL.md (repo root): the skill card is a useful standalone file to surface/include directly, not buried build data. Update include_str! path in src/run/discovery.rs plus the doc references in src/cli/mod.rs and specs/architecture.md.