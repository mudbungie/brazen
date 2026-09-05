# `declared-binary.bin` — provenance

The PASSING half of the `binary-content` rule's self-test (bl-39f1). Byte-identical
to `binary-content.bin`, which must be FLAGGED; the only difference between them is
this file.

| Fact | Value |
|---|---|
| Contents | 58 bytes of non-text filler, generated for this test |
| Secrets | none — it was never a capture of anything |
| Claim | that a tracked binary passes the scan iff a provenance document sits beside it, and that the escape is the document rather than a path on a list |

Delete this file and the self-test must go red, which is what proves the escape is
still the thing being consulted.
