use memex_types::{Mutation, Row, RowKey};
use std::collections::{HashMap, HashSet};

/// A batch's mutations collapsed to their final effect, so one transaction sends one statement per table.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct NetMutations {
    pub puts: Vec<Row>,
    pub deletes: Vec<RowKey>,
    /// Rows the batch deletes without having put them first. They must exist, or the batch is refused.
    pub must_exist: Vec<RowKey>,
}

/// Pure. `Err` names a row deleted twice in the batch with no put in between.
pub fn net_mutations<'a>(batches: impl IntoIterator<Item = &'a [Mutation]>) -> Result<NetMutations, RowKey> {
    let mut last: HashMap<RowKey, Option<&Row>> = HashMap::new();
    let mut must_exist: HashSet<RowKey> = HashSet::new();
    for batch in batches {
        for m in batch {
            match m {
                Mutation::Put(row) => {
                    last.insert(row.key(), Some(row));
                }
                Mutation::Delete(key) => match last.get(key) {
                    Some(None) => return Err(key.clone()),
                    Some(Some(_)) => {
                        last.insert(key.clone(), None);
                    }
                    None => {
                        must_exist.insert(key.clone());
                        last.insert(key.clone(), None);
                    }
                },
            }
        }
    }
    let mut net = NetMutations::default();
    for (key, effect) in last {
        match effect {
            Some(row) => net.puts.push(row.clone()),
            None => net.deletes.push(key),
        }
    }
    net.must_exist = must_exist.into_iter().collect();
    net.puts.sort_by_key(Row::key);
    net.deletes.sort();
    net.must_exist.sort();
    Ok(net)
}

#[cfg(test)]
mod tests {
    use super::*;
    use memex_types::{Header, ObjectId, PageRow, Status, Table};

    fn page(id: &str, body: &str) -> Row {
        Row::Page(PageRow {
            header: Header {
                id: ObjectId(id.into()),
                creator: "a".into(),
                status: Status::Active,
                version: 1,
                acl_id: None,
            },
            title: "t".into(),
            body: body.into(),
            updated_at_ms: 1,
        })
    }

    fn key(id: &str) -> RowKey {
        RowKey {
            table: Table::Page,
            id: ObjectId(id.into()),
        }
    }

    fn run(ops: &[Vec<Mutation>]) -> Result<NetMutations, RowKey> {
        net_mutations(ops.iter().map(Vec::as_slice))
    }

    #[test]
    fn the_last_put_wins() {
        let net = run(&[vec![Mutation::Put(page("a", "1"))], vec![Mutation::Put(page("a", "2"))]]).unwrap();
        assert_eq!(net.puts, vec![page("a", "2")]);
        assert!(net.deletes.is_empty() && net.must_exist.is_empty());
    }

    #[test]
    fn put_then_delete_needs_no_existing_row() {
        let net = run(&[vec![Mutation::Put(page("a", "1")), Mutation::Delete(key("a"))]]).unwrap();
        assert!(net.puts.is_empty() && net.must_exist.is_empty());
        assert_eq!(net.deletes, vec![key("a")]);
    }

    #[test]
    fn delete_then_put_requires_the_row_and_keeps_the_put() {
        let net = run(&[vec![Mutation::Delete(key("a"))], vec![Mutation::Put(page("a", "2"))]]).unwrap();
        assert_eq!(net.must_exist, vec![key("a")]);
        assert_eq!(net.puts, vec![page("a", "2")]);
        assert!(net.deletes.is_empty());
    }

    #[test]
    fn double_delete_is_refused() {
        assert_eq!(run(&[vec![Mutation::Delete(key("a")), Mutation::Delete(key("a"))]]), Err(key("a")));
    }

    #[test]
    fn empty_is_empty() {
        assert_eq!(run(&[]).unwrap(), NetMutations::default());
    }
}
