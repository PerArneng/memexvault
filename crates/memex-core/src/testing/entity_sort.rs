//! Spec 087's sort checks, through the facade. Spec 087 runs the same functions against Postgres, so nothing here may assume MemStore.
//!
//! Rows go straight to the store with the values a commit would materialize, and each check compares the server's order with a sort written here from the rows' JSON values, independently of `FieldSort::compare`.
use crate::entity::{self, fold_key, parse_time};
use crate::{Caller, CommitItem, Server, ServerEdges, ServerError, Snapshot, Store};
use memex_types::{ChangeSetState, EntityField, EntityRow, Event, Header, Mutation, ObjectId, Row, Status, VaultId};
use serde_json::{Value, json};
use std::cmp::Ordering;
use std::collections::BTreeSet;

pub const TYPE: &str = "Sortable";

/// One field of each kind that sorts, a list, and a field that isn't filterable.
pub fn schema() -> Value {
    json!({"type": "object", "additionalProperties": false, "properties": {
        "name": {"type": "string", "filterable": true},
        "price": {"type": "number", "filterable": true},
        "released": {"type": "string", "format": "date-time", "filterable": true},
        "live": {"type": "boolean", "filterable": true},
        "category": {"type": "string", "filterable": true},
        "tags": {"type": "array", "items": {"type": "string"}, "filterable": true},
        "notes": {"type": "string"},
        "shape": {"type": "object"}
    }})
}

/// The fields that sort, with how the independent sort reads them.
pub const SORTED_FIELDS: [&str; 4] = ["name", "price", "released", "live"];

const NAMES: [&str; 9] = ["Beta", "alpha", "Älpha", "  gamma   delta", "beta", "Zulu", "zulu", "éclair", "Ωmega"];
const PRICES: [f64; 7] = [-12.5, 0.0, 3.0, 3.0, 1e9, -0.0, 99.99];
const TIMES: [&str; 5] = [
    "2026-01-01",
    "2026-01-01T00:00:00Z",
    "1969-12-31T23:59:59.5Z",
    "2030-06-15",
    "2026-01-01T01:00:00+01:00",
];

/// Row `i` of the fixture. Every field is missing on some rows, and every field has ties.
pub fn values(i: usize) -> Vec<(&'static str, Value)> {
    let mut out = vec![];
    if i % 6 != 5 {
        let name = if i % 17 == 3 { "x".repeat(600) } else { NAMES[i % NAMES.len()].to_string() };
        out.push(("name", json!(name)));
    }
    if i % 5 != 4 {
        out.push(("price", json!(PRICES[i % PRICES.len()])));
    }
    if i % 4 != 3 {
        out.push(("released", json!(TIMES[i % TIMES.len()])));
    }
    if i % 3 != 2 {
        out.push(("live", json!(i.is_multiple_of(2))));
    }
    out.push(("category", json!(format!("c{}", i % 3))));
    out.push(("tags", json!([format!("t{}", i % 2), "all", format!("t{}", i % 2)])));
    out.push(("notes", json!("a note")));
    out
}

/// An entity as a commit would leave it: each field's value with the lookups the production materializer computes.
pub fn row(id: &str, i: usize) -> EntityRow {
    let defs = entity::fields_of(&schema());
    let fields = values(i)
        .into_iter()
        .map(|(name, value)| {
            let def = defs.iter().find(|d| d.name == name).expect("a field of the schema");
            EntityField {
                field: name.into(),
                lookups: entity::lookups_of(&value, def),
                value,
                assertion_id: ObjectId(format!("{name}-{id}")),
                why: "it's the only claim on this field".into(),
                others: vec![],
            }
        })
        .collect();
    EntityRow {
        header: Header {
            id: ObjectId(id.into()),
            creator: "sort".into(),
            status: Status::Active,
            version: 1,
            acl_id: None,
        },
        type_name: TYPE.into(),
        type_version: 1,
        aliases: vec![],
        alias_keys: vec![],
        fields,
        merged_from: vec![],
        redirect_to: None,
        changeset_id: ObjectId("sort".into()),
        updated_at_ms: 0,
    }
}

/// The key the independent sort uses, read from the JSON value. None is missing.
#[derive(Debug, Clone, PartialEq, PartialOrd)]
enum Key {
    Text(String),
    Number(f64),
    Time(i64),
}

fn key(r: &EntityRow, field: &str) -> Option<Key> {
    let value = &r.fields.iter().find(|f| f.field == field)?.value;
    match (field, value) {
        // The field table holds text up to 512 bytes, folded.
        ("name", Value::String(s)) if s.len() <= 512 => Some(Key::Text(fold_key(s))),
        ("price", Value::Number(n)) => n.as_f64().map(Key::Number),
        ("released", Value::String(s)) => parse_time(s).map(Key::Time),
        ("live", Value::Bool(b)) => Some(Key::Text(if *b { "true" } else { "false" }.into())),
        _ => None,
    }
}

fn key_order(a: &Key, b: &Key) -> Ordering {
    match (a, b) {
        (Key::Text(x), Key::Text(y)) => x.as_bytes().cmp(y.as_bytes()),
        // Postgres's float8 order: -0 equals 0.
        (Key::Number(x), Key::Number(y)) => x.partial_cmp(y).expect("finite"),
        (Key::Time(x), Key::Time(y)) => x.cmp(y),
        _ => panic!("mixed kinds"),
    }
}

/// The spec's order, written out: rows with a value by value then id in the direction asked, then rows without one, by id in the same direction.
pub fn expected(rows: &[EntityRow], field: &str, descending: bool) -> Vec<ObjectId> {
    let (mut with, mut without): (Vec<_>, Vec<_>) = rows.iter().map(|r| (key(r, field), r.header.id.clone())).partition(|(k, _)| k.is_some());
    with.sort_by(|(a, x), (b, y)| {
        let o = key_order(a.as_ref().unwrap(), b.as_ref().unwrap()).then_with(|| x.0.cmp(&y.0));
        if descending { o.reverse() } else { o }
    });
    without.sort_by(|(_, x), (_, y)| if descending { y.0.cmp(&x.0) } else { x.0.cmp(&y.0) });
    with.into_iter().chain(without).map(|(_, id)| id).collect()
}

/// Defines the type through a proposal, as a vault would.
pub fn define<E: ServerEdges>(s: &Server<E>, admin: &Caller, v: &VaultId) {
    let o = s
        .propose_entity_type(admin, v, TYPE, None, schema(), "spec 087's fixture".into(), "sort-type".into())
        .unwrap();
    assert_eq!(o.changeset.unwrap().state, ChangeSetState::Committed);
}

/// Writes rows straight to the store, as one event.
pub fn write<E: ServerEdges>(s: &Server<E>, v: &VaultId, rows: &[EntityRow]) {
    let store = s.engine().store();
    let lease = s.vault_lease_for_tests(v).unwrap();
    let head = store.snapshot(v).unwrap().head().unwrap().unwrap_or(0);
    let event = Event {
        seq: head + 1,
        vault: v.clone(),
        actor: "sort".into(),
        time_ms: 0,
        kind: "state.recorded".into(),
        schema_version: 1,
        payload: json!({"mutations": []}),
    };
    let mutations: Vec<Mutation> = rows.iter().cloned().map(|r| Mutation::Put(Row::Entity(r))).collect();
    store
        .commit_batch(
            &lease,
            &[CommitItem {
                event: &event,
                mutations: &mutations,
            }],
        )
        .unwrap();
}

/// Every page, by the cursor each returns, with `between` called after each page.
pub fn walk<E: ServerEdges>(
    s: &Server<E>,
    c: &Caller,
    v: &VaultId,
    filters: &[(String, String, String)],
    sort: &str,
    page: usize,
    mut between: impl FnMut(usize),
) -> Vec<ObjectId> {
    let mut out = vec![];
    let mut cursor: Option<String> = None;
    for n in 0.. {
        let (rows, parsed) = s.list_entities_sorted(c, v, TYPE, filters, sort, cursor.as_deref(), page + 1).unwrap();
        let more = rows.len() > page;
        out.extend(rows.iter().take(page).map(|r| r.header.id.clone()));
        if !more {
            break;
        }
        cursor = Some(entity::sort_cursor(&parsed, &rows[page - 1]));
        between(n);
        assert!(n < 10_000, "the walk doesn't end");
    }
    out
}

fn ids(n: usize, from: usize) -> Vec<String> {
    // Not in id order of `i`, so the id tiebreak isn't the fixture's order.
    (from..from + n)
        .map(|i| format!("{:08x}{:024x}", (i * 2_654_435_761) % 0xffff_ffff, i))
        .collect()
}

fn filter(f: &str, op: &str, v: &str) -> (String, String, String) {
    (f.into(), op.into(), v.into())
}

/// Verification 1: each kind, both directions, missing values and ties, alone and with filters, on one page and on many.
pub fn sort_matches_an_independent_sort<E: ServerEdges>(s: &Server<E>, admin: &Caller, v: &VaultId) {
    define(s, admin, v);
    let rows: Vec<EntityRow> = ids(120, 0).iter().enumerate().map(|(i, id)| row(id, i)).collect();
    write(s, v, &rows);
    for field in SORTED_FIELDS {
        let has = rows.iter().filter(|r| key(r, field).is_some()).count();
        assert!(has > 0 && has < rows.len(), "{field} has both values and missing ones");
        for (dir, descending) in [("asc", false), ("desc", true)] {
            let sort = format!("{field}:{dir}");
            let want = expected(&rows, field, descending);
            let (one, _) = s.list_entities_sorted(admin, v, TYPE, &[], &sort, None, 1000).unwrap();
            assert_eq!(one.iter().map(|r| r.header.id.clone()).collect::<Vec<_>>(), want, "{sort}, one page");
            for page in [1, 7, 50] {
                assert_eq!(walk(s, admin, v, &[], &sort, page, |_| {}), want, "{sort}, pages of {page}");
            }
            for (filters, keep) in [
                (vec![filter("category", "eq", "c1")], &(|i: usize| i % 3 == 1) as &dyn Fn(usize) -> bool),
                (vec![filter("tags", "eq", "t0")], &|i: usize| i.is_multiple_of(2)),
                (vec![filter("tags", "eq", "all"), filter("price", "gte", "0")], &|i: usize| {
                    i % 5 != 4 && PRICES[i % PRICES.len()] >= 0.0
                }),
                // Filters on each sorted field itself, which Postgres folds into the scan of that field's index.
                (vec![filter("price", "lt", "50")], &|i: usize| i % 5 != 4 && PRICES[i % PRICES.len()] < 50.0),
                (vec![filter("name", "eq", "ALPHA")], &|i: usize| {
                    i % 6 != 5 && i % 17 != 3 && fold_key(NAMES[i % NAMES.len()]) == "alpha"
                }),
                (vec![filter("released", "gte", "2026-01-01")], &|i: usize| {
                    i % 4 != 3 && parse_time(TIMES[i % TIMES.len()]).unwrap() >= parse_time("2026-01-01").unwrap()
                }),
                (vec![filter("live", "eq", "true"), filter("category", "eq", "c0")], &|i: usize| {
                    i.is_multiple_of(3) && i.is_multiple_of(2)
                }),
            ] {
                let kept: Vec<EntityRow> = rows.iter().enumerate().filter(|(i, _)| keep(*i)).map(|(_, r)| r.clone()).collect();
                assert_eq!(
                    walk(s, admin, v, &filters, &sort, 4, |_| {}),
                    expected(&kept, field, descending),
                    "{sort} with {filters:?}"
                );
            }
        }
    }
}

/// Verification 2: each row once, page after page, while rows are added before and after the cursor and with no value.
pub fn walking_returns_each_row_once_while_rows_are_written<E: ServerEdges>(s: &Server<E>, admin: &Caller, v: &VaultId) {
    define(s, admin, v);
    let rows: Vec<EntityRow> = ids(90, 0).iter().enumerate().map(|(i, id)| row(id, i)).collect();
    write(s, v, &rows);
    let mut next = 1000;
    for field in SORTED_FIELDS {
        for (dir, descending) in [("asc", false), ("desc", true)] {
            let sort = format!("{field}:{dir}");
            let before = s.list_entities_sorted(admin, v, TYPE, &[], &sort, None, 100_000).unwrap().0;
            let got = walk(s, admin, v, &[], &sort, 6, |_| {
                let added: Vec<EntityRow> = ids(3, next).iter().enumerate().map(|(k, id)| row(id, next + k)).collect();
                next += 3;
                write(s, v, &added);
            });
            let unique: BTreeSet<&ObjectId> = got.iter().collect();
            assert_eq!(unique.len(), got.len(), "{sort}: a row came twice");
            let original: BTreeSet<ObjectId> = before.iter().map(|r| r.header.id.clone()).collect();
            let kept: Vec<ObjectId> = got.iter().filter(|id| original.contains(*id)).cloned().collect();
            assert_eq!(
                kept,
                expected(&before, field, descending),
                "{sort}: every row there at the start, once, in order"
            );
            assert!(got.len() > kept.len(), "{sort}: some rows written during the walk were reached");
        }
    }
}

/// Verification 4: what can't be sorted says why, as `Invalid`, which the API answers with 400.
pub fn bad_sorts_are_refused<E: ServerEdges>(s: &Server<E>, admin: &Caller, v: &VaultId) {
    define(s, admin, v);
    write(s, v, &[row(&ids(1, 0)[0], 0)]);
    let refused = |sort: &str, cursor: Option<&str>| match s.list_entities_sorted(admin, v, TYPE, &[], sort, cursor, 10) {
        Err(ServerError::Invalid(m)) => m,
        other => panic!("{sort} {cursor:?}: {other:?}"),
    };
    assert_eq!(refused("nope:asc", None), "the type has no field nope");
    assert_eq!(refused("notes:asc", None), "the field notes isn't filterable, so it can't be sorted");
    assert_eq!(refused("tags:asc", None), "the field tags holds a list, so it can't be sorted");
    assert_eq!(refused("shape:asc", None), "the field shape isn't filterable, so it can't be sorted");
    assert_eq!(refused("price", None), "a sort is <field>:asc or <field>:desc, not 'price'");
    assert_eq!(refused("price:up", None), "a sort's direction is asc or desc, not 'up'");
    assert_eq!(refused(":asc", None), "the type has no field ");
    for cursor in ["abc", "abc\n", "\nn1", "abc\nt1", "abc\nnNaN", "abc\nninf", "abc\nn1x", "abc\nd1"] {
        assert!(
            refused("price:asc", Some(cursor)).starts_with("this cursor isn't from a list in this order"),
            "{cursor:?}"
        );
    }
    assert!(matches!(
        s.list_entities_sorted(admin, v, "Nope", &[], "price:asc", None, 10),
        Err(ServerError::NotFound)
    ));
    // A good cursor of each kind is accepted.
    for (sort, cursor) in [
        ("price:asc", "abc\nn-1.5"),
        ("name:desc", "abc\ntx y"),
        ("released:asc", "abc\nd-5"),
        ("live:asc", "abc\n-"),
    ] {
        assert!(s.list_entities_sorted(admin, v, TYPE, &[], sort, Some(cursor), 10).is_ok(), "{sort} {cursor}");
    }
}
