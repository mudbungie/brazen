//! The `claude_code` Protocol impl (claude-code spec): the Claude Code CLI driven as
//! a pure model pass-through over the EXEC transport. `encode` projects the canonical
//! request onto the pinned `claude` argv + a stdin prompt (spec §2, §4); `decode`
//! consumes the CLI's `--print` stream-json NDJSON, DELEGATING each `stream_event`
//! payload — which IS an Anthropic Messages SSE event — to the `anthropic_messages`
//! decoder (spec §5; one Messages parser in the codebase). Pure, no IO: the spawn
//! itself is the native transport's (spec §3). Vendor-blind like every impl: it reads
//! only `ProviderCtx` (`exec` = the row's program), never a row name.

mod decode;
mod encode;

use crate::canonical::{CanonicalError, CanonicalRequest, Event};
use crate::protocol::{
    DecodeState, ExecSpec, Frame, Framing, ModelsShape, Protocol, ProviderCtx, Shapes, Tuning,
    WireRequest,
};

/// The one shared, stateless instance (arch §4.4) — registered as `&'static dyn`.
pub struct ClaudeCode;

impl Protocol for ClaudeCode {
    fn encode(
        &self,
        req: &CanonicalRequest,
        ctx: &ProviderCtx,
    ) -> Result<WireRequest, CanonicalError> {
        encode::encode(req, ctx)
    }

    /// No HTTP path — the exec transport never reads a URL (spec §4.3). Empty, so the
    /// shared spine's `{base_url}{path}` composes to the row's completed-empty
    /// `base_url` and stays inert.
    fn path(&self, _ctx: &ProviderCtx) -> String {
        String::new()
    }

    /// The stdin body is the prompt as prose, not JSON (spec §4.3). Stamped as a
    /// header by the shared spine; inert on the exec path.
    fn content_type(&self) -> &str {
        "text/plain"
    }

    fn decode(&self, frame: Frame, state: &mut DecodeState) -> Result<Vec<Event>, CanonicalError> {
        decode::decode(frame, state)
    }

    fn decode_full(
        &self,
        body: &[u8],
        state: &mut DecodeState,
    ) -> Result<Vec<Event>, CanonicalError> {
        decode::decode_full(body, state)
    }

    /// stream-json is newline-delimited JSON — the existing NDJSON framer verbatim
    /// (spec §5.1).
    fn framing(&self) -> Framing {
        Framing::Ndjson
    }

    /// `--effort` on the child argv only — the CLI carries no lane flag (providers §6.2).
    fn tuning(&self) -> Tuning {
        Tuning {
            effort: true,
            priority: false,
        }
    }

    fn shapes(&self) -> Shapes {
        // The ONE dialect that carries neither (claude-code §4.1, §4.2), and the
        // reason this declaration exists. `encode` rejects a non-empty `tools`, a
        // non-`Auto` `tool_choice`, and anything but exactly one `user` message —
        // `ParseInput`/64 rather than a fabricated transcript. Listed, a host can
        // refuse this row for a tool-bearing role before it spends a call (bl-5053).
        Shapes {
            tools: false,
            multi_turn: false,
        }
    }

    /// The honest decline (spec §7.2): the CLI has no models listing, and a static
    /// list would be a second home for Anthropic's catalogue. `--list-models` fails
    /// with the next move in the message; learn-on-success fills the cache forward.
    fn models_shape(&self) -> Option<ModelsShape> {
        None
    }

    /// The subprocess target as DATA (spec §3.1): the request-independent base argv,
    /// so the `--raw` spine (which skips `encode`) reaches the same child the typed
    /// path does — raw-in feeds stdin verbatim as the prompt, raw-out streams the
    /// CLI's NDJSON verbatim. `None` when the row carries no `exec` (the same absence
    /// `encode` rejects with a `Config` error).
    fn exec_spec(&self, ctx: &ProviderCtx) -> Option<ExecSpec> {
        ctx.exec
            .map(|program| encode::exec_spec(program, "", ctx.model))
    }
}
