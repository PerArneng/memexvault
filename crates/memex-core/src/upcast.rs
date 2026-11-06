use memex_types::Event;
use serde_json::Value;
use std::fmt;

/// The current schema version of each event kind.
pub const CURRENT_VERSIONS: [(&str, u32); 9] = [
    ("state.recorded", 1),
    ("changeset.recorded", 1),
    ("segment.recorded", 1),
    ("source.recorded", 3),
    ("page.created", 1),
    ("page.edited", 1),
    ("page.deleted", 1),
    ("edge.linked", 1),
    ("edge.unlinked", 1),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpcastError {
    UnknownKind { seq: u64, kind: String },
    UnknownVersion { seq: u64, kind: String, version: u32 },
    Corrupt { seq: u64, detail: String },
}

impl fmt::Display for UpcastError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownKind { seq, kind } => write!(f, "event {seq}: unknown kind {kind}"),
            Self::UnknownVersion { seq, kind, version } => write!(f, "event {seq}: {kind} has unknown version {version}"),
            Self::Corrupt { seq, detail } => write!(f, "event {seq}: corrupt: {detail}"),
        }
    }
}

impl std::error::Error for UpcastError {}

/// Brings an old event to its kind's current version, one step at a time. The log itself is never rewritten.
pub fn upcast(event: &Event) -> Result<Event, UpcastError> {
    let current = CURRENT_VERSIONS
        .iter()
        .find(|(k, _)| *k == event.kind)
        .map(|(_, v)| *v)
        .ok_or_else(|| UpcastError::UnknownKind {
            seq: event.seq,
            kind: event.kind.clone(),
        })?;
    if event.schema_version == 0 || event.schema_version > current {
        return Err(UpcastError::UnknownVersion {
            seq: event.seq,
            kind: event.kind.clone(),
            version: event.schema_version,
        });
    }
    let mut out = event.clone();
    while out.schema_version < current {
        out = step(out)?;
    }
    Ok(out)
}

fn step(mut e: Event) -> Result<Event, UpcastError> {
    match (e.kind.as_str(), e.schema_version) {
        // Version 1 had no media type, and every source then was a PDF.
        ("source.recorded", 1) => {
            let seq = e.seq;
            let Value::Object(m) = &mut e.payload else {
                return Err(UpcastError::Corrupt {
                    seq,
                    detail: "payload isn't an object".into(),
                });
            };
            m.insert("media_type".into(), Value::String("application/pdf".into()));
            e.schema_version = 2;
            Ok(e)
        }
        // Version 2 had no upload details. Each such source is the first version of its own document.
        ("source.recorded", 2) => {
            let seq = e.seq;
            let Value::Object(m) = &mut e.payload else {
                return Err(UpcastError::Corrupt {
                    seq,
                    detail: "payload isn't an object".into(),
                });
            };
            let id = m.get("id").cloned().ok_or_else(|| UpcastError::Corrupt { seq, detail: "no id".into() })?;
            m.insert("name".into(), Value::String(String::new()));
            m.insert("size".into(), Value::from(0));
            m.insert("logical_id".into(), id);
            m.insert("version_no".into(), Value::from(1));
            m.insert("metadata".into(), Value::Object(serde_json::Map::new()));
            e.schema_version = 3;
            Ok(e)
        }
        (kind, version) => Err(UpcastError::UnknownVersion {
            seq: e.seq,
            kind: kind.into(),
            version,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memex_types::VaultId;
    use serde_json::json;

    fn ev(kind: &str, v: u32, payload: Value) -> Event {
        Event {
            seq: 3,
            vault: VaultId("v".into()),
            actor: "a".into(),
            time_ms: 1,
            kind: kind.into(),
            schema_version: v,
            payload,
        }
    }

    #[test]
    fn current_version_is_unchanged() {
        let e = ev("page.deleted", 1, json!({"id": "x"}));
        assert_eq!(upcast(&e).unwrap(), e);
    }

    #[test]
    fn version_one_source_gains_media_type() {
        let up = upcast(&ev("source.recorded", 1, json!({"id": "s"}))).unwrap();
        assert_eq!(up.schema_version, 3);
        assert_eq!(up.payload["logical_id"], "s");
        assert_eq!(up.payload["media_type"], "application/pdf");
    }

    #[test]
    fn future_and_zero_versions_are_refused() {
        assert!(matches!(
            upcast(&ev("page.deleted", 2, json!({}))),
            Err(UpcastError::UnknownVersion { version: 2, .. })
        ));
        assert!(matches!(
            upcast(&ev("page.deleted", 0, json!({}))),
            Err(UpcastError::UnknownVersion { version: 0, .. })
        ));
    }

    #[test]
    fn non_object_version_two_payload_is_corrupt() {
        assert!(matches!(upcast(&ev("source.recorded", 2, json!([]))), Err(UpcastError::Corrupt { .. })));
    }

    #[test]
    fn unknown_kind_is_refused() {
        assert!(matches!(upcast(&ev("nope", 1, json!({}))), Err(UpcastError::UnknownKind { .. })));
    }

    #[test]
    fn non_object_old_payload_is_corrupt() {
        assert!(matches!(upcast(&ev("source.recorded", 1, json!([1]))), Err(UpcastError::Corrupt { .. })));
    }
}
