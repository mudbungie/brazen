//! Provider-NATIVE raw bodies for the `--raw` projection check (bl-5f6e). `--raw`
//! sends stdin verbatim — no `encode` — so the body on that wire must be the
//! DIALECT's native shape, not brazen's canonical one. The messages-dialects
//! (anthropic/openai/mistral) accept the canonical messages-shaped body natively,
//! so their rows reuse it; every other dialect carries its native shape here. Which shape a row speaks is a per-row DATUM (`Row.raw`), never a
//! branch on the provider name.

use serde_json::json;

use super::{Row, PROMPT, SYSTEM};

/// The native wire shape of a row's `--raw` body — DATA on the `Row`.
pub enum RawBody {
    /// The canonical messages-shaped body IS this dialect's native body
    /// (anthropic / openai / mistral): reuse `Row::request`.
    Messages,
    /// OpenAI Responses: `instructions` + `input`, no `system`/`messages` (codex
    /// 400s `Unsupported parameter: system`). `store:false` is codex's mandate and
    /// harmless on a standard Responses row.
    Responses,
    /// Ollama `/api/chat`: flat string `content`, the model in the BODY.
    Chat,
}

/// Build the `--raw` stdin body for a row: the same system + prompt every other
/// assertion sends, projected onto the row's native raw shape.
pub fn request(row: &Row, model: &str) -> String {
    match row.raw {
        RawBody::Messages => row.request(false),
        RawBody::Responses => json!({
            "model": model,
            "instructions": SYSTEM,
            "input": [{ "role": "user", "content": [{ "type": "input_text", "text": PROMPT }] }],
            "stream": true,
            "store": false
        })
        .to_string(),
        RawBody::Chat => json!({
            "model": model,
            "messages": [
                { "role": "system", "content": SYSTEM },
                { "role": "user", "content": PROMPT }
            ],
            "stream": true
        })
        .to_string(),
    }
}
