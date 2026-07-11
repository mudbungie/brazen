//! The ingress ENCODE state (ingress.md §2, §10): everything `encode_response`
//! threads across events — the mirror of the egress `DecodeState`, shared by
//! every ingress dialect. ONE state serves both client shapes because the
//! aggregate IS the stream accumulated (§10): every event folds into these
//! accumulators on both paths; the SSE shape additionally renders per-event
//! frames, and `End` renders either the `[DONE]` sentinel or the folded body.
//! It also carries the two caller join points the `--serve`/`--in` shell wires:
//! the masqueraded HTTP [`status`](IngressState::status) (§9) and the
//! replay-stash write pairs ([`take_stash`](IngressState::take_stash), §5) —
//! the encoder EMITS `(key, payload)` pairs; it never touches the stash itself.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::canonical::{CanonicalRequest, Content};
use crate::store::{content_key, Clock};

/// Cross-event encode state for one response. Constructed per request from the
/// DECODED canonical request (the client's shape knobs live there) plus the
/// fired-adaptations list (§4) and the injected [`Clock`] (§2 identity).
pub struct IngressState {
    /// Client-facing shape (§10): `stream:true` → SSE frames per event; else
    /// every event folds silently and `End` renders the one aggregate body.
    pub(crate) stream: bool,
    /// The client's `stream_options.include_usage` (forwarded through `extra`
    /// by the decoder): usage rides the final SSE chunk iff it asked (§2).
    pub(crate) include_usage: bool,
    /// `created` for the fabricated identity — the injected Clock, never `now()`.
    pub(crate) created: u64,
    /// The client-requested model: the identity fallback when upstream names none.
    pub(crate) fallback_model: String,
    /// Upstream identity from `MessageStart` (`None` until then; fabricated on use).
    pub(crate) id: Option<String>,
    pub(crate) model: Option<String>,
    /// Fired lossy adaptations not yet exposed as SSE comment lines (§4).
    pub(crate) pending: Vec<String>,
    /// The full fired-adaptations list, for the aggregate `"brazen"` field (§4).
    pub(crate) adaptations: Vec<String>,
    /// Open canonical blocks: canonical content index → this dialect's routing.
    pub(crate) slots: BTreeMap<u32, Slot>,
    // -- the fold: the aggregate IS the stream accumulated (§10) --
    pub(crate) text: String,
    pub(crate) refusal: String,
    pub(crate) tools: Vec<ToolAcc>,
    pub(crate) finish: Option<String>,
    pub(crate) usage: Option<Value>,
    /// The §9 masquerade envelope, set by an `Error` event (last one wins).
    pub(crate) error: Option<Value>,
    pub(crate) status: Option<u16>,
    /// Opaque replay payload blocks accumulated toward the stash (§5), wire order.
    pub(crate) blocks: Vec<Content>,
    stash: Vec<(String, Vec<u8>)>,
}

/// Where a canonical content index routes in the client dialect.
pub(crate) enum Slot {
    /// A text block: deltas ride `delta.content`.
    Text,
    /// A tool call: its position in `tools` (== the wire `tool_calls[].index`).
    Tool(usize),
    /// A reasoning block: no client slot — accumulates toward the stash (§5).
    Thinking(ThinkAcc),
    /// A block the dialect cannot carry (server tools, unknown kinds): dropped.
    Skip,
}

/// One tool call, accumulated: the aggregate fold, the first-chunk identity,
/// and (when a `SignatureDelta` lands on it) the stash block (§5).
pub(crate) struct ToolAcc {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) args: String,
    pub(crate) signature: Option<String>,
}

/// One reasoning block accumulating toward a `Content::Thinking` stash block (§5).
#[derive(Default)]
pub(crate) struct ThinkAcc {
    pub(crate) text: String,
    pub(crate) id: Option<String>,
    pub(crate) signature: Option<String>,
    pub(crate) encrypted: Option<String>,
}

impl IngressState {
    /// State for one response: shape knobs read from the decoded request
    /// (`stream`; `stream_options.include_usage` rides `extra` — the decoder
    /// forwards it verbatim), the fired-adaptations list the shell resolved
    /// (§4 — the encoder never reads config), and the injected time source.
    pub fn for_request(
        req: &CanonicalRequest,
        adaptations: Vec<String>,
        clock: &dyn Clock,
    ) -> IngressState {
        let include_usage = req
            .extra
            .get("stream_options")
            .is_some_and(|o| o["include_usage"] == Value::Bool(true));
        IngressState {
            stream: req.stream == Some(true),
            include_usage,
            created: clock.now(),
            fallback_model: req.model.clone(),
            id: None,
            model: None,
            pending: adaptations.clone(),
            adaptations,
            slots: BTreeMap::new(),
            text: String::new(),
            refusal: String::new(),
            tools: Vec::new(),
            finish: None,
            usage: None,
            error: None,
            status: None,
            blocks: Vec::new(),
            stash: Vec::new(),
        }
    }

    /// The HTTP status the listener answers with: the §9 masqueraded status once
    /// an `Error` event was encoded, else 200.
    pub fn status(&self) -> u16 {
        self.status.unwrap_or(200)
    }

    /// Drain the stash-write join point (§5): the `(key, canonical-JSON payload)`
    /// pairs `End` finalized. The LISTENER writes them to the `ReplayStash`; the
    /// encoder emits pairs and never does IO.
    pub fn take_stash(&mut self) -> Vec<(String, Vec<u8>)> {
        std::mem::take(&mut self.stash)
    }

    /// Response identity (§2): upstream's id when `MessageStart` carried one,
    /// else fabricated-but-well-formed from the injected Clock's `created`.
    pub(crate) fn wire_id(&self) -> String {
        self.id
            .clone()
            .unwrap_or_else(|| format!("chatcmpl-brazen-{}", self.created))
    }

    /// Upstream's model name, else the one the client asked for.
    pub(crate) fn wire_model(&self) -> &str {
        self.model.as_deref().unwrap_or(&self.fallback_model)
    }

    /// Close block `index` (ContentStop): a reasoning or tool block that carried
    /// an opaque replay payload becomes a canonical stash block (§5), wire order.
    pub(crate) fn close(&mut self, index: u32) {
        match self.slots.remove(&index) {
            Some(Slot::Thinking(t))
                if t.id.is_some() || t.signature.is_some() || t.encrypted.is_some() =>
            {
                self.blocks.push(Content::Thinking {
                    text: t.text,
                    signature: t.signature,
                    id: t.id,
                    encrypted_content: t.encrypted,
                });
            }
            Some(Slot::Tool(i)) if self.tools[i].signature.is_some() => {
                let t = &self.tools[i];
                self.blocks.push(Content::ToolUse {
                    id: t.id.clone(),
                    name: t.name.clone(),
                    input: parse_args(&t.args),
                    signature: t.signature.clone(),
                });
            }
            _ => {}
        }
    }

    /// `End`'s stash finalization (§5): serialize the payload blocks once, keyed
    /// by what the client provably echoes — EVERY tool-call id of a tool-bearing
    /// turn (each one joins on replay, so any echoed id recalls the payload),
    /// else the shared content hash of the turn's text. Canonical JSON cannot
    /// fail to serialize; a hypothetical failure degrades to an empty payload —
    /// an unparseable stash entry the reader treats as a miss (the §5
    /// fail-open), never a panic.
    pub(crate) fn finish_stash(&mut self) {
        if self.blocks.is_empty() {
            return;
        }
        let payload = serde_json::to_vec(&self.blocks).unwrap_or_default();
        if self.tools.is_empty() {
            self.stash.push((content_key(&self.text), payload));
        } else {
            for t in &self.tools {
                self.stash.push((t.id.clone(), payload.clone()));
            }
        }
    }
}

/// Accumulated `arguments` fragments → the stash block's `input` (parsed only
/// at block close, never mid-stream): `""` is the wire's empty-input convention
/// → `{}`; a fragment stream that never became JSON degrades to `null`, never a
/// panic (arch §9.5).
fn parse_args(args: &str) -> Value {
    if args.is_empty() {
        return Value::Object(Map::new());
    }
    serde_json::from_str(args).unwrap_or(Value::Null)
}
