//! The vector index on pgvector. Vectors, watermark and generations live in the vault's schema, so a write and its watermark commit in one transaction.
use memex_store_postgres::{Conn, PgStore, schema_of_vault};
use memex_types::{Generation, LexicalFilter, LexicalHit, LexicalResults, ModelInfo, ObjectId, VaultId, VectorError, VectorHealth, VectorOp, VectorQuery};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

/// An exact scan is faster and better than the graph below this many matches. Spec 004 measured about 0.75 µs per row on one thread, and 25 ms at p95 for 20,000 rows on 8.
const EXACT_MAX_MATCHES: f64 = 12_000.0;

/// How a search runs. Which one is best depends on how much of the vault a filter matches, per spec 004.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchPlan {
    /// Reads every match. Perfect recall, and cheap when there are few.
    Exact,
    /// Walks the graph, going on until enough results pass the filter when `iterative`.
    Graph { ef_search: u32, iterative: bool },
}

/// Pure. `matches` and `total` are estimates, and `matches` is `None` with no filter.
pub fn choose_plan(matches: Option<f64>, total: f64, limit: usize) -> SearchPlan {
    let at_least = |ef: u32| ef.max(limit.min(1000) as u32);
    let Some(matches) = matches else {
        return SearchPlan::Graph {
            ef_search: at_least(300),
            iterative: false,
        };
    };
    if matches <= EXACT_MAX_MATCHES {
        return SearchPlan::Exact;
    }
    let share = if total > 0.0 { matches / total } else { 1.0 };
    let ef = if share >= 0.35 {
        300
    } else if share >= 0.15 {
        200
    } else {
        100
    };
    SearchPlan::Graph {
        ef_search: at_least(ef),
        iterative: true,
    }
}

/// `[a,b,c]`, the form pgvector reads.
pub fn vector_text(v: &[f32]) -> String {
    let mut out = String::with_capacity(v.len() * 11 + 2);
    out.push('[');
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(out, "{x}");
    }
    out.push(']');
    out
}

/// Reads the form pgvector writes. `None` for anything else.
pub fn parse_vector(text: &str) -> Option<Vec<f32>> {
    let inner = text.strip_prefix('[')?.strip_suffix(']')?;
    if inner.is_empty() {
        return Some(vec![]);
    }
    inner.split(',').map(|x| x.parse().ok()).collect()
}

struct GenRow {
    gen_id: i32,
    model: ModelInfo,
    watermark: Option<u64>,
}

fn backend(e: postgres::Error) -> VectorError {
    match e.as_db_error() {
        Some(db) => VectorError::Backend(format!("{}: {}", db.code().code(), db.message())),
        None => VectorError::Backend(e.to_string()),
    }
}

fn store_error(e: memex_store_postgres::PgError) -> VectorError {
    VectorError::Backend(e.to_string())
}

fn table(schema: &str, gen_id: i32) -> String {
    format!("{schema}.vectors_gen{gen_id}")
}

const fn role_of(generation: Generation) -> &'static str {
    match generation {
        Generation::Live => "live",
        Generation::Building => "building",
    }
}

/// A string as an SQL literal. Search sends one batch of statements in one round trip, which can't carry parameters, so this is the only way user input reaches that SQL. `None` for text Postgres can't hold.
pub fn quote(text: &str) -> Option<String> {
    if text.contains('\0') {
        return None;
    }
    Some(format!("'{}'", text.replace('\'', "''")))
}

fn quoted_array(items: impl IntoIterator<Item = String>) -> Result<String, VectorError> {
    let quoted: Vec<String> = items
        .into_iter()
        .map(|i| quote(&i).ok_or_else(|| VectorError::Invalid("a filter holds a NUL character".into())))
        .collect::<Result<_, _>>()?;
    Ok(format!("ARRAY[{}]::text[]", quoted.join(",")))
}

/// The filter as a WHERE clause of literals, or nothing with no filter.
pub fn filter_sql(f: &LexicalFilter) -> Result<String, VectorError> {
    let mut parts = vec![];
    if !f.kinds.is_empty() {
        parts.push(format!("kind = ANY({})", quoted_array(f.kinds.iter().map(|k| k.as_str().to_string()))?));
    }
    if !f.source_ids.is_empty() {
        parts.push(format!("source_ids && {}", quoted_array(f.source_ids.iter().map(|s| s.0.clone()))?));
    }
    if !f.epistemic.is_empty() {
        parts.push(format!("epistemic = ANY({})", quoted_array(f.epistemic.iter().cloned())?));
    }
    if let Some(t) = f.valid_at_ms {
        parts.push(format!("(valid_from IS NULL OR valid_from <= {t}) AND (valid_to IS NULL OR {t} < valid_to)"));
    }
    Ok(if parts.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", parts.join(" AND "))
    })
}

/// The rows of a batch of statements, as text.
fn batch_rows(c: &mut Conn, sql: &str) -> Result<Vec<postgres::SimpleQueryRow>, VectorError> {
    Ok(c.client
        .simple_query(sql)
        .map_err(backend)?
        .into_iter()
        .filter_map(|m| match m {
            postgres::SimpleQueryMessage::Row(r) => Some(r),
            _ => None,
        })
        .collect())
}

/// Shares the store's connection pool, since it's the same database.
pub struct PgVectors {
    store: Arc<PgStore>,
}

impl PgVectors {
    pub fn new(store: Arc<PgStore>) -> Self {
        Self { store }
    }

    fn with_conn<T>(&self, work: impl FnOnce(&mut Conn) -> Result<T, VectorError>) -> Result<T, VectorError> {
        let mut conn = self.store.take_conn().map_err(store_error)?;
        let result = work(&mut conn);
        // A failed batch can leave its transaction open, and a pooled connection must not carry that on, so it's closed instead.
        if result.is_ok() {
            self.store.give_conn(conn);
        }
        result
    }

    fn generation(conn: &mut Conn, schema: &str, role: &str) -> Result<Option<GenRow>, VectorError> {
        let sql = format!("SELECT gen, model, dim, watermark FROM {schema}.vector_generations WHERE role = $1");
        let row = conn.query_opt(&sql, &[&role]);
        match row {
            Ok(row) => Ok(row.map(|r| {
                let dim: i32 = r.get(2);
                let watermark: Option<i64> = r.get(3);
                GenRow {
                    gen_id: r.get(0),
                    model: ModelInfo {
                        name: r.get(1),
                        dimension: dim as u32,
                    },
                    watermark: watermark.map(|w| w as u64),
                }
            })),
            // The vault has no schema yet, so it has no generation either.
            Err(e) if e.as_db_error().is_some_and(|d| d.code().code() == "42P01" || d.code().code() == "3F000") => Ok(None),
            Err(e) => Err(backend(e)),
        }
    }

    /// Size in bytes of what this vault's vector search holds on disk: stored embeddings, and each generation with its indexes.
    pub fn disk_bytes(&self, vault: &VaultId) -> Result<(u64, u64), VectorError> {
        let schema = schema_of_vault(vault);
        self.with_conn(|c| {
            let embeddings: i64 = c
                .client
                .query_one(&format!("SELECT pg_total_relation_size('{schema}.embeddings')"), &[])
                .map_err(backend)?
                .get(0);
            let live = Self::generation(c, &schema, "live")?;
            let live_bytes: i64 = match live {
                Some(g) => c
                    .client
                    .query_one(&format!("SELECT pg_total_relation_size('{}')", table(&schema, g.gen_id)), &[])
                    .map_err(backend)?
                    .get(0),
                None => 0,
            };
            Ok((embeddings as u64, live_bytes as u64))
        })
    }

    /// How many documents the live generation holds.
    pub fn count(&self, vault: &VaultId) -> Result<u64, VectorError> {
        let schema = schema_of_vault(vault);
        self.with_conn(|c| {
            let Some(g) = Self::generation(c, &schema, "live")? else {
                return Ok(0);
            };
            let n: i64 = c
                .client
                .query_one(&format!("SELECT count(*) FROM {}", table(&schema, g.gen_id)), &[])
                .map_err(backend)?
                .get(0);
            Ok(n as u64)
        })
    }

    /// The planner's guess at how many rows match and how many there are, in one round trip.
    fn estimate(conn: &mut Conn, name: &str, clause: &str) -> Result<(f64, f64), VectorError> {
        let quoted = quote(name).ok_or_else(|| VectorError::Invalid("a bad table name".into()))?;
        let rows = batch_rows(
            conn,
            &format!("EXPLAIN (FORMAT JSON) SELECT 1 FROM {name} {clause}; SELECT reltuples::text FROM pg_class WHERE oid = {quoted}::regclass;"),
        )?;
        let plan: serde_json::Value = rows
            .first()
            .and_then(|r| r.get(0))
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or_default();
        let matches = plan[0]["Plan"]["Plan Rows"].as_f64().unwrap_or(0.0);
        let total: f64 = rows.get(1).and_then(|r| r.get(0)).and_then(|t| t.parse().ok()).unwrap_or(0.0);
        Ok((matches, total.max(0.0)))
    }
}

impl PgVectors {
    pub fn health(&self, vault: &VaultId) -> Result<VectorHealth, VectorError> {
        let schema = schema_of_vault(vault);
        self.with_conn(|c| {
            Ok(match Self::generation(c, &schema, "live")? {
                Some(g) => VectorHealth::Ready {
                    model: g.model,
                    watermark: g.watermark,
                },
                None => VectorHealth::Missing,
            })
        })
    }

    pub fn watermark(&self, vault: &VaultId, generation: Generation) -> Result<Option<u64>, VectorError> {
        let schema = schema_of_vault(vault);
        self.with_conn(|c| Ok(Self::generation(c, &schema, role_of(generation))?.and_then(|g| g.watermark)))
    }

    pub fn begin_build(&self, vault: &VaultId, model: &ModelInfo) -> Result<(), VectorError> {
        let schema = schema_of_vault(vault);
        self.with_conn(|c| {
            let mut t = c.client.transaction().map_err(backend)?;
            let stale = t
                .query(&format!("DELETE FROM {schema}.vector_generations WHERE role = 'building' RETURNING gen"), &[])
                .map_err(backend)?;
            for row in stale {
                t.batch_execute(&format!("DROP TABLE IF EXISTS {}", table(&schema, row.get(0))))
                    .map_err(backend)?;
            }
            let gen_id: i32 = t
                .query_one(&format!("SELECT coalesce(max(gen), 0) + 1 FROM {schema}.vector_generations"), &[])
                .map_err(backend)?
                .get(0);
            let name = table(&schema, gen_id);
            t.batch_execute(&format!(
                "CREATE TABLE {name} (
                    id text COLLATE \"C\" PRIMARY KEY,
                    kind text NOT NULL,
                    source_ids text[] NOT NULL,
                    epistemic text,
                    valid_from bigint,
                    valid_to bigint,
                    embedding halfvec({dim}) NOT NULL
                );",
                dim = model.dimension
            ))
            .map_err(backend)?;
            t.execute(
                &format!("INSERT INTO {schema}.vector_generations (gen, model, dim, role, watermark) VALUES ($1, $2, $3, 'building', NULL)"),
                &[&gen_id, &model.name, &(model.dimension as i32)],
            )
            .map_err(backend)?;
            t.commit().map_err(backend)
        })
    }

    pub fn apply(&self, vault: &VaultId, generation: Generation, ops: &[VectorOp], watermark: Option<u64>) -> Result<(), VectorError> {
        let schema = schema_of_vault(vault);
        self.with_conn(|c| {
            let g = Self::generation(c, &schema, role_of(generation))?.ok_or_else(|| VectorError::Invalid("no such generation".into()))?;
            for op in ops {
                if let VectorOp::Upsert(d) = op
                    && d.vector.len() as u32 != g.model.dimension
                {
                    return Err(VectorError::WrongDimension {
                        expected: g.model.dimension,
                        got: d.vector.len() as u32,
                    });
                }
            }
            // The last write to an id in a batch is the one that counts.
            let mut last: HashMap<&str, &VectorOp> = HashMap::with_capacity(ops.len());
            for op in ops {
                let id = match op {
                    VectorOp::Upsert(d) => d.id.0.as_str(),
                    VectorOp::Delete(id) => id.0.as_str(),
                };
                last.insert(id, op);
            }
            let (mut ids, mut kinds, mut sources, mut epistemic, mut from, mut to, mut vectors, mut gone) = (vec![], vec![], vec![], vec![], vec![], vec![], vec![], vec![]);
            for op in last.values() {
                match op {
                    VectorOp::Upsert(d) => {
                        ids.push(d.id.0.clone());
                        kinds.push(d.kind.as_str().to_string());
                        sources.push(serde_json::json!(d.source_ids.iter().map(|s| s.0.as_str()).collect::<Vec<_>>()));
                        epistemic.push(d.epistemic.clone());
                        from.push(d.valid_from_ms);
                        to.push(d.valid_to_ms);
                        vectors.push(vector_text(&d.vector));
                    }
                    VectorOp::Delete(id) => gone.push(id.0.clone()),
                }
            }
            let name = table(&schema, g.gen_id);
            let mut t = c.client.transaction().map_err(backend)?;
            if !ids.is_empty() {
                t.execute(
                    &format!(
                        "INSERT INTO {name} (id, kind, source_ids, epistemic, valid_from, valid_to, embedding)
                         SELECT d.id, d.kind, ARRAY(SELECT jsonb_array_elements_text(d.sources)), d.epistemic, d.vf, d.vt, d.v::halfvec
                         FROM unnest($1::text[], $2::text[], $3::jsonb[], $4::text[], $5::bigint[], $6::bigint[], $7::text[]) AS d(id, kind, sources, epistemic, vf, vt, v)
                         ON CONFLICT (id) DO UPDATE SET kind = EXCLUDED.kind, source_ids = EXCLUDED.source_ids, epistemic = EXCLUDED.epistemic,
                            valid_from = EXCLUDED.valid_from, valid_to = EXCLUDED.valid_to, embedding = EXCLUDED.embedding"
                    ),
                    &[&ids, &kinds, &sources, &epistemic, &from, &to, &vectors],
                )
                .map_err(backend)?;
            }
            if !gone.is_empty() {
                t.execute(&format!("DELETE FROM {name} WHERE id = ANY($1::text[])"), &[&gone]).map_err(backend)?;
            }
            let mark = watermark.map(|w| w as i64);
            t.execute(&format!("UPDATE {schema}.vector_generations SET watermark = $1 WHERE gen = $2"), &[&mark, &g.gen_id])
                .map_err(backend)?;
            t.commit().map_err(backend)
        })
    }

    /// Builds the graph and filter indexes first, which is slow but only touches the new table, so search goes on using the old one. Building them after loading is faster, and with `fastupdate` off the source index has no pending list, which spec 019 measured costing a filtered search 14 ms per query.
    pub fn flip(&self, vault: &VaultId) -> Result<(), VectorError> {
        let schema = schema_of_vault(vault);
        self.with_conn(|c| {
            let g = Self::generation(c, &schema, "building")?.ok_or_else(|| VectorError::Invalid("nothing is being built".into()))?;
            let name = table(&schema, g.gen_id);
            c.client
                .batch_execute(&format!(
                    "BEGIN; SET LOCAL maintenance_work_mem = '1500MB';
                     CREATE INDEX IF NOT EXISTS vectors_gen{gen_id}_graph ON {name} USING hnsw (embedding halfvec_ip_ops) WITH (m = 16, ef_construction = 128);
                     CREATE INDEX IF NOT EXISTS vectors_gen{gen_id}_sources ON {name} USING gin (source_ids) WITH (fastupdate = off);
                     CREATE INDEX IF NOT EXISTS vectors_gen{gen_id}_kind ON {name} (kind);
                     CREATE INDEX IF NOT EXISTS vectors_gen{gen_id}_epistemic ON {name} (epistemic);
                     COMMIT; ANALYZE {name};",
                    gen_id = g.gen_id
                ))
                .map_err(backend)?;
            let mut t = c.client.transaction().map_err(backend)?;
            let old = t
                .query(&format!("DELETE FROM {schema}.vector_generations WHERE role = 'previous' RETURNING gen"), &[])
                .map_err(backend)?;
            for row in old {
                t.batch_execute(&format!("DROP TABLE IF EXISTS {}", table(&schema, row.get(0))))
                    .map_err(backend)?;
            }
            t.batch_execute(&format!(
                "UPDATE {schema}.vector_generations SET role = 'previous' WHERE role = 'live';
                 UPDATE {schema}.vector_generations SET role = 'live' WHERE gen = {};",
                g.gen_id
            ))
            .map_err(backend)?;
            t.commit().map_err(backend)
        })
    }

    pub fn rollback(&self, vault: &VaultId) -> Result<(), VectorError> {
        let schema = schema_of_vault(vault);
        self.with_conn(|c| {
            let previous = Self::generation(c, &schema, "previous")?.ok_or_else(|| VectorError::Invalid("there's no earlier generation".into()))?;
            let live = Self::generation(c, &schema, "live")?;
            let mut t = c.client.transaction().map_err(backend)?;
            if let Some(live) = live {
                t.batch_execute(&format!(
                    "DELETE FROM {schema}.vector_generations WHERE gen = {};
                     DROP TABLE IF EXISTS {};",
                    live.gen_id,
                    table(&schema, live.gen_id)
                ))
                .map_err(backend)?;
            }
            t.batch_execute(&format!("UPDATE {schema}.vector_generations SET role = 'live' WHERE gen = {}", previous.gen_id))
                .map_err(backend)?;
            t.commit().map_err(backend)
        })
    }

    pub fn search(&self, vault: &VaultId, query: &VectorQuery) -> Result<LexicalResults, VectorError> {
        let schema = schema_of_vault(vault);
        self.with_conn(|c| {
            let g = Self::generation(c, &schema, "live")?.ok_or_else(|| VectorError::Unavailable("no generation yet".into()))?;
            if g.model.name != query.model {
                return Err(VectorError::ModelMismatch {
                    live: g.model.name,
                    asked: query.model.clone(),
                });
            }
            if query.vector.len() as u32 != g.model.dimension {
                return Err(VectorError::WrongDimension {
                    expected: g.model.dimension,
                    got: query.vector.len() as u32,
                });
            }
            let name = table(&schema, g.gen_id);
            let clause = filter_sql(&query.filter)?;
            let estimated = if clause.is_empty() { None } else { Some(Self::estimate(c, &name, &clause)?) };
            let plan = choose_plan(estimated.map(|e| e.0), estimated.map_or(0.0, |e| e.1), query.limit);
            let settings = match plan {
                // Which plan the planner picks here depends on its statistics sample, and p95 was twice as bad on some samples, so it gets no choice.
                SearchPlan::Exact => {
                    "SET LOCAL enable_indexscan = off; SET LOCAL enable_seqscan = off; SET LOCAL max_parallel_workers_per_gather = 0;".to_string()
                }
                SearchPlan::Graph { ef_search, iterative } => format!(
                    "SET LOCAL hnsw.ef_search = {ef_search}; SET LOCAL hnsw.iterative_scan = {};",
                    if iterative { "relaxed_order" } else { "off" }
                ),
            };
            // One round trip. As a constant, the query vector is read once: passed as text and cast, Postgres would read it again for every row it scores.
            let target = format!("'{}'::vector::halfvec({})", vector_text(&query.vector), g.model.dimension);
            let sql = format!(
                "BEGIN; SET LOCAL jit = off; {settings}
                 SELECT id, kind, -d FROM (SELECT id, kind, embedding <#> {target} AS d FROM {name} {clause} ORDER BY d LIMIT {limit}) AS best;
                 COMMIT;",
                limit = query.limit
            );
            let rows = batch_rows(c, &sql)?;
            let mut hits: Vec<LexicalHit> = rows
                .iter()
                .filter_map(|r| {
                    Some(LexicalHit {
                        id: ObjectId(r.get(0)?.to_string()),
                        kind: memex_types::DocKind::parse(r.get(1)?)?,
                        score: r.get(2)?.parse().ok()?,
                    })
                })
                .collect();
            // The graph orders by score alone, and sorting by id too would stop it being used.
            hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
            Ok(LexicalResults { hits, watermark: g.watermark })
        })
    }

    pub fn embeddings(&self, vault: &VaultId, model: &str, hashes: &[String]) -> Result<HashMap<String, Vec<f32>>, VectorError> {
        let schema = schema_of_vault(vault);
        self.with_conn(|c| {
            let sql = format!("SELECT content_hash, vector::text FROM {schema}.embeddings WHERE model = $1 AND content_hash = ANY($2::text[])");
            let rows = c.query(&sql, &[&model, &hashes]).map_err(backend)?;
            rows.into_iter()
                .map(|r| {
                    let text: String = r.get(1);
                    let vector = parse_vector(&text).ok_or_else(|| VectorError::Backend("a stored embedding can't be read".into()))?;
                    Ok((r.get(0), vector))
                })
                .collect()
        })
    }

    pub fn store_embeddings(&self, vault: &VaultId, model: &str, rows: &[(String, Vec<f32>)]) -> Result<(), VectorError> {
        if rows.is_empty() {
            return Ok(());
        }
        let schema = schema_of_vault(vault);
        self.with_conn(|c| {
            let hashes: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
            let vectors: Vec<String> = rows.iter().map(|r| vector_text(&r.1)).collect();
            let sql = format!(
                "INSERT INTO {schema}.embeddings (model, content_hash, vector)
                 SELECT $1, d.h, d.v::halfvec FROM unnest($2::text[], $3::text[]) AS d(h, v)
                 ON CONFLICT (model, content_hash) DO NOTHING"
            );
            c.query(&sql, &[&model, &hashes, &vectors]).map_err(backend)?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_filter_uses_the_graph_without_iterating() {
        assert_eq!(
            choose_plan(None, 200_000.0, 50),
            SearchPlan::Graph {
                ef_search: 300,
                iterative: false
            }
        );
    }

    #[test]
    fn few_matches_go_exact_whatever_the_vault_size() {
        assert_eq!(choose_plan(Some(10.0), 1_000_000.0, 50), SearchPlan::Exact);
        assert_eq!(choose_plan(Some(EXACT_MAX_MATCHES), 200_000.0, 50), SearchPlan::Exact);
        assert_eq!(choose_plan(Some(12.0), 0.0, 50), SearchPlan::Exact, "a table never analyzed");
    }

    #[test]
    fn wider_filters_search_a_wider_graph() {
        let ef = |share: f64| match choose_plan(Some(share * 1_000_000.0), 1_000_000.0, 50) {
            SearchPlan::Graph { ef_search, iterative: true } => ef_search,
            other => panic!("{other:?}"),
        };
        assert_eq!(ef(0.5), 300);
        assert_eq!(ef(0.2), 200);
        assert_eq!(ef(0.1), 100);
    }

    #[test]
    fn a_large_limit_widens_the_graph_search() {
        assert_eq!(
            choose_plan(None, 1000.0, 500),
            SearchPlan::Graph {
                ef_search: 500,
                iterative: false
            }
        );
        assert_eq!(
            choose_plan(None, 1000.0, 100_000),
            SearchPlan::Graph {
                ef_search: 1000,
                iterative: false
            },
            "pgvector's own limit"
        );
    }

    #[test]
    fn vectors_survive_their_text_form() {
        let v = vec![0.5, -0.25, 1e-3, 0.0];
        assert_eq!(parse_vector(&vector_text(&v)), Some(v));
        assert_eq!(parse_vector("[]"), Some(vec![]));
        assert_eq!(parse_vector("0.5,1"), None);
        assert_eq!(parse_vector("[0.5,x]"), None);
    }

    #[test]
    fn filters_become_literals() {
        let f = LexicalFilter {
            kinds: vec![memex_types::DocKind::Claim],
            source_ids: vec![ObjectId("a'; DROP TABLE x; --".into())],
            epistemic: vec!["verified".into()],
            valid_at_ms: Some(5),
        };
        let sql = filter_sql(&f).unwrap();
        assert!(sql.contains("kind = ANY(ARRAY['claim']::text[])"), "{sql}");
        assert!(sql.contains("source_ids && ARRAY['a''; DROP TABLE x; --']::text[]"), "{sql}");
        assert!(sql.contains("epistemic = ANY(ARRAY['verified']::text[])"), "{sql}");
        assert!(sql.contains("valid_from <= 5") && sql.contains("5 < valid_to"), "{sql}");
        assert_eq!(filter_sql(&LexicalFilter::default()).unwrap(), "");
    }

    #[test]
    fn text_postgres_can_t_hold_is_refused() {
        assert_eq!(quote("it's"), Some("'it''s'".to_string()));
        assert_eq!(quote("back\\slash"), Some("'back\\slash'".to_string()));
        assert_eq!(quote("a\0b"), None);
        let f = LexicalFilter {
            epistemic: vec!["a\0b".into()],
            ..Default::default()
        };
        assert!(matches!(filter_sql(&f), Err(VectorError::Invalid(_))));
    }
}
