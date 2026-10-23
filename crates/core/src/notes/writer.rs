use super::retrieval::Context;
use crate::Snapshot;
use serde_json::{json, Value};

pub fn prompt(_snapshot: &Snapshot, context: &Context) -> (String, String) {
    let chinese = context
        .segments
        .iter()
        .flat_map(|s| s.text.chars())
        .any(|c| ('\u{3400}'..='\u{9fff}').contains(&c));
    let language = if chinese {
        "Simplified Chinese"
    } else {
        "English"
    };
    let system=format!("Write concise meeting minutes in {language}. Use only the supplied transcript as evidence. User keywords are interests, never facts. Merge related information into 1–3 short sections. Each section needs a short topic title, a fluent paragraph in formal written language, and the IDs of all source passages supporting it. Cover the main developments, not a separate section per keyword. Preserve uncertainty, corrections, names and relative dates exactly. State the latest correction as the current position. Do not invent causes, dates or decisions. Do not repeat information across sections. Return JSON only.");
    let transcript: Vec<Value> = context
        .segments
        .iter()
        .enumerate()
        .map(|(i, s)| json!({"id":i+1,"text":s.text}))
        .collect();
    (
        system,
        serde_json::to_string(&json!({"keywords":context.keywords,"transcript":transcript}))
            .expect("serializable context"),
    )
}
pub fn schema(count: usize) -> Value {
    json!({"type":"object","properties":{"sections":{"type":"array","maxItems":count.min(3),"items":{"type":"object","properties":{"title":{"type":"string"},"body":{"type":"string"},"sources":{"type":"array","items":{"type":"integer","minimum":1,"maximum":count},"minItems":1,"maxItems":9}},"required":["title","body","sources"],"additionalProperties":false}}},"required":["sections"],"additionalProperties":false})
}
