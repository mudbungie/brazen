+++
title = "Unblock the release gate: stop depending on the sunset macos-13 Intel runner (cancelled 3/3, never executes). Cross-build x86_64-apple-darwin on the Apple-Silicon runner (build-only in CI + cross-built prebuilt binary); reconcile portability docs"
created = 1782284280
updated = 1782284283
claimant = "Dialectic"
priority = 1
tags = ["impl"]
+++
macos-13 (GitHub's deprecated Intel mac runner) is cancelled every run — it never executes, so timeout-minutes can't catch it and CI never goes green, which blocks the workflow_run auto-publish gate. Everything else (incl. both Windows after bl-f51d) is green.

## Changes
- ci.yml matrix: macos-13/x86_64-apple-darwin -> { runner: macos-14, target: x86_64-apple-darwin, build_only: true }. Gate the test step with if: !matrix.build_only so x86-mac cross-BUILDS on Apple Silicon (reliable) but skips native test (no GitHub Intel runner exists). The other 6 entries still build+test natively. Update the 'every entry is a NATIVE runner' comment.
- release-binaries.yml: x86_64-apple-darwin runner macos-13 -> macos-14 (taiki-e cross-builds the binary for upload; no native run needed), so Intel-Mac users still get a prebuilt bz.
- architecture.md §10: note x86_64-apple-darwin is cross-built (build-proven) on the arm runner because GitHub sunset macos-13; the other targets execute natively. Honest reconcile of 'portability proven by execution'.
- README §Platform support: footnote that x86-mac is build-verified (cross-compiled), not native-tested, in CI.

## Close gate
YAML parses; make check green. (Real signal: next CI run goes green with no macos-13.)