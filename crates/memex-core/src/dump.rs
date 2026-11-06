use crate::{Snapshot, StoreError};
use memex_types::{Row, Table};
use sha2::{Digest, Sha256};
use std::fmt::Write;

/// The full state of a vault in one canonical byte form, for comparing stores.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dump {
    pub text: String,
    pub sha256: String,
}

/// Same rows and head in any input order give the same bytes. `serde_json` sorts object keys, and rows are sorted here.
pub fn canonical_dump(head: Option<u64>, rows: &[Row]) -> Dump {
    let mut sorted: Vec<&Row> = rows.iter().collect();
    sorted.sort_by_key(|r| r.key());
    let mut text = format!("head {}\n", head.map_or("none".to_string(), |h| h.to_string()));
    for r in sorted {
        let value = serde_json::to_value(r).unwrap_or(serde_json::Value::Null);
        text.push_str(&value.to_string());
        text.push('\n');
    }
    let digest = Sha256::digest(text.as_bytes());
    let mut sha256 = String::with_capacity(64);
    for b in digest {
        let _ = write!(sha256, "{b:02x}");
    }
    Dump { text, sha256 }
}

/// Pages through a snapshot, so a large vault doesn't take one huge read.
pub fn dump_store<S: Snapshot>(snap: &S) -> Result<Dump, StoreError> {
    let mut rows = vec![];
    for table in Table::ALL {
        let mut after = None;
        loop {
            let page = snap.rows(table, after.as_ref())?;
            let Some(last) = page.last() else { break };
            after = Some(last.header().id.clone());
            rows.extend(page);
        }
    }
    Ok(canonical_dump(snap.head()?, &rows))
}

#[cfg(test)]
mod tests {
    use super::*;
    use memex_types::{Header, ObjectId, PageRow, Status};

    fn page(id: &str, title: &str, body: &str) -> Row {
        Row::Page(PageRow {
            header: Header {
                id: ObjectId(id.into()),
                creator: "a".into(),
                status: Status::Active,
                version: 1,
                acl_id: None,
            },
            title: title.into(),
            body: body.into(),
            updated_at_ms: 5,
        })
    }

    #[test]
    fn input_order_does_not_change_the_hash() {
        let (a, b, c) = (page("a", "Å", "Göteborg ß ñ"), page("b", "", ""), page("c", "日本語", "line\nbreak \"q\""));
        let one = canonical_dump(Some(3), &[a.clone(), b.clone(), c.clone()]);
        let two = canonical_dump(Some(3), &[c, a, b]);
        assert_eq!(one, two);
        assert_eq!(one.sha256.len(), 64);
    }

    #[test]
    fn empty_store_has_a_stable_dump() {
        let d = canonical_dump(None, &[]);
        assert_eq!(d.text, "head none\n");
        assert_eq!(d, canonical_dump(None, &[]));
    }

    #[test]
    fn any_change_changes_the_hash() {
        let base = canonical_dump(Some(1), &[page("a", "t", "x")]);
        assert_ne!(base.sha256, canonical_dump(Some(2), &[page("a", "t", "x")]).sha256);
        assert_ne!(base.sha256, canonical_dump(Some(1), &[page("a", "t", "y")]).sha256);
        assert_ne!(base.sha256, canonical_dump(Some(1), &[page("a", "t", "x"), page("b", "", "")]).sha256);
    }

    #[test]
    fn non_ascii_stays_verbatim() {
        assert!(canonical_dump(Some(1), &[page("a", "Åsa", "ñandú")]).text.contains("ñandú"));
    }
}
