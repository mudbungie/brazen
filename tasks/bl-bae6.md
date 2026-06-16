+++
title = "Guard publish until ready: publish=false + version 0.0.0"
created = 1781652492
updated = 1781652494
claimant = "Dickie"
tags = ["impl"]
+++
Re-add publish=false to both crates (the one-line 'not yet' switch; release.yml stays inert until flipped) and drop [workspace.package] version to 0.0.0 to reflect pre-release reality. Metadata stays fully wired.