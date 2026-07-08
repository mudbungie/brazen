//! The synthesized-stream mechanics shared by the structure-less decoders
//! (`openai` chat, `google_genai`, `ollama_chat`) — protocol-dedup spec, D2.
//!
//! These are the drain/index discipline the canonical model *implies*, NOT a
//! flow engine: pure functions of `(&mut DecodeState, &mut Vec<Event>)` with zero
//! wire knowledge. Each provider keeps its own `decode`/content/tool-call/terminal
//! flow and merely *calls* these — the wire-shaped parts (tool-call wholeness, the
//! `tool_index` map, `Usage`/`Finish` placement) stay where they diverge, in the
//! provider. The explicit-structure decoders (`anthropic`, `openai_responses`) key
//! off the wire's own index and never touch this module.

use crate::canonical::{ContentKind, Event};
use crate::protocol::{DecodeState, OpenBlock};

/// The next canonical index to assign — the open map's dense `0..n` (blocks never
/// close mid-stream), never stored (arch §3.1; single source of truth).
pub(crate) fn next_index(state: &DecodeState) -> u32 {
    state.open.len() as u32
}

/// The canonical index of the open block of `kind`, opening one if none is open
/// (identity before content): the at-most-one lazy block of its kind. Drives both the
/// lazy text block and the Google `thought` thinking block.
fn open_lazy(state: &mut DecodeState, out: &mut Vec<Event>, kind: ContentKind) -> u32 {
    if let Some((i, _)) = state.open.iter().find(|(_, b)| b.kind == kind) {
        return *i;
    }
    let i = next_index(state);
    state.open.insert(i, OpenBlock { kind: kind.clone() });
    out.push(Event::ContentStart { index: i, kind });
    i
}

/// The canonical index of the open text block, opening one if none exists (the lazy
/// text block): identity before content. At most one text block is ever open.
pub(crate) fn open_text(state: &mut DecodeState, out: &mut Vec<Event>) -> u32 {
    open_lazy(state, out, ContentKind::Text {})
}

/// The canonical index of the open thinking block, opening one if none exists — the
/// Google `thought` analog of `open_text` (§4.4). At most one thinking block is open.
pub(crate) fn open_thinking(state: &mut DecodeState, out: &mut Vec<Event>) -> u32 {
    open_lazy(state, out, ContentKind::Thinking { id: None }) // Google has no reasoning-item id
}

/// The terminal drain: synthesize `ContentStop` for every still-open block in
/// ascending index order (the synthesized wire sends no per-block stop), removing
/// each. The provider then arranges `Usage`/`Finish`/`terminated` per its wire.
pub(crate) fn drain(state: &mut DecodeState, out: &mut Vec<Event>) {
    let mut open: Vec<u32> = state.open.keys().copied().collect();
    open.sort_unstable();
    for index in open {
        state.open.remove(&index);
        out.push(Event::ContentStop { index });
    }
}
