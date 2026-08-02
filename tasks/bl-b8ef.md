+++
title = "-f media-detection: attach images/PDFs from the CLI"
created = 1785642500
updated = 1785642500
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
architecture.md §5.5 names this as the deferred separate ball: -f is text-only; a mapped file extension should emit Content::Image / Content::Document (Base64) instead of Content::Text. The canonical variants and all five per-dialect encodes already exist (bl-956c); this ball adds only the CLI-side detection.

Design (to be written into §5.5):
- Extension table, NOT magic-byte sniffing (the explicit-signal rule): png/jpg/jpeg/gif/webp → Image{Base64}, pdf → Document{Base64, application/pdf}; case-insensitive.
- Everything else (incl. -f -) stays the text path unchanged: read_to_string, non-UTF-8 → 66.
- Media reads via fs::read (binary), STANDARD base64; missing/unreadable → 66 as today.
- Docs: SKILL.md/README -f lines; Cargo.toml base64 audit note (URL_SAFE_NO_PAD-only claim).