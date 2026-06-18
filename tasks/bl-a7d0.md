+++
title = "Model cache impl: dissolve probe → ModelCache seam + total select_model + serve cache lookup"
created = 1781821454
updated = 1781824714
claimant = "Tindall"
priority = 40
tags = ["impl"]
+++
Implement the lazy-match model cache per specs/model-discovery.md §4–§8 + architecture.md §1/§2/§4.3/§6.5 (delivered by bl-135d). This SUPERSEDES the probe built by the now-closed bl-67e2/bl-1189: the probe code is REMOVED, not extended. Design-first is done — bring code in line with the spec. 100% line coverage + 300-line/file cap (the close gate).

Spec quotes that pin the behavior (model-discovery.md):
- §5: "brazen NEVER lists automatically. bz list-models is the ONLY writer of the cache; the generation path is READ-ONLY against the cache." "The probe is dissolved. There is no needs_probe query and no ResolvedConfig.probe."
- §4: select_model is TOTAL — "a non-empty seed the cache cannot place is passed through unchanged" (verbatim); "the lone Config (78) error is seed=="" && models.is_empty()". 66 is NOT used.

DELIVERABLES (one capability; may be done as ordered sub-steps):

1. select_model total contract + Provenance (src/canonical/model.rs, currently 82 lines).
   - Add `pub enum Provenance { Cached, Verbatim }`.
   - Change sig: `pub fn select_model(models: &[Model], seed: &str) -> Result<(String, Provenance), CanonicalError>`.
   - Semantics: seed=="" → first default-flagged else models[0] (Cached); seed=="" && empty list → ErrorKind::Config (78), msg "no model given and no model cache for <provider>; pass --model or run `bz list-models`". seed!="" → exact match (Cached) → else first substring match, case-insensitive, list order (Cached) → else seed VERBATIM (Verbatim). REMOVE the old "unmatched seed → Config 78 NoMatch".

2. ModelCache seam (src/store.rs ~161 lines; double in src/testing/; impl in bz/src/native.rs ~111 lines).
   - `pub trait ModelCache { fn get(&self, provider: &str) -> Option<Vec<Model>>; fn put(&self, provider: &str, models: &[Model]); }`.
   - get is FORGIVING: missing/parse-error/garbage file → None (never Err). put is best-effort, ATOMIC (temp + rename); write failure warns, does not fail list-models.
   - XdgModelCache: $XDG_CACHE_HOME/brazen/models/<provider>.json; format = the {"models":[{id,default}]} shape list-models --json emits (reuse the serde of Model; do not re-invent).
   - In-memory double in testing (sibling of testing/store.rs CredStore double).

3. Spine widening (src/run/mod.rs `run`, src/run/serve.rs, bz/src/main.rs).
   - run() gains `cache: &dyn ModelCache` (after `store`, before `clock` — mirror arch §1 spine). Thread into serve. main.rs wires XdgModelCache; tests inject the double.

4. Dissolve probe + serve cache lookup (src/config/resolved.rs, src/config/resolve.rs, src/run/serve.rs:63-73).
   - REMOVE ResolvedConfig.probe (resolved.rs:37) and the needs_probe/row_has_prefixes query (resolve.rs:32-58). model_prefixes stays for ROUTING ONLY.
   - ADD ResolvedConfig.model_from_cache: bool (the carried provenance fact for the 404 hint).
   - REPLACE the serve probe block with the UNCONDITIONAL cache lookup (model-discovery §5.2):
       if !raw {
           let models = cache.get(&cfg.provider.name).unwrap_or_default();   // miss → empty list
           let (wire, prov) = select_model(&models, &cfg.model)?;
           cfg.model = wire;
           cfg.model_from_cache = matches!(prov, Provenance::Cached);
       }
   - serve becomes a SINGLE-send path (generation only). --raw still skips the lookup (encode bypassed, model never read).

5. list-models writes the cache (src/run/models.rs ~181 lines; fetch_models is at :73).
   - After a successful decode_models, call cache.put(provider, &models) — the SOLE write site. Best-effort (warn on fail, do not change exit). fetch_models is now used ONLY by the verb (serve no longer calls it).

6. 404-by-provenance hint (src/run/serve.rs / src/run/respond.rs).
   - On a 404 on the GENERATION request, enrich the error message by cfg.model_from_cache (model-discovery §5.3): Cached → "<model> was in the cache but the provider rejected it; the cache may be stale — re-run `bz list-models`"; Verbatim → "<model> is not in the model cache; run `bz list-models` to refresh or enable partial matching". Both EXIT 69 (unchanged); only the message differs.

TESTS (model-discovery.md §8; 100% line cov):
- select_model: empty seed → default (Cached); empty seed + empty list → Config 78; partial → exact-before-contains, first-in-order (Cached); non-empty no-match → seed verbatim (Verbatim); full id present → itself (Cached).
- ModelCache round-trip: in-memory put/get; unknown provider → None; XDG impl corrupt/missing → None; put atomic.
- serve cache lookup: MockTransport ONE send (no probe send); primed cache → partial expands in encoded body; empty cache → full id verbatim; --raw skips.
- 404 provenance: Cached 404 → 69 + stale hint; Verbatim 404 → 69 + not-in-cache hint.
- list-models verb: --json AND BRAZEN_OUTPUT=ndjson both emit {"models":[…]}; cache double records put; NoProvider/auth/non-2xx → 78/77/69-70.
- Method on the wire: get → Method::Get; new/encode → Post.
- DELETE the obsolete needs_probe + two-send serve-probe-orchestration tests.

GATES: every changed file ≤300 lines (watch model.rs); fmt + clippy -D warnings + cargo llvm-cov --fail-under-lines 100 (make check). README/AGENTS unaffected (no user-facing flag added; --fail-on-404 was dropped in design).