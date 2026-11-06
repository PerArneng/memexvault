//! Decides how to answer a query, by rules alone: there's no model on the read path.
use crate::lexical::{QueryError, parse_terms, trim_word};
use memex_types::{LexicalFilter, ObjectId, QueryPath, QueryTerm, SearchMode};
use std::collections::BTreeSet;

/// Spec 018 measured 32-word sentences at p95 12 ms at 200k, and longer ones cost more with little gain, so the keyword index sees at most this many.
pub const MAX_LEXICAL_TERMS: usize = 32;
/// How many hits each signal offers to fusion.
pub const CANDIDATES_PER_SIGNAL: usize = 100;
/// Literal matches count this much more on a query that's mostly identifiers.
pub const IDENTIFIER_LEXICAL_WEIGHT: f32 = 2.0;

#[derive(Debug, Clone, PartialEq)]
pub struct QueryPlan {
    pub path: QueryPath,
    pub use_lexical: bool,
    pub use_vector: bool,
    pub lexical_weight: f32,
    pub vector_weight: f32,
    /// The id to look up first, on the exact path.
    pub exact_id: Option<ObjectId>,
    pub terms: Vec<QueryTerm>,
    pub dropped_terms: usize,
}

/// A UUID, which is what every object's id is.
pub fn is_object_id(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => *b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

/// A word whose spelling is the point: `ORA-00904`, `XK-42/B`, `H2O`, `NASA`. An ordinary word or a plain number isn't.
pub fn is_identifier(word: &str) -> bool {
    let word = trim_word(word);
    let digits = word.chars().any(|c| c.is_ascii_digit());
    let letters = word.chars().any(char::is_alphabetic);
    let joined = word.chars().any(|c| matches!(c, '-' | '/' | '_' | '.' | ':'));
    let shouting = word.chars().count() >= 3 && letters && word.chars().all(|c| !c.is_alphabetic() || c.is_uppercase());
    (digits && (letters || joined)) || shouting
}

fn identifier_share(terms: &[QueryTerm]) -> f32 {
    let words: Vec<&str> = terms
        .iter()
        .flat_map(|t| match t {
            QueryTerm::Word(w) => vec![w.as_str()],
            QueryTerm::Phrase(p) => p.iter().map(String::as_str).collect(),
        })
        .collect();
    if words.is_empty() {
        return 0.0;
    }
    words.iter().filter(|w| is_identifier(w)).count() as f32 / words.len() as f32
}

/// Keeps a term's first appearance, up to the limit. Returns how many it cut.
fn cut_terms(terms: Vec<QueryTerm>) -> (Vec<QueryTerm>, usize) {
    let total = terms.len();
    let mut seen = BTreeSet::new();
    let kept: Vec<QueryTerm> = terms
        .into_iter()
        .filter(|t| {
            seen.insert(match t {
                QueryTerm::Word(w) => w.to_lowercase(),
                QueryTerm::Phrase(p) => p.join(" ").to_lowercase(),
            })
        })
        .take(MAX_LEXICAL_TERMS)
        .collect();
    let dropped = total - kept.len();
    (kept, dropped)
}

fn filter_is_set(f: &LexicalFilter) -> bool {
    *f != LexicalFilter::default()
}

/// Bad queries are refused here, before anything runs. A query with too many words isn't bad: its first ones search the keyword index, and all of them reach the embedder.
pub fn plan_query(text: &str, filter: &LexicalFilter, mode: SearchMode) -> Result<QueryPlan, QueryError> {
    crate::vector::check_query_text(text)?;
    let (use_lexical, use_vector) = match mode {
        SearchMode::Hybrid => (true, true),
        SearchMode::Lexical => (true, false),
        SearchMode::Vector => (false, true),
    };
    let (terms, dropped_terms) = if use_lexical { cut_terms(parse_terms(text)?) } else { (vec![], 0) };
    let trimmed = text.trim();
    let exact_id = is_object_id(trimmed).then(|| ObjectId(trimmed.to_string()));
    let identifier = use_lexical && use_vector && identifier_share(&terms) >= 0.5;
    let path = if exact_id.is_some() {
        QueryPath::Exact
    } else if identifier {
        QueryPath::Identifier
    } else if filter_is_set(filter) {
        QueryPath::Filtered
    } else {
        QueryPath::Natural
    };
    Ok(QueryPlan {
        path,
        use_lexical,
        use_vector,
        lexical_weight: if identifier { IDENTIFIER_LEXICAL_WEIGHT } else { 1.0 },
        vector_weight: 1.0,
        exact_id,
        terms,
        dropped_terms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(text: &str) -> QueryPath {
        plan_query(text, &LexicalFilter::default(), SearchMode::Hybrid).unwrap().path
    }

    #[test]
    fn ids_go_to_the_store() {
        let id = "0192f3a1-7c2e-7a10-9b3e-1234567890ab";
        assert_eq!(path(id), QueryPath::Exact);
        assert_eq!(path(&format!("  {id} ")), QueryPath::Exact);
        let plan = plan_query(id, &LexicalFilter::default(), SearchMode::Hybrid).unwrap();
        assert_eq!(plan.exact_id, Some(ObjectId(id.into())));
        assert_ne!(path("0192f3a1-7c2e-7a10-9b3e-1234567890az"), QueryPath::Exact, "not hex");
        assert_ne!(path("0192f3a1"), QueryPath::Exact);
    }

    #[test]
    fn identifiers_are_told_from_ordinary_words() {
        for w in ["ORA-00904", "XK-42/B", "H2O", "NASA", "(ISO-9001),", "1.2.3"] {
            assert!(is_identifier(w), "{w}");
        }
        for w in ["hello", "Göteborg", "1984", "A", "well-known", "EU"] {
            assert!(!is_identifier(w), "{w}");
        }
    }

    #[test]
    fn a_query_of_mostly_identifiers_favours_keywords() {
        assert_eq!(path("ORA-00904"), QueryPath::Identifier);
        assert_eq!(path("error ORA-00904"), QueryPath::Identifier);
        let plan = plan_query("XK-42/B manual", &LexicalFilter::default(), SearchMode::Hybrid).unwrap();
        assert_eq!((plan.lexical_weight, plan.vector_weight), (IDENTIFIER_LEXICAL_WEIGHT, 1.0));
        assert_eq!(path("what does error ORA-00904 mean exactly"), QueryPath::Natural);
    }

    #[test]
    fn a_filter_picks_the_filtered_path_unless_something_stronger_applies() {
        let f = LexicalFilter {
            trust_tiers: vec!["trusted".into()],
            ..Default::default()
        };
        assert_eq!(plan_query("red bikes", &f, SearchMode::Hybrid).unwrap().path, QueryPath::Filtered);
        assert_eq!(plan_query("ORA-00904", &f, SearchMode::Hybrid).unwrap().path, QueryPath::Identifier);
        assert_eq!(path("red bikes"), QueryPath::Natural);
    }

    #[test]
    fn modes_choose_the_signals() {
        let f = LexicalFilter::default();
        let signals = |m| {
            let p = plan_query("ORA-00904 x", &f, m).unwrap();
            (p.use_lexical, p.use_vector, p.path)
        };
        assert_eq!(signals(SearchMode::Hybrid), (true, true, QueryPath::Identifier));
        assert_eq!(signals(SearchMode::Lexical), (true, false, QueryPath::Natural));
        assert_eq!(signals(SearchMode::Vector), (false, true, QueryPath::Natural));
    }

    #[test]
    fn only_the_vector_mode_ignores_keyword_syntax() {
        let f = LexicalFilter::default();
        assert_eq!(plan_query(r#"open "quote"#, &f, SearchMode::Hybrid).unwrap_err(), QueryError::UnbalancedQuote);
        assert!(plan_query(r#"open "quote"#, &f, SearchMode::Vector).is_ok());
    }

    #[test]
    fn bad_queries_are_refused() {
        let f = LexicalFilter::default();
        for mode in [SearchMode::Hybrid, SearchMode::Lexical, SearchMode::Vector] {
            assert_eq!(plan_query("   ", &f, mode).unwrap_err(), QueryError::Empty);
            assert!(matches!(
                plan_query(&"a".repeat(crate::lexical::MAX_QUERY_BYTES + 1), &f, mode),
                Err(QueryError::TooLong { .. })
            ));
        }
        assert_eq!(plan_query("?! --", &f, SearchMode::Hybrid).unwrap_err(), QueryError::Empty);
    }

    #[test]
    fn a_long_query_is_cut_not_refused() {
        let text: String = (0..100).map(|i| format!("w{i} ")).collect();
        let plan = plan_query(&text, &LexicalFilter::default(), SearchMode::Hybrid).unwrap();
        assert_eq!((plan.terms.len(), plan.dropped_terms), (MAX_LEXICAL_TERMS, 100 - MAX_LEXICAL_TERMS));
        assert_eq!(plan.terms[0], QueryTerm::Word("w0".into()), "the first words stay");
    }

    #[test]
    fn repeated_words_count_once() {
        let plan = plan_query("red RED red bike", &LexicalFilter::default(), SearchMode::Hybrid).unwrap();
        assert_eq!(plan.terms, vec![QueryTerm::Word("red".into()), QueryTerm::Word("bike".into())]);
        assert_eq!(plan.dropped_terms, 2);
    }
}
