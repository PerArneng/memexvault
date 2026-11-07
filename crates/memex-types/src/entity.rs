//! Runtime-defined entity types and their entities. An entity's current state is materialized from its Assertions, whose subject is `entity:<id>`.
use crate::ObjectId;
use crate::rows::{Header, Status};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

/// An Assertion about an entity has this before the entity's id in its subject.
pub const ENTITY_SUBJECT_PREFIX: &str = "entity:";
/// The field name aliases have in the field-value table. Field names can't start with `$`, so it can't collide.
pub const ALIAS_FIELD: &str = "$alias";

pub fn entity_subject(id: &ObjectId) -> String {
    format!("{ENTITY_SUBJECT_PREFIX}{}", id.0)
}

/// The entity an Assertion's subject names, if it names one.
pub fn entity_of_subject(subject: &str) -> Option<ObjectId> {
    subject.strip_prefix(ENTITY_SUBJECT_PREFIX).map(|id| ObjectId(id.into()))
}

/// One version of a type. New versions are added, and old ones never change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityTypeRow {
    pub header: Header,
    pub name: String,
    pub version_no: u32,
    pub schema: Value,
    /// Absent on the built-in first versions of the seed types.
    pub changeset_id: Option<ObjectId>,
    pub created_at_ms: i64,
    /// `seed`, `new`, `additive` or `breaking`.
    pub change: String,
    pub reasons: Vec<String>,
}

/// The id of a type's row. Zero padding makes id order the version order.
pub fn entity_type_row_id(name: &str, version_no: u32) -> ObjectId {
    ObjectId(format!("{name}@{version_no:06}"))
}

/// The type a type row's id belongs to.
pub fn type_of_row_id(id: &ObjectId) -> Option<&str> {
    id.0.split_once('@').map(|(name, _)| name)
}

/// What an exact or range filter can match on one value. A folded text, a number or a time, whichever the field holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldLookup {
    pub text: Option<String>,
    pub number: Option<Number>,
    pub time_ms: Option<i64>,
}

/// Another active claim on the same field. Its value may be the same as the current one, which supports it, or differ, which is a conflict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimedValue {
    pub assertion_id: ObjectId,
    pub value: Value,
}

/// A field's current state: the value chosen, the claim it came from, why it won, and the claims it beat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityField {
    pub field: String,
    pub value: Value,
    pub assertion_id: ObjectId,
    pub why: String,
    pub others: Vec<ClaimedValue>,
    pub lookups: Vec<FieldLookup>,
}

/// A thing of some type. Rows are materialized, so they can always be computed again from the Assertions about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityRow {
    pub header: Header,
    pub type_name: String,
    /// The type version its state was last checked against.
    pub type_version: u32,
    pub aliases: Vec<String>,
    /// The aliases folded, as a filter or a resolve compares them. Core computes these, because a store doesn't know the folding.
    pub alias_keys: Vec<String>,
    pub fields: Vec<EntityField>,
    /// Entities merged into this one, however long ago, whose Assertions count as its own.
    pub merged_from: Vec<ObjectId>,
    /// Set while this entity is merged into another.
    pub redirect_to: Option<ObjectId>,
    pub changeset_id: ObjectId,
    pub updated_at_ms: i64,
}

/// One row of the field-value table.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldValueRow {
    pub field: String,
    pub text: Option<String>,
    pub number: Option<f64>,
    pub time_ms: Option<i64>,
}

impl EntityRow {
    pub fn is_current(&self) -> bool {
        self.header.status == Status::Active && self.redirect_to.is_none()
    }

    /// Every subject whose Assertions make up this entity.
    pub fn subjects(&self) -> Vec<String> {
        std::iter::once(&self.header.id).chain(&self.merged_from).map(entity_subject).collect()
    }

    pub fn field(&self, name: &str) -> Option<&EntityField> {
        self.fields.iter().find(|f| f.field == name)
    }

    /// What goes in the field-value table. An entity merged away has none, so filters only ever find current ones.
    pub fn value_rows(&self) -> Vec<FieldValueRow> {
        if !self.is_current() {
            return vec![];
        }
        let mut out: Vec<FieldValueRow> = self
            .alias_keys
            .iter()
            .map(|a| FieldValueRow {
                field: ALIAS_FIELD.into(),
                text: Some(a.clone()),
                number: None,
                time_ms: None,
            })
            .collect();
        for f in &self.fields {
            for l in &f.lookups {
                out.push(FieldValueRow {
                    field: f.field.clone(),
                    text: l.text.clone(),
                    number: l.number.as_ref().and_then(Number::as_f64),
                    time_ms: l.time_ms,
                });
            }
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldOp {
    Eq,
    Lt,
    Lte,
    Gt,
    Gte,
}

impl FieldOp {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "eq" => Some(Self::Eq),
            "lt" => Some(Self::Lt),
            "lte" => Some(Self::Lte),
            "gt" => Some(Self::Gt),
            "gte" => Some(Self::Gte),
            _ => None,
        }
    }

    pub const fn as_sql(self) -> &'static str {
        match self {
            Self::Eq => "=",
            Self::Lt => "<",
            Self::Lte => "<=",
            Self::Gt => ">",
            Self::Gte => ">=",
        }
    }

    /// Whether `left op right` holds.
    pub fn holds<T: PartialOrd>(self, left: &T, right: &T) -> bool {
        match self {
            Self::Eq => left == right,
            Self::Lt => left < right,
            Self::Lte => left <= right,
            Self::Gt => left > right,
            Self::Gte => left >= right,
        }
    }
}

/// Already parsed for the field's kind. Text is folded.
#[derive(Debug, Clone, PartialEq)]
pub enum FilterValue {
    Text(String),
    Number(f64),
    Time(i64),
}

/// One typed condition. There's no query language: a caller names a field, an operator and a value.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldFilter {
    pub field: String,
    pub op: FieldOp,
    pub value: FilterValue,
}

/// An entity with this exact folded value in one of its fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueMatch {
    pub entity_id: ObjectId,
    pub type_name: String,
    pub field: String,
}

/// Entities of one type sharing a folded value in one field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedValue {
    pub field: String,
    pub text: String,
    pub entity_ids: Vec<ObjectId>,
}

impl FieldLookup {
    /// Whether this value passes a filter of the same kind.
    pub fn passes(&self, op: FieldOp, value: &FilterValue) -> bool {
        match value {
            FilterValue::Text(want) => self.text.as_ref().is_some_and(|t| op.holds(t, want)),
            FilterValue::Number(want) => self.number.as_ref().and_then(Number::as_f64).is_some_and(|n| op.holds(&n, want)),
            FilterValue::Time(want) => self.time_ms.is_some_and(|t| op.holds(&t, want)),
        }
    }
}
