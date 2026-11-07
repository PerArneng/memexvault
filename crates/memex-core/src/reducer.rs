use crate::{Snapshot, StoreError};
use memex_types::{ChangeSetRow, EdgeRow, Event, Header, Mutation, ObjectId, PageRow, Row, RowKey, SegmentRow, SourceRow, Status, Table};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReduceError {
    UnknownKind(String),
    /// The payload doesn't match the event's kind and version.
    Corrupt {
        seq: u64,
        detail: String,
    },
    AlreadyExists(RowKey),
    NotFound(RowKey),
    /// An edit was based on a version that's no longer current.
    StaleVersion {
        key: RowKey,
        current: u32,
        base: u32,
    },
    Store(StoreError),
}

impl fmt::Display for ReduceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownKind(k) => write!(f, "unknown event kind {k}"),
            Self::Corrupt { seq, detail } => write!(f, "event {seq}: corrupt: {detail}"),
            Self::AlreadyExists(k) => write!(f, "{:?} {} already exists", k.table, k.id),
            Self::NotFound(k) => write!(f, "{:?} {} not found", k.table, k.id),
            Self::StaleVersion { key, current, base } => write!(f, "{:?} {} is at version {current}, not {base}", key.table, key.id),
            Self::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ReduceError {}

impl From<StoreError> for ReduceError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

#[derive(Deserialize)]
struct SourceRecorded {
    id: ObjectId,
    sha256: String,
    media_type: String,
    trust_tier: String,
    name: String,
    size: u64,
    logical_id: ObjectId,
    version_no: u32,
    metadata: serde_json::Value,
}

/// Rows written as one atomic step, outside governance: an upload with its job, or a job's result with its completion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateRecorded {
    pub mutations: Vec<Mutation>,
}

#[derive(Deserialize)]
struct PageCreated {
    id: ObjectId,
    title: String,
    body: String,
}

#[derive(Deserialize)]
struct PageEdited {
    id: ObjectId,
    base_version: u32,
    body: String,
}

#[derive(Deserialize)]
struct SegmentRecorded {
    id: ObjectId,
    source_id: ObjectId,
    language: Option<String>,
    text: String,
}

/// The payload of a `changeset.recorded` event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeSetRecorded {
    pub changeset: ChangeSetRow,
    pub mutations: Vec<Mutation>,
}

#[derive(Deserialize)]
struct ById {
    id: ObjectId,
}

#[derive(Deserialize)]
struct EdgeLinked {
    id: ObjectId,
    from: ObjectId,
    to: ObjectId,
    kind: String,
}

fn parse<T: serde::de::DeserializeOwned>(e: &Event) -> Result<T, ReduceError> {
    serde_json::from_value(e.payload.clone()).map_err(|err| ReduceError::Corrupt {
        seq: e.seq,
        detail: err.to_string(),
    })
}

fn header(id: ObjectId, e: &Event, version: u32) -> Header {
    Header {
        id,
        creator: e.actor.clone(),
        status: Status::Active,
        version,
        acl_id: None,
    }
}

fn must_not_exist<S: Snapshot>(snap: &S, key: &RowKey) -> Result<(), ReduceError> {
    match snap.get(key)? {
        Some(_) => Err(ReduceError::AlreadyExists(key.clone())),
        None => Ok(()),
    }
}

fn must_exist<S: Snapshot>(snap: &S, key: &RowKey) -> Result<Row, ReduceError> {
    snap.get(key)?.ok_or_else(|| ReduceError::NotFound(key.clone()))
}

/// Turns one current-version event into typed row mutations. Pure: it reads through the trait and writes nothing.
pub fn reduce<S: Snapshot>(snap: &S, e: &Event) -> Result<Vec<Mutation>, ReduceError> {
    let key = |table, id: &ObjectId| RowKey { table, id: id.clone() };
    match e.kind.as_str() {
        "source.recorded" => {
            let p: SourceRecorded = parse(e)?;
            must_not_exist(snap, &key(Table::Source, &p.id))?;
            Ok(vec![Mutation::Put(Row::Source(SourceRow {
                header: header(p.id, e, 1),
                sha256: p.sha256,
                media_type: p.media_type,
                trust_tier: p.trust_tier,
                recorded_at_ms: e.time_ms,
                name: p.name,
                size: p.size,
                logical_id: p.logical_id,
                version_no: p.version_no,
                metadata: p.metadata,
            }))])
        }
        "page.created" => {
            let p: PageCreated = parse(e)?;
            must_not_exist(snap, &key(Table::Page, &p.id))?;
            Ok(vec![Mutation::Put(Row::Page(PageRow {
                header: header(p.id, e, 1),
                title: p.title,
                body: p.body,
                updated_at_ms: e.time_ms,
                slug: String::new(),
                redirects: vec![],
            }))])
        }
        "page.edited" => {
            let p: PageEdited = parse(e)?;
            let k = key(Table::Page, &p.id);
            let Row::Page(mut row) = must_exist(snap, &k)? else {
                return Err(ReduceError::NotFound(k));
            };
            if row.header.version != p.base_version {
                return Err(ReduceError::StaleVersion {
                    key: k,
                    current: row.header.version,
                    base: p.base_version,
                });
            }
            row.header.version += 1;
            row.body = p.body;
            row.updated_at_ms = e.time_ms;
            Ok(vec![Mutation::Put(Row::Page(row))])
        }
        "page.deleted" => {
            let p: ById = parse(e)?;
            let k = key(Table::Page, &p.id);
            must_exist(snap, &k)?;
            Ok(vec![Mutation::Delete(k)])
        }
        "edge.linked" => {
            let p: EdgeLinked = parse(e)?;
            must_not_exist(snap, &key(Table::Edge, &p.id))?;
            Ok(vec![Mutation::Put(Row::Edge(EdgeRow {
                header: header(p.id, e, 1),
                from: p.from,
                to: p.to,
                kind: p.kind,
                block_id: None,
            }))])
        }
        "edge.unlinked" => {
            let p: ById = parse(e)?;
            let k = key(Table::Edge, &p.id);
            must_exist(snap, &k)?;
            Ok(vec![Mutation::Delete(k)])
        }
        "segment.recorded" => {
            let p: SegmentRecorded = parse(e)?;
            must_not_exist(snap, &key(Table::Segment, &p.id))?;
            Ok(vec![Mutation::Put(Row::Segment(SegmentRow {
                header: header(p.id, e, 1),
                source_id: p.source_id,
                language: p.language,
                text: p.text,
                rendition_id: None,
                start: 0,
                end: 0,
                heading: None,
                page_from: None,
                page_to: None,
                time: None,
            }))])
        }
        "state.recorded" => {
            let p: StateRecorded = parse(e)?;
            Ok(p.mutations)
        }
        // The governance flow decided everything before writing this event, and it records the result, so replay applies it as it stands.
        "changeset.recorded" => {
            let p: ChangeSetRecorded = parse(e)?;
            let mut out = p.mutations;
            out.push(Mutation::Put(Row::ChangeSet(p.changeset)));
            Ok(out)
        }
        other => Err(ReduceError::UnknownKind(other.to_string())),
    }
}

/// The rows `reduce` will read for this event, so a batch can fetch them all in one call. Pure.
pub fn read_keys(kind: &str, payload: &serde_json::Value) -> Option<RowKey> {
    let table = match kind {
        "source.recorded" => Table::Source,
        "page.created" | "page.edited" | "page.deleted" => Table::Page,
        "edge.linked" | "edge.unlinked" => Table::Edge,
        "segment.recorded" => Table::Segment,
        _ => return None,
    };
    let id = payload.get("id")?.as_str()?;
    Some(RowKey {
        table,
        id: ObjectId(id.into()),
    })
}
