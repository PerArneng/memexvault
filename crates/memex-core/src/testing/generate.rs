//! Random but valid-shaped events, including ones a reducer must refuse.
use crate::Seed;
use memex_types::EventDraft;
use serde_json::{Value, json};

const TEXTS: [&str; 7] = [
    "",
    "plain text",
    "Göteborg ligger vid Göta älv",
    "Straße über München",
    "El niño está aquí",
    "line\nbreak \"quoted\" \\ back",
    "日本語 and emoji 🚀",
];

fn draft(kind: &str, version: u32, payload: Value) -> EventDraft {
    EventDraft {
        actor: "generator".into(),
        kind: kind.into(),
        schema_version: version,
        payload,
    }
}

/// Draws one event. Ids come from a small range, so edits, deletes, duplicates and missing targets all occur.
pub fn random_draft<R: Seed>(rng: &R, id_space: u64) -> EventDraft {
    let id = |prefix: &str| format!("{prefix}-{:08}", rng.next_u64() % id_space);
    let text = || TEXTS[(rng.next_u64() % TEXTS.len() as u64) as usize];
    match rng.next_u64() % 8 {
        0 => draft(
            "source.recorded",
            2,
            json!({"id": id("s"), "sha256": format!("{:064x}", rng.next_u64()), "media_type": "application/pdf", "trust_tier": "unverified"}),
        ),
        // An old-version event, so replay always exercises the upcaster.
        1 => draft(
            "source.recorded",
            1,
            json!({"id": id("s"), "sha256": format!("{:064x}", rng.next_u64()), "trust_tier": "trusted"}),
        ),
        2 | 3 => draft("page.created", 1, json!({"id": id("p"), "title": text(), "body": text()})),
        4 => draft("page.edited", 1, json!({"id": id("p"), "base_version": rng.next_u64() % 3 + 1, "body": text()})),
        5 => draft("page.deleted", 1, json!({"id": id("p")})),
        6 => draft("edge.linked", 1, json!({"id": id("e"), "from": id("p"), "to": id("s"), "kind": "cites"})),
        _ => draft("edge.unlinked", 1, json!({"id": id("e")})),
    }
}
