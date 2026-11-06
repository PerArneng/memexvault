//! Decides whether a result looks like an answer at all. Rank fusion always returns something, so this reads the signals' own scores instead.
use crate::lexical::trim_word;
use memex_types::QueryTerm;

/// Lowercase with common diacritics removed, so `Goteborg` matches `Göteborg`.
pub fn fold_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars().flat_map(char::to_lowercase) {
        match c {
            'å' | 'ä' | 'á' | 'à' | 'â' | 'ã' => out.push('a'),
            'ö' | 'ó' | 'ò' | 'ô' | 'õ' | 'ø' => out.push('o'),
            'ü' | 'ú' | 'ù' | 'û' => out.push('u'),
            'é' | 'è' | 'ê' | 'ë' => out.push('e'),
            'í' | 'ì' | 'î' | 'ï' => out.push('i'),
            'ñ' => out.push('n'),
            'ç' => out.push('c'),
            'ß' => out.push_str("ss"),
            c => out.push(c),
        }
    }
    out
}

/// The share of a query's longer words, by characters, that appear in any of these texts. Short words carry little meaning, in any language.
pub fn term_coverage(terms: &[QueryTerm], texts: &[&str]) -> Option<f32> {
    let words: Vec<String> = terms
        .iter()
        .flat_map(|t| match t {
            QueryTerm::Word(w) => vec![w.as_str()],
            QueryTerm::Phrase(p) => p.iter().map(String::as_str).collect(),
        })
        .map(|w| fold_text(trim_word(w)))
        .filter(|w| w.chars().count() >= MIN_WORD_CHARS)
        .collect();
    if words.is_empty() {
        return None;
    }
    let haystack: Vec<String> = texts.iter().map(|t| fold_text(t)).collect();
    let weight = |w: &String| w.chars().count() as f32;
    let total: f32 = words.iter().map(weight).sum();
    let found: f32 = words.iter().filter(|w| haystack.iter().any(|h| h.contains(w.as_str()))).map(weight).sum();
    Some(found / total)
}

const MIN_WORD_CHARS: usize = 4;

/// The signals behind the verdict. Each is absent if its signal didn't run.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Signals {
    /// Best cosine similarity from the vector index.
    pub best_vector: Option<f32>,
    /// From `term_coverage`, over the top items.
    pub coverage: Option<f32>,
}

/// Spec 020 chose these on half the golden set and checked them on the other half, for e5-small: another embedding model needs its own. Rerank scores are left out, since at any floor that caught 80% of unanswerable questions they also flagged about a quarter of answerable ones.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    pub vector_floor: f32,
    pub coverage_floor: f32,
}

pub const THRESHOLDS: Thresholds = Thresholds {
    vector_floor: 0.85,
    coverage_floor: 0.65,
};

/// True when nothing looks like an answer: no text is close in meaning, and none holds most of the query's words.
pub fn is_low_relevance(s: Signals, t: Thresholds) -> bool {
    let by_vector = s.best_vector.is_some_and(|v| v >= t.vector_floor);
    let by_words = s.coverage.is_some_and(|c| c >= t.coverage_floor);
    !(by_vector || by_words)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Thresholds = Thresholds {
        vector_floor: 0.8,
        coverage_floor: 0.5,
    };

    fn w(s: &str) -> QueryTerm {
        QueryTerm::Word(s.into())
    }

    #[test]
    fn folding_matches_the_tested_languages() {
        assert_eq!(fold_text("Göteborg ÅÄÖ"), "goteborg aao");
        assert_eq!(fold_text("Straße"), "strasse");
        assert_eq!(fold_text("Niño"), "nino");
    }

    #[test]
    fn coverage_weighs_longer_words_and_ignores_short_ones() {
        let terms = [w("who"), w("is"), w("zorblax"), w("founder")];
        assert_eq!(term_coverage(&terms, &["The founder of it."]), Some(7.0 / 14.0));
        assert_eq!(term_coverage(&terms, &["founder", "Zorblax"]), Some(1.0), "any text counts");
        assert_eq!(term_coverage(&terms, &[]), Some(0.0));
        assert_eq!(term_coverage(&[w("is"), w("it")], &["is it"]), None, "nothing long enough to judge");
        assert_eq!(
            term_coverage(&[QueryTerm::Phrase(vec!["red".into(), "bicycle".into()])], &["a bicycle"]),
            Some(1.0)
        );
    }

    #[test]
    fn either_signal_clearing_its_floor_is_enough() {
        let s = |v, c| Signals { best_vector: v, coverage: c };
        assert!(!is_low_relevance(s(Some(0.9), Some(0.0)), T));
        assert!(!is_low_relevance(s(Some(0.1), Some(0.9)), T));
        assert!(is_low_relevance(s(Some(0.1), Some(0.1)), T));
        assert!(is_low_relevance(s(Some(0.1), None), T));
        assert!(!is_low_relevance(s(None, Some(0.9)), T));
        assert!(is_low_relevance(s(None, None), T));
    }
}
