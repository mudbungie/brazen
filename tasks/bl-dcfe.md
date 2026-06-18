+++
title = "Carry the provider error body on the model-discovery path + dedup src/run single-source forks"
created = 1781816027
updated = 1781816032
claimant = "Holloway"
priority = 2
tags = ["cleanup"]
+++
Single-source/carry-the-fact cleanup of src/run/ (four verified review findings, same region):

(1) MAJOR carry-the-fact: fetch_models (models.rs) returns on non-2xx WITHOUT reading the body; http_status_err hard-codes message + provider_detail:None. The data plane does the opposite via the shared json::http_error (drains the non-2xx body, carries it VERBATIM in provider_detail + best-effort message). fetch_models is shared by the list-models verb AND the serve probe, so 400/401 diagnostics are thrown away on discovery while surfaced on generation. FIX: drain resp.body and route through json::http_error(&body,status); delete http_status_err; add pub(crate) re-export of json::http_error in protocol/mod.rs. Spec model-discovery §2/§5.2: 'like the data plane' -> make literal (carry the body), fix doc word.

(2) NIT fold-in: one drain helper. models.rs + respond.rs both define private drain. Hoist ONE fn drain(body)->Result<Vec<u8>,io::Error> to run::mod. models.rs maps io::Error into its CanonicalError message (keep carried {e}); respond.rs maps Err(_)=>().

(3) NIT fold-in: one 2xx boundary. respond.rs is_2xx documented 'one place the success/error split is named' but models.rs open-codes the range. Lift is_2xx to pub(super), call from models.rs.

(4) MINOR one home for seam projection: serve.rs + models.rs open-code the IDENTICAL ResolvedConfig -> ProviderCtx/AuthCtx projection. Mirror timeouts() query: add auth_ctx(&self) and provider_ctx(&self, beta) on ResolvedConfig; both call sites use them.

Tests: tighten list_models non-2xx test to assert body reaches provider_detail/message; add probe-path test (serve probe non-2xx surfaces provider_detail in-band). Gate: make check (fmt, clippy -D, 100% line cov, 300-line cap).