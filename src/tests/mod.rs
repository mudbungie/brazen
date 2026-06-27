//! In-crate relocation of the unit/integration suite (arch §9.8). These modules
//! were `tests/*.rs` integration crates; living here lets them exercise crate
//! internals through the `#[cfg(test)]` prelude in `lib.rs` WITHOUT those
//! internals being `pub` — so test layout no longer drives the semver surface.
//! Each `*_support` module is shared harness; the rest are one test file each.
//! `src/tests/` is excluded from the coverage denominator (it is the tests, not
//! the lib-under-test — Makefile `cov`), like `src/native`.

mod config_support;
mod decode_full_support;
mod list_models_support;
mod login_support;
mod oauth_pure_support;
mod responses_decode_errors_support;
mod run_support;

mod ambient_discovery;
mod anthropic_decode;
mod anthropic_encode;
mod anthropic_fixtures;
mod auth_apply;
mod canonical_error;
mod canonical_event;
mod canonical_model;
mod canonical_request;
mod cli_args;
mod config_body_defaults;
mod config_dump;
mod config_env;
mod config_errors;
mod config_fill;
mod config_partial;
mod config_preamble;
mod config_resolve;
mod config_route;
mod config_strip;
mod cross_check_basic;
mod decode_full;
mod decode_full_structured;
mod google_decode_errors;
mod google_encode;
mod google_fixtures;
mod list_models;
mod list_models_help;
mod login_browser;
mod login_device;
mod model_discovery_decode;
mod oauth2_provider_recipe;
mod oauth_pure;
mod oauth_pure_callback;
mod oauth_refresh;
mod ollama_decode_errors;
mod ollama_encode;
mod ollama_fixtures;
mod oneliner;
mod openai_decode_errors;
mod openai_encode;
mod openai_fixtures;
mod pipeline_input;
mod pipeline_parse;
mod pipeline_pretty;
mod pipeline_pretty_footer;
mod pipeline_sink;
mod pipeline_style;
mod protocol_sse_determinism;
mod protocol_sse_framers;
mod responses_decode_errors;
mod responses_decode_finish;
mod responses_encode;
mod responses_fixtures;
mod run_cache;
mod run_config;
mod run_control;
mod run_failures;
mod run_modes;
mod run_stream;
mod seams_config;
mod seams_protocol;
mod seams_store;
mod seams_transport;
