//! The type registry and entities' pure rules: which schemas a type may have, whether a change is additive or breaking, how a field's current value is chosen, and what an entity's search document says.
use crate::governance::PlanError;
use crate::lexical::value_text;
use crate::search::relevance::fold_text;
use crate::{Snapshot, StoreError};
use memex_types::{
    ALIAS_FIELD, AssertionRow, AttrValue, ClaimedValue, DocKind, EntityField, EntityRow, EntityTypeRow, Epistemic, EvidenceRow, FieldFilter, FieldLookup,
    FieldOp, FilterValue, Header, IndexDoc, IndexOp, MovedClaim, Mutation, ObjectId, Op, OpKind, Row, RowKey, SegmentRow, SourceRow, Status, Table, ValueMatch,
    entity_of_subject, entity_subject, entity_type_row_id,
};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

/// Fields in one type, counting nested ones. Each searchable field is a path in the keyword index, and every field is a set of rows in the field-value table.
pub const MAX_FIELDS_PER_TYPE: usize = 200;
/// Types in one vault, which bounds the distinct fields at this times `MAX_FIELDS_PER_TYPE`.
pub const MAX_TYPES: usize = 500;
pub const MAX_SCHEMA_BYTES: usize = 256 * 1024;
pub const MAX_NAME_BYTES: usize = 64;
pub const MAX_ALIASES: usize = 50;
pub const MAX_ALIAS_BYTES: usize = 200;
/// Longer text isn't exact-matched, as a B-tree entry has a size limit and nobody filters by a whole paragraph.
pub const MAX_LOOKUP_TEXT_BYTES: usize = 512;
/// A chain of merges longer than this can only be a corrupt row.
const MAX_REDIRECTS: usize = 64;

/// What a field holds, which decides which column it filters on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Number,
    Boolean,
    Time,
    /// Objects and anything else with no one comparable value.
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDef {
    pub name: String,
    pub kind: FieldKind,
    pub is_array: bool,
    pub searchable: bool,
    pub filterable: bool,
    pub identity: bool,
}

fn scalar_kind(schema: &Map<String, Value>) -> FieldKind {
    let declared = match schema.get("type") {
        Some(Value::String(t)) => Some(t.as_str()),
        Some(Value::Array(types)) => types.iter().filter_map(Value::as_str).find(|t| *t != "null"),
        _ => None,
    };
    match declared {
        Some("string") => match schema.get("format").and_then(Value::as_str) {
            Some("date" | "date-time") => FieldKind::Time,
            _ => FieldKind::Text,
        },
        Some("number" | "integer") => FieldKind::Number,
        Some("boolean") => FieldKind::Boolean,
        _ => FieldKind::Other,
    }
}

fn annotation(schema: &Map<String, Value>, name: &str) -> bool {
    schema.get(name).and_then(Value::as_bool).unwrap_or(false)
}

/// The fields a schema declares, in name order. It doesn't compile the schema, so reads can afford it.
pub fn fields_of(schema: &Value) -> Vec<FieldDef> {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return vec![];
    };
    let mut out: Vec<FieldDef> = properties
        .iter()
        .filter_map(|(name, def)| {
            let def = def.as_object()?;
            let (kind, is_array) = match def.get("items").and_then(Value::as_object) {
                Some(items) if def.get("type").and_then(Value::as_str) == Some("array") => (scalar_kind(items), true),
                _ => (scalar_kind(def), false),
            };
            Some(FieldDef {
                name: name.clone(),
                kind,
                is_array,
                searchable: annotation(def, "searchable"),
                filterable: annotation(def, "filterable"),
                identity: annotation(def, "identity"),
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn name_is_valid(name: &str) -> bool {
    let mut chars = name.chars();
    !name.is_empty()
        && name.len() <= MAX_NAME_BYTES
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Refuses anything that could reach outside the schema itself. It runs before the validator is built, so nothing is ever fetched, and the crate's retrieval features are off as well.
fn check_references(value: &Value, fields: &mut usize) -> Result<(), String> {
    match value {
        Value::Array(items) => items.iter().try_for_each(|v| check_references(v, fields)),
        Value::Object(map) => {
            for (key, v) in map {
                match key.as_str() {
                    "$ref" => match v.as_str() {
                        Some(r) if r.starts_with('#') => {}
                        Some(r) => return Err(format!("the remote $ref '{r}' isn't allowed: a schema has to stand on its own")),
                        None => return Err("a $ref has to be a string".into()),
                    },
                    "$dynamicRef" | "$recursiveRef" | "$id" => return Err(format!("{key} isn't allowed in a type's schema")),
                    "$schema" if v.as_str() != Some("https://json-schema.org/draft/2020-12/schema") => {
                        return Err("the only $schema allowed is draft 2020-12".into());
                    }
                    "properties" => {
                        if let Some(p) = v.as_object() {
                            *fields += p.len();
                        }
                    }
                    _ => {}
                }
                check_references(v, fields)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn compile(schema: &Value) -> Result<jsonschema::Validator, String> {
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .should_validate_formats(true)
        .build(schema)
        .map_err(|e| format!("the schema isn't valid: {e}"))
}

/// Everything a type's schema must be. The rules that matter later: a closed object, so a type's fields are exactly the ones it declares, with safe names and sensible annotations.
pub fn check_schema(name: &str, schema: &Value) -> Result<(), String> {
    if !name_is_valid(name) {
        return Err(format!(
            "a type's name starts with a letter, has only letters, digits and _, and is at most {MAX_NAME_BYTES} bytes"
        ));
    }
    if schema.to_string().len() > MAX_SCHEMA_BYTES {
        return Err(format!("the schema is over the limit of {MAX_SCHEMA_BYTES} bytes"));
    }
    let mut fields = 0;
    check_references(schema, &mut fields)?;
    if fields > MAX_FIELDS_PER_TYPE {
        return Err(format!("the schema has {fields} fields, over the limit of {MAX_FIELDS_PER_TYPE}"));
    }
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return Err("a type's schema must have \"type\": \"object\"".into());
    }
    if schema.get("additionalProperties") != Some(&Value::Bool(false)) {
        return Err("a type's schema must have \"additionalProperties\": false, so its fields are the ones it declares".into());
    }
    let Some(properties) = schema.get("properties").and_then(Value::as_object).filter(|p| !p.is_empty()) else {
        return Err("a type's schema must declare at least one field in \"properties\"".into());
    };
    for (field, def) in properties {
        if !name_is_valid(field) {
            return Err(format!(
                "the field name '{field}' isn't allowed: it must start with a letter and have only letters, digits and _"
            ));
        }
        let Some(def) = def.as_object() else {
            return Err(format!("the field {field} must be described by an object"));
        };
        for a in ["searchable", "filterable", "identity"] {
            if def.get(a).is_some_and(|v| !v.is_boolean()) {
                return Err(format!("the annotation {a} on {field} must be true or false"));
            }
        }
    }
    for f in fields_of(schema) {
        if f.kind == FieldKind::Other && (f.searchable || f.filterable || f.identity) {
            return Err(format!(
                "the field {} is annotated, but it doesn't hold strings, numbers, booleans or dates",
                f.name
            ));
        }
        if f.identity && f.kind != FieldKind::Text {
            return Err(format!("the identity field {} must hold text", f.name));
        }
    }
    compile(schema).map(|_| ())
}

/// How a new version relates to the one before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeChange {
    New,
    Additive,
    Breaking,
}

impl TypeChange {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Additive => "additive",
            Self::Breaking => "breaking",
        }
    }
}

/// Keywords that say nothing about which values are valid.
const NOTES: [&str; 3] = ["title", "description", "examples"];
const ANNOTATIONS: [&str; 3] = ["searchable", "filterable", "identity"];

fn rules_of(def: &Value) -> Value {
    match def.as_object() {
        Some(map) => Value::Object(
            map.iter()
                .filter(|(k, _)| !NOTES.contains(&k.as_str()) && !ANNOTATIONS.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        ),
        None => def.clone(),
    }
}

fn required_of(schema: &Value) -> BTreeSet<String> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).map(String::from).collect())
        .unwrap_or_default()
}

/// Additive means every entity valid before stays valid and is still found the same ways. Anything this can't show to be safe counts as breaking, since being wrong that way only costs a review.
pub fn classify_change(old: &Value, new: &Value) -> (TypeChange, Vec<String>) {
    let (mut breaking, mut additive) = (vec![], vec![]);
    let empty = Map::new();
    let old_fields = old.get("properties").and_then(Value::as_object).unwrap_or(&empty);
    let new_fields = new.get("properties").and_then(Value::as_object).unwrap_or(&empty);
    let (old_required, new_required) = (required_of(old), required_of(new));
    for (name, def) in old_fields {
        let Some(next) = new_fields.get(name) else {
            breaking.push(format!("the field {name} is removed"));
            continue;
        };
        if rules_of(def) != rules_of(next) {
            breaking.push(format!("the rules for the field {name} change"));
        }
        for a in ANNOTATIONS {
            match (
                def.get(a).and_then(Value::as_bool).unwrap_or(false),
                next.get(a).and_then(Value::as_bool).unwrap_or(false),
            ) {
                (true, false) => breaking.push(format!("the field {name} is no longer {a}")),
                (false, true) => additive.push(format!("the field {name} becomes {a}")),
                _ => {}
            }
        }
    }
    for name in new_fields.keys().filter(|n| !old_fields.contains_key(*n)) {
        if new_required.contains(name) {
            breaking.push(format!("the new field {name} is required, and existing entities don't have it"));
        } else {
            additive.push(format!("the optional field {name} is added"));
        }
    }
    for name in new_required.difference(&old_required).filter(|n| old_fields.contains_key(*n)) {
        breaking.push(format!("the field {name} becomes required"));
    }
    for name in old_required.difference(&new_required).filter(|n| new_fields.contains_key(*n)) {
        additive.push(format!("the field {name} is no longer required"));
    }
    let outer = |s: &Value| -> Map<String, Value> {
        s.as_object()
            .map(|m| {
                m.iter()
                    .filter(|(k, _)| !NOTES.contains(&k.as_str()) && !matches!(k.as_str(), "properties" | "required"))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default()
    };
    if outer(old) != outer(new) {
        breaking.push("a rule on the whole object changes".into());
    }
    if breaking.is_empty() {
        if additive.is_empty() {
            additive.push("only descriptions change".into());
        }
        (TypeChange::Additive, additive)
    } else {
        (TypeChange::Breaking, breaking)
    }
}

fn seed_row(name: &str, schema: Value) -> EntityTypeRow {
    EntityTypeRow {
        header: Header {
            id: entity_type_row_id(name, 1),
            creator: "system".into(),
            status: Status::Active,
            version: 1,
            acl_id: None,
        },
        name: name.into(),
        version_no: 1,
        schema,
        changeset_id: None,
        created_at_ms: 0,
        change: "seed".into(),
        reasons: vec![],
    }
}

/// The types every vault starts with, as version 1. They live in the code rather than each vault's log, so a vault's first event is still the first thing someone did in it. Never edit one: a change is a new version, proposed like any other.
pub fn seed_types() -> &'static [EntityTypeRow] {
    static SEEDS: OnceLock<Vec<EntityTypeRow>> = OnceLock::new();
    SEEDS.get_or_init(|| {
        vec![
            seed_row(
                "Decision",
                json!({"type": "object", "additionalProperties": false, "required": ["title"], "properties": {
                    "title": {"type": "string", "searchable": true, "identity": true, "filterable": true},
                    "status": {"type": "string", "filterable": true},
                    "decided_on": {"type": "string", "format": "date", "filterable": true},
                    "rationale": {"type": "string", "searchable": true}
                }}),
            ),
            seed_row(
                "Organization",
                json!({"type": "object", "additionalProperties": false, "required": ["name"], "properties": {
                    "name": {"type": "string", "searchable": true, "identity": true, "filterable": true},
                    "domain": {"type": "string", "identity": true, "filterable": true},
                    "country": {"type": "string", "filterable": true},
                    "description": {"type": "string", "searchable": true}
                }}),
            ),
            seed_row(
                "Person",
                json!({"type": "object", "additionalProperties": false, "required": ["name"], "properties": {
                    "name": {"type": "string", "searchable": true, "identity": true, "filterable": true},
                    "email": {"type": "string", "identity": true, "filterable": true},
                    "title": {"type": "string", "searchable": true},
                    "organization": {"type": "string", "searchable": true, "filterable": true}
                }}),
            ),
            seed_row(
                "Product",
                json!({"type": "object", "additionalProperties": false, "required": ["name"], "properties": {
                    "name": {"type": "string", "searchable": true, "identity": true, "filterable": true},
                    "sku": {"type": "string", "identity": true, "filterable": true},
                    "price": {"type": "number", "filterable": true},
                    "released": {"type": "string", "format": "date", "filterable": true},
                    "description": {"type": "string", "searchable": true}
                }}),
            ),
            seed_row(
                "Project",
                json!({"type": "object", "additionalProperties": false, "required": ["name"], "properties": {
                    "name": {"type": "string", "searchable": true, "identity": true, "filterable": true},
                    "status": {"type": "string", "filterable": true},
                    "started": {"type": "string", "format": "date", "filterable": true},
                    "description": {"type": "string", "searchable": true}
                }}),
            ),
        ]
    })
}

fn seed_of(name: &str) -> Option<&'static EntityTypeRow> {
    seed_types().iter().find(|t| t.name == name)
}

/// The newest version of each of these types, seeds included. One store call.
pub fn latest_types<S: Snapshot>(snap: &S, names: &[String]) -> Result<HashMap<String, EntityTypeRow>, StoreError> {
    let mut out: HashMap<String, EntityTypeRow> = names.iter().filter_map(|n| seed_of(n).map(|t| (n.clone(), t.clone()))).collect();
    if names.is_empty() {
        return Ok(out);
    }
    for row in snap.entity_type_versions(names)? {
        let newer = out.get(&row.name).is_none_or(|t| t.version_no < row.version_no);
        if newer {
            out.insert(row.name.clone(), row);
        }
    }
    Ok(out)
}

/// All of a type's versions, oldest first.
pub fn type_history<S: Snapshot>(snap: &S, name: &str) -> Result<Vec<EntityTypeRow>, StoreError> {
    let mut out: Vec<EntityTypeRow> = seed_of(name).cloned().into_iter().collect();
    out.extend(snap.entity_type_versions(&[name.to_string()])?);
    Ok(out)
}

/// One page of types by name, each at its newest version, seeds included.
pub fn list_types<S: Snapshot>(snap: &S, after: Option<&str>, limit: usize) -> Result<Vec<EntityTypeRow>, StoreError> {
    let stored = snap.latest_entity_types(after, limit)?;
    let mut by_name: BTreeMap<String, EntityTypeRow> = seed_types()
        .iter()
        .filter(|t| after.is_none_or(|a| t.name.as_str() > a))
        .map(|t| (t.name.clone(), t.clone()))
        .collect();
    let full = stored.len() == limit;
    let last_stored = stored.last().map(|t| t.name.clone());
    for row in stored {
        by_name.insert(row.name.clone(), row);
    }
    // Past the last stored name on a full page, a seed might come before a stored type on the next page, so it waits for that page.
    let mut out: Vec<EntityTypeRow> = by_name
        .into_values()
        .filter(|t| !full || last_stored.as_ref().is_none_or(|l| &t.name <= l))
        .collect();
    out.truncate(limit);
    Ok(out)
}

/// What submitting this schema would do, as far as the schema goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypePreview {
    /// Why it would be refused. None means it's acceptable.
    pub problem: Option<String>,
    pub change: TypeChange,
    pub reasons: Vec<String>,
    pub current_version: Option<u32>,
    pub next_version: u32,
}

/// The one classification: the preview route shows it, and a submitted change is decided by it, so they can't disagree.
pub fn preview_change<S: Snapshot>(snap: &S, name: &str, schema: &Value) -> Result<TypePreview, StoreError> {
    let current = latest_types(snap, &[name.to_string()])?.remove(name);
    let mut problem = check_schema(name, schema).err();
    let (change, reasons, current_version, next_version) = match &current {
        None => {
            if problem.is_none() && count_types(snap)? >= MAX_TYPES {
                problem = Some(format!("this vault has reached the limit of {MAX_TYPES} types"));
            }
            (TypeChange::New, vec!["a new type".to_string()], None, 1)
        }
        Some(c) => {
            let (change, reasons) = if problem.is_none() {
                classify_change(&c.schema, schema)
            } else {
                (TypeChange::Breaking, vec![])
            };
            if problem.is_none() && c.schema == *schema {
                problem = Some(format!("the schema is the same as version {}", c.version_no));
            }
            (change, reasons, Some(c.version_no), c.version_no + 1)
        }
    };
    Ok(TypePreview {
        problem,
        change,
        reasons,
        current_version,
        next_version,
    })
}

fn count_types<S: Snapshot>(snap: &S) -> Result<usize, StoreError> {
    let stored = snap.latest_entity_types(None, MAX_TYPES + 1)?;
    let extra_seeds = seed_types().iter().filter(|s| !stored.iter().any(|t| t.name == s.name)).count();
    Ok(stored.len() + extra_seeds)
}

/// The risk table's kind for a schema change.
pub fn schema_op_kind<S: Snapshot>(snap: &S, name: &str, schema: &Value) -> Result<OpKind, StoreError> {
    let preview = preview_change(snap, name, schema)?;
    Ok(match preview.change {
        TypeChange::New | TypeChange::Additive => OpKind::AdditiveSchemaChange,
        TypeChange::Breaking => OpKind::BreakingSchemaChange,
    })
}

/// Whether a ChangeSet has anything to do with entities, so other proposals pay nothing for these checks.
pub fn touches_entities(ops: &[Op]) -> bool {
    ops.iter().any(|op| match op {
        Op::DefineEntityType { .. } | Op::CreateEntity { .. } | Op::SetEntityAliases { .. } | Op::MergeEntities { .. } | Op::SplitEntity { .. } => true,
        Op::AddAssertion { subject, .. } => entity_of_subject(subject).is_some(),
        // The claim it replaces may be about an entity, which only a read can tell.
        Op::SupersedeAssertion { .. } | Op::RetractAssertion { .. } => true,
        _ => false,
    })
}

/// Parses a filter for what the field holds. Text compares folded, so `acme` finds `Acme`.
pub fn parse_filter(defs: &[FieldDef], field: &str, op: &str, raw: &str) -> Result<FieldFilter, String> {
    let def = defs.iter().find(|d| d.name == field).ok_or_else(|| format!("the type has no field {field}"))?;
    if !def.filterable {
        return Err(format!("the field {field} isn't filterable"));
    }
    let op = FieldOp::parse(op).ok_or_else(|| format!("unknown operator '{op}': use eq, lt, lte, gt or gte"))?;
    let value = match def.kind {
        FieldKind::Text | FieldKind::Boolean => {
            if op != FieldOp::Eq {
                return Err(format!("the field {field} can only be compared with eq"));
            }
            if def.kind == FieldKind::Boolean && !matches!(raw, "true" | "false") {
                return Err(format!("the field {field} holds true or false, not '{raw}'"));
            }
            FilterValue::Text(fold_key(raw))
        }
        FieldKind::Number => match raw.parse::<f64>() {
            Ok(n) if n.is_finite() => FilterValue::Number(n),
            _ => return Err(format!("the field {field} holds numbers, not '{raw}'")),
        },
        FieldKind::Time => FilterValue::Time(parse_time(raw).ok_or_else(|| format!("the field {field} holds dates, not '{raw}'"))?),
        FieldKind::Other => return Err(format!("the field {field} can't be compared")),
    };
    Ok(FieldFilter {
        field: field.to_string(),
        op,
        value,
    })
}

/// Case, diacritics and runs of space don't count in an exact match.
pub fn fold_key(text: &str) -> String {
    fold_text(text).split_whitespace().collect::<Vec<_>>().join(" ")
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn date_ms(text: &str) -> Option<i64> {
    let b = text.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let (y, m, d) = (
        text[..4].parse::<i64>().ok()?,
        text[5..7].parse::<i64>().ok()?,
        text[8..10].parse::<i64>().ok()?,
    );
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if !(1..=12).contains(&m) || d < 1 || d > days[(m - 1) as usize] {
        return None;
    }
    Some(days_from_civil(y, m, d) * 86_400_000)
}

/// A date or an RFC 3339 time, in milliseconds since 1970. These are the two forms the `date` and `date-time` formats allow.
pub fn parse_time(text: &str) -> Option<i64> {
    if text.len() == 10 {
        return date_ms(text);
    }
    if !text.is_ascii() || text.len() < 20 || !matches!(text.as_bytes()[10], b'T' | b't') {
        return None;
    }
    let date = date_ms(&text[..10])?;
    let (hour, minute, second) = (
        text[11..13].parse::<i64>().ok()?,
        text[14..16].parse::<i64>().ok()?,
        text[17..19].parse::<i64>().ok()?,
    );
    if text.as_bytes()[13] != b':' || text.as_bytes()[16] != b':' || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let mut rest = &text[19..];
    let mut millis = 0;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.chars().take_while(char::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        let shown = format!("{:0<3}", &fraction[..digits.min(3)]);
        millis = shown.parse::<i64>().ok()?;
        rest = &fraction[digits..];
    }
    let offset = match rest {
        "Z" | "z" => 0,
        _ => {
            let b = rest.as_bytes();
            if b.len() != 6 || !matches!(b[0], b'+' | b'-') || b[3] != b':' {
                return None;
            }
            let minutes = rest[1..3].parse::<i64>().ok()? * 60 + rest[4..6].parse::<i64>().ok()?;
            if b[0] == b'-' { -minutes } else { minutes }
        }
    };
    Some(date + (hour * 3600 + minute * 60 + second) * 1000 + millis - offset * 60_000)
}

fn scalar_lookup(value: &Value, kind: FieldKind) -> Option<FieldLookup> {
    let (mut text, mut number, mut time_ms) = (None, None, None);
    match (kind, value) {
        (FieldKind::Text, Value::String(s)) if s.len() <= MAX_LOOKUP_TEXT_BYTES => text = Some(fold_key(s)),
        (FieldKind::Number, Value::Number(n)) => number = Some(n.clone()),
        (FieldKind::Boolean, Value::Bool(b)) => text = Some(b.to_string()),
        (FieldKind::Time, Value::String(s)) => time_ms = parse_time(s),
        _ => return None,
    }
    if text.is_none() && number.is_none() && time_ms.is_none() {
        return None;
    }
    Some(FieldLookup { text, number, time_ms })
}

fn lookups_of(value: &Value, def: &FieldDef) -> Vec<FieldLookup> {
    let mut out: Vec<FieldLookup> = vec![];
    let items: Vec<&Value> = match value {
        Value::Array(items) if def.is_array => items.iter().collect(),
        v => vec![v],
    };
    for item in items {
        if let Some(l) = scalar_lookup(item, def.kind)
            && !out.contains(&l)
        {
            out.push(l);
        }
    }
    out
}

fn epistemic_rank(e: Epistemic) -> u8 {
    match e {
        Epistemic::Verified => 3,
        Epistemic::Asserted => 2,
        Epistemic::Inferred => 1,
        Epistemic::Disputed => 0,
    }
}

fn epistemic_name(e: Epistemic) -> &'static str {
    match e {
        Epistemic::Verified => "verified",
        Epistemic::Asserted => "asserted",
        Epistemic::Inferred => "inferred",
        Epistemic::Disputed => "disputed",
    }
}

/// Best first. A claim's own confidence plays no part, since a model reports its own, and trust in a source can change later without any change here. ChangeSet ids start with the time, so a higher one is newer.
fn preference(a: &AssertionRow, b: &AssertionRow) -> std::cmp::Ordering {
    epistemic_rank(b.epistemic)
        .cmp(&epistemic_rank(a.epistemic))
        .then_with(|| a.machine.cmp(&b.machine))
        .then_with(|| b.changeset_id.cmp(&a.changeset_id))
        .then_with(|| b.header.id.cmp(&a.header.id))
}

fn why_chosen(chosen: &AssertionRow, others: &[&AssertionRow]) -> String {
    let Some(rival) = others.iter().find(|o| o.value != chosen.value) else {
        return if others.is_empty() {
            "it's the only claim on this field".into()
        } else {
            "all the claims on this field agree".into()
        };
    };
    if chosen.epistemic != rival.epistemic {
        format!("{} beats {}", epistemic_name(chosen.epistemic), epistemic_name(rival.epistemic))
    } else if chosen.machine != rival.machine {
        "a person's claim beats a machine's".into()
    } else {
        "both claims have the same standing, and this one is newer".into()
    }
}

/// Sets the entity's fields from all the active claims about it. Pure, so the same claims always give the same state. A claim on a field its type no longer has stays in the store, and isn't part of the current state.
pub fn materialize(row: &mut EntityRow, schema: &Value, claims: &[&AssertionRow]) {
    let defs = fields_of(schema);
    let mut by_field: BTreeMap<&str, Vec<&AssertionRow>> = BTreeMap::new();
    for c in claims {
        by_field.entry(c.predicate.as_str()).or_default().push(c);
    }
    row.fields = defs
        .iter()
        .filter_map(|def| {
            let mut found = by_field.remove(def.name.as_str())?;
            found.sort_by(|a, b| preference(a, b));
            let (chosen, rest) = found.split_first()?;
            Some(EntityField {
                field: def.name.clone(),
                value: chosen.value.clone(),
                assertion_id: chosen.header.id.clone(),
                why: why_chosen(chosen, rest),
                others: rest
                    .iter()
                    .map(|o| ClaimedValue {
                        assertion_id: o.header.id.clone(),
                        value: o.value.clone(),
                    })
                    .collect(),
                lookups: lookups_of(&chosen.value, def),
            })
        })
        .collect();
    row.alias_keys = row.aliases.iter().map(|a| fold_key(a)).collect::<BTreeSet<_>>().into_iter().collect();
}

/// The current state as a plain object, which is what the schema describes.
pub fn attributes_of(row: &EntityRow) -> Value {
    Value::Object(row.fields.iter().map(|f| (f.field.clone(), f.value.clone())).collect())
}

fn check_aliases(aliases: &[String]) -> Result<(), PlanError> {
    if aliases.len() > MAX_ALIASES {
        return Err(PlanError::Invalid(format!("an entity can have at most {MAX_ALIASES} aliases")));
    }
    if let Some(bad) = aliases.iter().find(|a| a.trim().is_empty() || a.len() > MAX_ALIAS_BYTES) {
        return Err(PlanError::Invalid(format!("the alias '{bad}' must have 1 to {MAX_ALIAS_BYTES} bytes")));
    }
    Ok(())
}

fn key(table: Table, id: &ObjectId) -> RowKey {
    RowKey { table, id: id.clone() }
}

fn stale(row: &EntityRow, base: u32) -> PlanError {
    PlanError::Stale {
        key: key(Table::Entity, &row.header.id),
        current: Box::new(Row::Entity(row.clone())),
        base,
    }
}

fn missing(id: &ObjectId) -> PlanError {
    PlanError::Invalid(format!("entity {id} doesn't exist"))
}

/// What's known about entities while a ChangeSet is planned: rows as this ChangeSet has left them so far.
struct Work<'a, S: Snapshot> {
    snap: &'a S,
    changeset: &'a ObjectId,
    creator: &'a str,
    now_ms: i64,
    entities: BTreeMap<ObjectId, EntityRow>,
    types: HashMap<String, EntityTypeRow>,
    new_entities: BTreeSet<ObjectId>,
    /// Rows to write.
    changed: BTreeSet<ObjectId>,
    /// Rows whose state has to be worked out again from their claims.
    to_materialize: BTreeSet<ObjectId>,
    /// A claim is being retired, which may leave its entity without a required field. That's the truth of it, and a type's schema mustn't keep a wrong claim in place.
    retiring: bool,
}

impl<S: Snapshot> Work<'_, S> {
    fn entity(&mut self, id: &ObjectId) -> Result<Option<&EntityRow>, PlanError> {
        if !self.entities.contains_key(id)
            && let Some(Row::Entity(e)) = self.snap.get(&key(Table::Entity, id))?
        {
            self.entities.insert(id.clone(), e);
        }
        Ok(self.entities.get(id))
    }

    /// A row, checked to be current and at the version the proposer saw.
    fn at_version(&mut self, id: &ObjectId, base: u32) -> Result<EntityRow, PlanError> {
        let row = self.entity(id)?.cloned().ok_or_else(|| missing(id))?;
        if row.header.version != base {
            return Err(stale(&row, base));
        }
        if let Some(to) = &row.redirect_to {
            return Err(PlanError::Invalid(format!("entity {id} is merged into {to}")));
        }
        Ok(row)
    }

    /// Where an id leads now.
    fn current_id(&mut self, id: &ObjectId) -> Result<ObjectId, PlanError> {
        let mut at = id.clone();
        for _ in 0..MAX_REDIRECTS {
            let row = self.entity(&at)?.ok_or_else(|| missing(&at))?;
            match row.redirect_to.clone() {
                Some(next) => at = next,
                None => return Ok(at),
            }
        }
        Err(PlanError::Invalid(format!("entity {id} redirects too many times")))
    }

    fn type_of(&mut self, name: &str) -> Result<EntityTypeRow, PlanError> {
        if !self.types.contains_key(name) {
            let found = latest_types(self.snap, &[name.to_string()])?;
            let row = found
                .get(name)
                .cloned()
                .ok_or_else(|| PlanError::Invalid(format!("there's no entity type {name}")))?;
            self.types.insert(name.to_string(), row);
        }
        Ok(self.types[name].clone())
    }

    fn store(&mut self, mut row: EntityRow) {
        let id = row.header.id.clone();
        if !self.new_entities.contains(&id) && self.changed.insert(id.clone()) {
            row.header.version += 1;
        }
        row.changeset_id = self.changeset.clone();
        row.updated_at_ms = self.now_ms;
        self.entities.insert(id, row);
    }

    fn new_entity(&mut self, id: &ObjectId, type_row: &EntityTypeRow, aliases: Vec<String>) -> Result<(), PlanError> {
        if self.entity(id)?.is_some() {
            return Err(PlanError::Exists(key(Table::Entity, id)));
        }
        check_aliases(&aliases)?;
        self.new_entities.insert(id.clone());
        self.changed.insert(id.clone());
        self.to_materialize.insert(id.clone());
        self.entities.insert(
            id.clone(),
            EntityRow {
                header: Header {
                    id: id.clone(),
                    creator: self.creator.into(),
                    status: Status::Active,
                    version: 1,
                    acl_id: None,
                },
                type_name: type_row.name.clone(),
                type_version: type_row.version_no,
                aliases,
                alias_keys: vec![],
                fields: vec![],
                merged_from: vec![],
                redirect_to: None,
                changeset_id: self.changeset.clone(),
                updated_at_ms: self.now_ms,
            },
        );
        Ok(())
    }

    fn define_type(&mut self, name: &str, base_version: Option<u32>, schema: &Value) -> Result<Mutation, PlanError> {
        let preview = preview_change(self.snap, name, schema)?;
        if let Some(problem) = preview.problem {
            return Err(PlanError::Invalid(problem));
        }
        if preview.current_version != base_version {
            let current = latest_types(self.snap, &[name.to_string()])?.remove(name);
            return match (current, base_version) {
                (Some(c), Some(base)) => Err(PlanError::Stale {
                    key: key(Table::EntityType, &c.header.id),
                    current: Box::new(Row::EntityType(c)),
                    base,
                }),
                (Some(c), None) => Err(PlanError::Exists(key(Table::EntityType, &c.header.id))),
                (None, _) => Err(PlanError::Invalid(format!("there's no entity type {name} to build on"))),
            };
        }
        let row = EntityTypeRow {
            header: Header {
                id: entity_type_row_id(name, preview.next_version),
                creator: self.creator.into(),
                status: Status::Active,
                version: 1,
                acl_id: None,
            },
            name: name.into(),
            version_no: preview.next_version,
            schema: schema.clone(),
            changeset_id: Some(self.changeset.clone()),
            created_at_ms: self.now_ms,
            change: preview.change.as_str().into(),
            reasons: preview.reasons,
        };
        self.types.insert(name.to_string(), row.clone());
        Ok(Mutation::Put(Row::EntityType(row)))
    }

    fn merge(&mut self, source: &ObjectId, source_version: u32, target: &ObjectId, target_version: u32) -> Result<(), PlanError> {
        if source == target {
            return Err(PlanError::Invalid("an entity can't be merged into itself".into()));
        }
        let mut from = self.at_version(source, source_version)?;
        let mut into = self.at_version(target, target_version)?;
        if from.type_name != into.type_name {
            return Err(PlanError::Invalid(format!("a {} can't be merged into a {}", from.type_name, into.type_name)));
        }
        into.merged_from.push(source.clone());
        into.merged_from.extend(from.merged_from.iter().cloned());
        for alias in &from.aliases {
            if !into.aliases.contains(alias) {
                into.aliases.push(alias.clone());
            }
        }
        check_aliases(&into.aliases)?;
        from.redirect_to = Some(target.clone());
        from.header.status = Status::Superseded;
        self.store(from);
        self.store(into);
        self.to_materialize.insert(target.clone());
        Ok(())
    }

    fn split(
        &mut self,
        op: (&ObjectId, u32, &ObjectId),
        moved: &[MovedClaim],
        aliases: &[String],
        new_id: &mut dyn FnMut() -> ObjectId,
    ) -> Result<Vec<Mutation>, PlanError> {
        let (id, base_version, new_entity_id) = op;
        let mut row = self.at_version(id, base_version)?;
        if moved.is_empty() && aliases.is_empty() {
            return Err(PlanError::Invalid("a split has to move at least one claim or alias".into()));
        }
        if let Some(alias) = aliases.iter().find(|a| !row.aliases.contains(a)) {
            return Err(PlanError::Invalid(format!("entity {id} has no alias '{alias}'")));
        }
        let type_row = self.type_of(&row.type_name)?;
        self.new_entity(new_entity_id, &type_row, aliases.to_vec())?;
        row.aliases.retain(|a| !aliases.contains(a));
        let subjects = row.subjects();
        self.store(row);
        self.to_materialize.insert(id.clone());
        let keys: Vec<RowKey> = moved.iter().map(|m| key(Table::Assertion, &m.assertion)).collect();
        let found = if keys.is_empty() { vec![] } else { self.snap.get_many(&keys)? };
        let ids: Vec<ObjectId> = moved.iter().map(|m| m.assertion.clone()).collect();
        let evidence = if ids.is_empty() { vec![] } else { self.snap.evidence_of(&ids)? };
        let mut out = vec![];
        for (m, row) in moved.iter().zip(found) {
            let Some(Row::Assertion(mut old)) = row else {
                return Err(PlanError::Invalid(format!("claim {} doesn't exist", m.assertion)));
            };
            if old.header.status != Status::Active || !subjects.contains(&old.subject) {
                return Err(PlanError::Invalid(format!("claim {} isn't an active claim about entity {id}", m.assertion)));
            }
            if self.snap.get(&key(Table::Assertion, &m.new_id))?.is_some() {
                return Err(PlanError::Exists(key(Table::Assertion, &m.new_id)));
            }
            let subject = entity_subject(new_entity_id);
            let mut copy = old.clone();
            copy.header = Header {
                id: m.new_id.clone(),
                creator: self.creator.into(),
                status: Status::Active,
                version: 1,
                acl_id: None,
            };
            copy.canonical_hash = crate::governance::canonical_hash(&subject, &copy.predicate, &copy.value);
            copy.subject = subject;
            copy.changeset_id = self.changeset.clone();
            old.header.status = Status::Superseded;
            old.header.version += 1;
            old.superseded_by = Some(m.new_id.clone());
            out.push(Mutation::Put(Row::Assertion(old)));
            out.push(Mutation::Put(Row::Assertion(copy)));
            for e in evidence.iter().filter(|e| e.assertion_id == m.assertion) {
                let mut e: EvidenceRow = e.clone();
                e.header.id = new_id();
                e.header.creator = self.creator.into();
                e.header.version = 1;
                e.assertion_id = m.new_id.clone();
                out.push(Mutation::Put(Row::Evidence(e)));
            }
        }
        Ok(out)
    }

    /// Checks the new claims against their types, then works out and validates each touched entity's state.
    fn finish(&mut self, claims: &[&Mutation]) -> Result<Vec<Mutation>, PlanError> {
        let mut new_claims: Vec<&AssertionRow> = vec![];
        let mut gone: BTreeSet<&ObjectId> = BTreeSet::new();
        for m in claims {
            let Mutation::Put(Row::Assertion(a)) = m else { continue };
            if a.header.status != Status::Active {
                gone.insert(&a.header.id);
                // Its entity loses it, whatever the claim replacing it is about.
                if let Some(id) = entity_of_subject(&a.subject) {
                    let at = self.current_id(&id)?;
                    self.to_materialize.insert(at);
                }
                continue;
            }
            new_claims.push(a);
            if let Some(id) = entity_of_subject(&a.subject) {
                let at = self.current_id(&id)?;
                self.to_materialize.insert(at);
            }
        }
        let touched: Vec<ObjectId> = self.to_materialize.iter().cloned().collect();
        let mut subjects: BTreeSet<String> = BTreeSet::new();
        for id in &touched {
            let row = self.entity(id)?.ok_or_else(|| missing(id))?;
            subjects.extend(row.subjects());
        }
        let list: Vec<String> = subjects.into_iter().collect();
        let stored = if list.is_empty() { vec![] } else { self.snap.assertions_about(&list)? };
        let mut out = vec![];
        for id in &touched {
            let mut row = self.entity(id)?.cloned().ok_or_else(|| missing(id))?;
            let type_row = self.type_of(&row.type_name)?;
            let row_subjects = row.subjects();
            let about = |a: &&AssertionRow| row_subjects.contains(&a.subject);
            let mine: Vec<&AssertionRow> = stored
                .iter()
                .filter(about)
                .filter(|a| !gone.contains(&a.header.id) && !new_claims.iter().any(|n| n.header.id == a.header.id))
                .chain(new_claims.iter().copied().filter(about))
                .collect();
            check_claims(&row, &type_row, new_claims.iter().copied().filter(about))?;
            row.type_version = type_row.version_no;
            materialize(&mut row, &type_row.schema, &mine);
            let attributes = attributes_of(&row);
            if !self.retiring
                && let Some(problem) = compile(&type_row.schema).map_err(PlanError::Invalid)?.iter_errors(&attributes).next()
            {
                return Err(PlanError::Invalid(format!("entity {id} isn't a valid {}: {problem}", type_row.name)));
            }
            self.store(row);
        }
        for id in std::mem::take(&mut self.changed) {
            if let Some(row) = self.entities.get(&id) {
                out.push(Mutation::Put(Row::Entity(row.clone())));
            }
        }
        Ok(out)
    }
}

/// Each new claim's value must be valid for its field, including one that won't become the current value.
fn check_claims<'a>(row: &EntityRow, type_row: &EntityTypeRow, claims: impl Iterator<Item = &'a AssertionRow>) -> Result<(), PlanError> {
    let mut partial = type_row.schema.clone();
    if let Some(map) = partial.as_object_mut() {
        map.remove("required");
    }
    let mut validator = None;
    for c in claims {
        if validator.is_none() {
            validator = Some(compile(&partial).map_err(PlanError::Invalid)?);
        }
        let single = json!({c.predicate.clone(): c.value.clone()});
        if let Some(v) = &validator
            && let Some(problem) = v.iter_errors(&single).next()
        {
            return Err(PlanError::Invalid(format!(
                "the value of {} isn't valid for a {}: {problem}",
                c.predicate, row.type_name
            )));
        }
    }
    Ok(())
}

/// The entity rows and any extra claims a ChangeSet's ops lead to. `earlier` holds the mutations already planned, whose claims count. Base versions are checked again here, at commit.
pub fn plan_entities<S: Snapshot>(
    snap: &S,
    ops: &[Op],
    earlier: &[Mutation],
    creator: &str,
    changeset: &ObjectId,
    now_ms: i64,
    new_id: &mut dyn FnMut() -> ObjectId,
) -> Result<Vec<Mutation>, PlanError> {
    if !touches_entities(ops) {
        return Ok(vec![]);
    }
    let mut work = Work {
        snap,
        changeset,
        creator,
        now_ms,
        entities: BTreeMap::new(),
        types: HashMap::new(),
        new_entities: BTreeSet::new(),
        changed: BTreeSet::new(),
        to_materialize: BTreeSet::new(),
        retiring: ops.iter().any(|op| matches!(op, Op::RetractAssertion { .. })),
    };
    let mut out = vec![];
    for op in ops {
        match op {
            Op::DefineEntityType { name, base_version, schema } => out.push(work.define_type(name, *base_version, schema)?),
            Op::CreateEntity { id, type_name, aliases } => {
                let type_row = work.type_of(type_name)?;
                work.new_entity(id, &type_row, aliases.clone())?;
            }
            Op::SetEntityAliases { id, base_version, aliases } => {
                check_aliases(aliases)?;
                let mut row = work.at_version(id, *base_version)?;
                row.aliases.clone_from(aliases);
                work.store(row);
                work.to_materialize.insert(id.clone());
            }
            Op::MergeEntities {
                source,
                source_version,
                target,
                target_version,
            } => work.merge(source, *source_version, target, *target_version)?,
            Op::SplitEntity {
                id,
                base_version,
                new_id: new_entity,
                moved,
                aliases,
            } => out.extend(work.split((id, *base_version, new_entity), moved, aliases, new_id)?),
            _ => {}
        }
    }
    let claims: Vec<&Mutation> = earlier.iter().chain(out.iter()).collect();
    let rows = work.finish(&claims)?;
    out.extend(rows);
    Ok(out)
}

/// Why this ChangeSet needs a person's approval whoever holds admin scope: every always-review kind, and any type or raised source trust an agent proposed.
pub fn needs_a_person(kinds: &[OpKind], ops: &[Op], proposer_is_machine: bool) -> bool {
    kinds.iter().any(|k| crate::governance::ALWAYS_REVIEW.contains(k))
        || (proposer_is_machine && (ops.iter().any(|op| matches!(op, Op::DefineEntityType { .. })) || kinds.contains(&OpKind::RaiseSourceTrust)))
}

/// Entities whose identity field or alias is exactly this folded text. Matches on other fields don't count, as a shared status or country says nothing about who it is. One store call, and one more only on a match.
pub fn identity_matches<S: Snapshot>(snap: &S, folded: &str, limit: usize) -> Result<Vec<ValueMatch>, StoreError> {
    let found = snap.entities_with_value(folded, limit)?;
    if found.is_empty() {
        return Ok(found);
    }
    let names: Vec<String> = found.iter().map(|m| m.type_name.clone()).collect::<BTreeSet<_>>().into_iter().collect();
    let types = latest_types(snap, &names)?;
    Ok(found
        .into_iter()
        .filter(|m| {
            m.field == ALIAS_FIELD
                || types
                    .get(&m.type_name)
                    .is_some_and(|t| fields_of(&t.schema).iter().any(|d| d.identity && d.name == m.field))
        })
        .collect())
}

/// What an entity's card says first: its name, or failing that an alias, or its id.
fn label_of(row: &EntityRow, defs: &[FieldDef]) -> String {
    defs.iter()
        .filter(|d| d.identity)
        .find_map(|d| row.field(&d.name).and_then(|f| f.value.as_str().map(String::from)))
        .or_else(|| row.aliases.first().cloned())
        .unwrap_or_else(|| row.header.id.0.clone())
}

fn field_text(value: &Value) -> String {
    let mut out = String::new();
    value_text(value, &mut out);
    out.trim_end().to_string()
}

/// The rendered text card: the type, the name, the aliases and each searchable or identity field. This is what the keyword index reads and what gets embedded, and it's deterministic, so an unchanged entity is never embedded again.
pub fn render_card(row: &EntityRow, schema: &Value) -> String {
    let defs = fields_of(schema);
    let mut card = format!("{}: {}", row.type_name, label_of(row, &defs));
    if !row.aliases.is_empty() {
        card.push_str("\nalso known as: ");
        card.push_str(&row.aliases.join(", "));
    }
    for d in defs.iter().filter(|d| d.searchable || d.identity) {
        if let Some(f) = row.field(&d.name) {
            card.push('\n');
            card.push_str(&d.name);
            card.push_str(": ");
            card.push_str(&field_text(&f.value));
        }
    }
    card
}

/// The document for an entity. Its sources are those behind its current values, as with a claim.
pub(crate) fn entity_doc(
    row: &EntityRow,
    schema: &Value,
    cited: &[&EvidenceRow],
    segments: &HashMap<ObjectId, SegmentRow>,
    sources: &HashMap<ObjectId, SourceRow>,
) -> IndexDoc {
    let (mut source_ids, mut language) = (BTreeSet::new(), None);
    for e in cited {
        if let Some(s) = segments.get(&e.segment_id) {
            source_ids.insert(s.source_id.clone());
            language = language.or_else(|| s.language.clone());
        }
    }
    let trust_tiers: BTreeSet<String> = source_ids.iter().filter_map(|id| sources.get(id)).map(|s| s.trust_tier.clone()).collect();
    let attrs = fields_of(schema)
        .iter()
        .filter(|d| d.searchable)
        .filter_map(|d| {
            row.field(&d.name).map(|f| AttrValue {
                type_name: row.type_name.clone(),
                field: d.name.clone(),
                text: field_text(&f.value),
            })
        })
        .collect();
    IndexDoc {
        id: row.header.id.clone(),
        kind: DocKind::Entity,
        language,
        text: render_card(row, schema),
        source_ids: source_ids.into_iter().collect(),
        epistemic: None,
        valid_from_ms: None,
        valid_to_ms: None,
        doc_type: Some(row.type_name.clone()),
        trust_tiers: trust_tiers.into_iter().collect(),
        attrs,
    }
}

/// The ids of the claims behind an entity's current values.
pub(crate) fn current_claims(row: &EntityRow) -> impl Iterator<Item = &ObjectId> {
    row.fields.iter().map(|f| &f.assertion_id)
}

/// As `segment_ops`, for entities. Store calls don't grow with the number of ids. An entity that's gone or merged away leaves the index.
pub fn entity_ops<S: Snapshot>(snap: &S, ids: &[ObjectId]) -> Result<Vec<IndexOp>, StoreError> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let keys: Vec<RowKey> = ids.iter().map(|id| key(Table::Entity, id)).collect();
    let rows: Vec<EntityRow> = snap
        .get_many(&keys)?
        .into_iter()
        .filter_map(|r| match r {
            Some(Row::Entity(e)) if e.is_current() => Some(e),
            _ => None,
        })
        .collect();
    let names: Vec<String> = rows.iter().map(|r| r.type_name.clone()).collect::<BTreeSet<_>>().into_iter().collect();
    let types = latest_types(snap, &names)?;
    let claims: Vec<ObjectId> = rows.iter().flat_map(|r| current_claims(r).cloned()).collect();
    let evidence = if claims.is_empty() { vec![] } else { snap.evidence_of(&claims)? };
    let segment_keys: Vec<RowKey> = evidence
        .iter()
        .map(|e| e.segment_id.clone())
        .collect::<BTreeSet<_>>()
        .iter()
        .map(|id| key(Table::Segment, id))
        .collect();
    let segments: HashMap<ObjectId, SegmentRow> = if segment_keys.is_empty() {
        HashMap::new()
    } else {
        snap.get_many(&segment_keys)?
            .into_iter()
            .filter_map(|r| match r {
                Some(Row::Segment(s)) => Some((s.header.id.clone(), s)),
                _ => None,
            })
            .collect()
    };
    let sources = crate::lexical::sources_by_id(snap, segments.values().map(|s| s.source_id.clone()))?;
    let mut by_claim: HashMap<&ObjectId, Vec<&EvidenceRow>> = HashMap::new();
    for e in &evidence {
        by_claim.entry(&e.assertion_id).or_default().push(e);
    }
    let mut found: HashMap<ObjectId, IndexOp> = HashMap::with_capacity(rows.len());
    for row in &rows {
        let Some(t) = types.get(&row.type_name) else { continue };
        let cited: Vec<&EvidenceRow> = current_claims(row).flat_map(|c| by_claim.get(c).into_iter().flatten().copied()).collect();
        found.insert(row.header.id.clone(), IndexOp::Upsert(entity_doc(row, &t.schema, &cited, &segments, &sources)));
    }
    Ok(ids.iter().map(|id| found.remove(id).unwrap_or_else(|| IndexOp::Delete(id.clone()))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use memex_types::FilterValue;

    fn schema(props: Value) -> Value {
        json!({"type": "object", "additionalProperties": false, "properties": props})
    }

    #[test]
    fn every_seed_type_passes_its_own_rules() {
        let names: Vec<&str> = seed_types().iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Decision", "Organization", "Person", "Product", "Project"]);
        for t in seed_types() {
            check_schema(&t.name, &t.schema).unwrap_or_else(|e| panic!("{}: {e}", t.name));
        }
    }

    #[test]
    fn a_schema_can_only_refer_to_itself() {
        let local = json!({"type": "object", "additionalProperties": false, "$defs": {"n": {"type": "number"}}, "properties": {"a": {"$ref": "#/$defs/n"}}});
        check_schema("T", &local).unwrap();
        for bad in [
            json!({"type": "object", "additionalProperties": false, "properties": {"a": {"$ref": "https://example.com/s.json"}}}),
            json!({"type": "object", "additionalProperties": false, "properties": {"a": {"$ref": "other.json#/x"}}}),
            json!({"type": "object", "additionalProperties": false, "properties": {"a": {"$dynamicRef": "#x"}}}),
            json!({"$id": "https://example.com/", "type": "object", "additionalProperties": false, "properties": {"a": {"type": "string"}}}),
            json!({"$schema": "https://example.com/meta", "type": "object", "additionalProperties": false, "properties": {"a": {"type": "string"}}}),
            json!({"type": "object", "additionalProperties": false, "properties": {"a": {"allOf": [{"items": {"$ref": "file:///etc/passwd"}}]}}}),
        ] {
            assert!(check_schema("T", &bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn bad_schemas_say_what_is_wrong() {
        let cases = [
            (json!({"properties": {"a": {"type": "string"}}, "additionalProperties": false}), "type"),
            (json!({"type": "object", "properties": {"a": {"type": "string"}}}), "additionalProperties"),
            (json!({"type": "object", "additionalProperties": false, "properties": {}}), "at least one"),
            (schema(json!({"a.b": {"type": "string"}})), "field name"),
            (schema(json!({"$a": {"type": "string"}})), "field name"),
            (schema(json!({"a": {"type": "string", "searchable": "yes"}})), "annotation"),
            (schema(json!({"a": {"type": "object", "filterable": true}})), "annotated"),
            (schema(json!({"a": {"type": "number", "identity": true}})), "identity"),
            (schema(json!({"a": {"type": "not-a-type"}})), "isn't valid"),
        ];
        for (bad, says) in cases {
            let e = check_schema("T", &bad).unwrap_err();
            assert!(e.contains(says), "{bad}: {e}");
        }
        assert!(check_schema("1Bad", &schema(json!({"a": {"type": "string"}}))).is_err());
        assert!(check_schema("a@b", &schema(json!({"a": {"type": "string"}}))).is_err());
    }

    #[test]
    fn a_type_with_too_many_fields_is_refused_even_if_they_are_nested() {
        let many = |n: usize| -> Value { (0..n).map(|i| (format!("f{i}"), json!({"type": "string"}))).collect::<Map<_, _>>().into() };
        check_schema("T", &schema(many(MAX_FIELDS_PER_TYPE))).unwrap();
        let e = check_schema("T", &schema(many(MAX_FIELDS_PER_TYPE + 1))).unwrap_err();
        assert!(e.contains("limit"), "{e}");
        assert!(check_schema("T", &schema(many(10_000))).is_err());
        let nested = schema(json!({"a": {"type": "object", "properties": many(MAX_FIELDS_PER_TYPE)}}));
        assert!(check_schema("T", &nested).is_err());
    }

    #[test]
    fn changes_are_classified() {
        let base = json!({"type": "object", "additionalProperties": false, "required": ["a"], "properties": {
            "a": {"type": "string", "searchable": true}, "b": {"type": "number"}
        }});
        let with = |edit: fn(&mut Value)| {
            let mut s = base.clone();
            edit(&mut s);
            classify_change(&base, &s).0
        };
        use TypeChange::{Additive, Breaking};
        assert_eq!(with(|s| s["properties"]["c"] = json!({"type": "string"})), Additive);
        assert_eq!(with(|s| s["properties"]["b"]["filterable"] = json!(true)), Additive);
        assert_eq!(with(|s| s["properties"]["a"]["description"] = json!("x")), Additive);
        assert_eq!(with(|s| s["required"] = json!([])), Additive);
        assert_eq!(
            with(|s| {
                s["properties"].as_object_mut().unwrap().remove("b");
            }),
            Breaking
        );
        assert_eq!(with(|s| s["properties"]["b"]["type"] = json!("string")), Breaking);
        assert_eq!(with(|s| s["properties"]["b"]["maximum"] = json!(5)), Breaking);
        assert_eq!(with(|s| s["properties"]["a"]["searchable"] = json!(false)), Breaking);
        assert_eq!(with(|s| s["required"] = json!(["a", "b"])), Breaking);
        assert_eq!(
            with(|s| {
                s["properties"]["c"] = json!({"type": "string"});
                s["required"] = json!(["a", "c"]);
            }),
            Breaking
        );
        assert_eq!(with(|s| s["minProperties"] = json!(1)), Breaking);
    }

    #[test]
    fn times_parse_and_bad_ones_do_not() {
        assert_eq!(parse_time("1970-01-01"), Some(0));
        assert_eq!(parse_time("1970-01-02"), Some(86_400_000));
        assert_eq!(parse_time("2000-03-01"), Some(951_868_800_000));
        assert_eq!(parse_time("2024-02-29"), Some(1_709_164_800_000));
        assert_eq!(parse_time("1970-01-01T00:00:01Z"), Some(1000));
        assert_eq!(parse_time("1970-01-01T01:00:00+01:00"), Some(0));
        assert_eq!(parse_time("1970-01-01T00:00:00.5Z"), Some(500));
        for bad in [
            "2023-02-29",
            "2024-13-01",
            "2024-1-1",
            "soon",
            "2024-01-01T25:00:00Z",
            "2024-01-01T00:00:00",
            "",
        ] {
            assert_eq!(parse_time(bad), None, "{bad}");
        }
    }

    #[test]
    fn filters_are_typed_by_the_field() {
        let defs = fields_of(&schema(json!({
            "name": {"type": "string", "filterable": true},
            "price": {"type": "number", "filterable": true},
            "at": {"type": "string", "format": "date", "filterable": true},
            "live": {"type": "boolean", "filterable": true},
            "note": {"type": "string"}
        })));
        assert_eq!(
            parse_filter(&defs, "name", "eq", " ACME  Pump").unwrap().value,
            FilterValue::Text("acme pump".into())
        );
        assert_eq!(parse_filter(&defs, "price", "lt", "100").unwrap().value, FilterValue::Number(100.0));
        assert_eq!(parse_filter(&defs, "at", "gte", "1970-01-02").unwrap().value, FilterValue::Time(86_400_000));
        for (f, op, v) in [
            ("nope", "eq", "x"),
            ("note", "eq", "x"),
            ("name", "lt", "x"),
            ("name", "like", "x"),
            ("price", "lt", "cheap"),
            ("price", "lt", "NaN"),
            ("at", "lt", "soon"),
            ("live", "eq", "maybe"),
            ("live", "lt", "true"),
        ] {
            assert!(parse_filter(&defs, f, op, v).is_err(), "{f} {op} {v}");
        }
    }

    fn claim(id: &str, cs: &str, field: &str, value: Value, epistemic: Epistemic, machine: bool) -> AssertionRow {
        AssertionRow {
            header: Header {
                id: ObjectId(id.into()),
                creator: "t".into(),
                status: Status::Active,
                version: 1,
                acl_id: None,
            },
            subject: "entity:e".into(),
            predicate: field.into(),
            value,
            epistemic,
            machine,
            confidence_permille: 1000,
            valid_from_ms: None,
            valid_to_ms: None,
            canonical_hash: String::new(),
            changeset_id: ObjectId(cs.into()),
            superseded_by: None,
        }
    }

    fn entity() -> EntityRow {
        EntityRow {
            header: Header {
                id: ObjectId("e".into()),
                creator: "t".into(),
                status: Status::Active,
                version: 1,
                acl_id: None,
            },
            type_name: "T".into(),
            type_version: 1,
            aliases: vec!["Åke".into(), "AKE".into()],
            alias_keys: vec![],
            fields: vec![],
            merged_from: vec![],
            redirect_to: None,
            changeset_id: ObjectId("c".into()),
            updated_at_ms: 0,
        }
    }

    #[test]
    fn the_choice_and_why_follow_standing_then_people_then_age() {
        let s = schema(json!({"p": {"type": "number", "filterable": true}, "tags": {"type": "array", "items": {"type": "string"}, "filterable": true}}));
        let cases = [
            (Epistemic::Verified, false, Epistemic::Asserted, false, "verified beats asserted"),
            (Epistemic::Asserted, true, Epistemic::Inferred, false, "asserted beats inferred"),
            (Epistemic::Asserted, false, Epistemic::Asserted, true, "a person's claim beats a machine's"),
        ];
        for (ae, am, be, bm, why) in cases {
            let (a, b) = (claim("a", "1", "p", json!(1), ae, am), claim("b", "2", "p", json!(2), be, bm));
            let mut row = entity();
            materialize(&mut row, &s, &[&b, &a]);
            let f = row.field("p").unwrap();
            assert_eq!(f.value, json!(1));
            assert_eq!(f.why, why);
            assert_eq!(f.others.len(), 1);
            assert_eq!(f.others[0].value, json!(2));
        }
        let (a, b) = (
            claim("a", "1", "p", json!(1), Epistemic::Asserted, true),
            claim("b", "2", "p", json!(2), Epistemic::Asserted, true),
        );
        let mut row = entity();
        materialize(&mut row, &s, &[&a, &b]);
        assert_eq!(row.field("p").unwrap().value, json!(2));
        assert!(row.field("p").unwrap().why.contains("newer"));
        let same = claim("c", "3", "p", json!(2), Epistemic::Asserted, true);
        let mut row = entity();
        materialize(&mut row, &s, &[&same, &b]);
        assert!(row.field("p").unwrap().why.contains("agree"));
        let mut row = entity();
        materialize(&mut row, &s, &[&same]);
        assert!(row.field("p").unwrap().why.contains("only"));
    }

    #[test]
    fn the_same_claims_in_any_order_give_the_same_state() {
        let s = schema(json!({"p": {"type": "number", "filterable": true}}));
        let (a, b, c) = (
            claim("a", "1", "p", json!(1), Epistemic::Inferred, true),
            claim("b", "2", "p", json!(2), Epistemic::Asserted, true),
            claim("c", "3", "p", json!(3), Epistemic::Asserted, false),
        );
        let (mut one, mut two) = (entity(), entity());
        materialize(&mut one, &s, &[&a, &b, &c]);
        materialize(&mut two, &s, &[&c, &a, &b]);
        assert_eq!(one, two);
        assert_eq!(one.alias_keys, ["ake"], "aliases are folded and deduplicated");
    }

    #[test]
    fn a_claim_on_a_field_the_type_lost_is_not_current_state() {
        let s = schema(json!({"p": {"type": "number"}}));
        let (a, b) = (
            claim("a", "1", "p", json!(1), Epistemic::Asserted, false),
            claim("b", "1", "gone", json!(1), Epistemic::Asserted, false),
        );
        let mut row = entity();
        materialize(&mut row, &s, &[&a, &b]);
        assert_eq!(row.fields.len(), 1);
    }

    #[test]
    fn lookups_hold_what_a_filter_compares() {
        let s = schema(json!({
            "name": {"type": "string"},
            "price": {"type": "number"},
            "at": {"type": "string", "format": "date"},
            "live": {"type": "boolean"},
            "tags": {"type": "array", "items": {"type": "string"}},
            "obj": {"type": "object"}
        }));
        let long = "x".repeat(MAX_LOOKUP_TEXT_BYTES + 1);
        let claims = [
            claim("a", "1", "name", json!("Åke  Berg"), Epistemic::Asserted, false),
            claim("b", "1", "price", json!(9.5), Epistemic::Asserted, false),
            claim("c", "1", "at", json!("1970-01-02"), Epistemic::Asserted, false),
            claim("d", "1", "live", json!(true), Epistemic::Asserted, false),
            claim("e", "1", "tags", json!(["A", "a", "b"]), Epistemic::Asserted, false),
            claim("f", "1", "obj", json!({"k": 1}), Epistemic::Asserted, false),
        ];
        let refs: Vec<&AssertionRow> = claims.iter().collect();
        let mut row = entity();
        materialize(&mut row, &s, &refs);
        let values = row.value_rows();
        assert_eq!(values.iter().filter(|v| v.field == "$alias").count(), 1);
        assert_eq!(values.iter().find(|v| v.field == "name").unwrap().text.as_deref(), Some("ake berg"));
        assert_eq!(values.iter().find(|v| v.field == "price").unwrap().number, Some(9.5));
        assert_eq!(values.iter().find(|v| v.field == "at").unwrap().time_ms, Some(86_400_000));
        assert_eq!(values.iter().find(|v| v.field == "live").unwrap().text.as_deref(), Some("true"));
        assert_eq!(values.iter().filter(|v| v.field == "tags").count(), 2, "array items, deduplicated");
        assert!(values.iter().all(|v| v.field != "obj"));
        let c = claim("g", "1", "name", json!(long), Epistemic::Asserted, false);
        let mut row = entity();
        materialize(&mut row, &s, &[&c]);
        assert!(row.value_rows().iter().all(|v| v.field != "name"), "too long for an exact match");
        row.redirect_to = Some(ObjectId("x".into()));
        assert!(row.value_rows().is_empty(), "a merged entity can't be found by filters");
    }

    #[test]
    fn the_card_says_what_is_searchable() {
        let s = schema(json!({
            "name": {"type": "string", "identity": true},
            "desc": {"type": "string", "searchable": true},
            "secret": {"type": "string"}
        }));
        let claims = [
            claim("a", "1", "name", json!("Acme"), Epistemic::Asserted, false),
            claim("b", "1", "desc", json!("A pump"), Epistemic::Asserted, false),
            claim("c", "1", "secret", json!("hidden"), Epistemic::Asserted, false),
        ];
        let refs: Vec<&AssertionRow> = claims.iter().collect();
        let mut row = entity();
        materialize(&mut row, &s, &refs);
        let card = render_card(&row, &s);
        assert_eq!(card, "T: Acme\nalso known as: Åke, AKE\ndesc: A pump\nname: Acme");
        let doc = entity_doc(&row, &s, &[], &HashMap::new(), &HashMap::new());
        assert_eq!(doc.kind, DocKind::Entity);
        assert_eq!(doc.doc_type.as_deref(), Some("T"));
        assert_eq!(doc.attrs.len(), 1);
        assert_eq!(doc.attrs[0].field, "desc");
    }

    #[test]
    fn only_people_approve_what_the_rules_reserve_for_them() {
        let define = Op::DefineEntityType {
            name: "T".into(),
            base_version: None,
            schema: json!({}),
        };
        assert!(needs_a_person(&[OpKind::MergeEntities], &[], false));
        assert!(needs_a_person(&[OpKind::SplitEntity], &[], false));
        assert!(needs_a_person(&[OpKind::BreakingSchemaChange], &[], false));
        assert!(needs_a_person(&[OpKind::AdditiveSchemaChange], std::slice::from_ref(&define), true));
        assert!(!needs_a_person(&[OpKind::AdditiveSchemaChange], &[define], false));
        assert!(!needs_a_person(&[OpKind::AddClaim], &[], true));
    }
}
