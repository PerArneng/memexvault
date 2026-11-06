//! How text becomes search terms. The neutral field keeps identifiers whole, the folded field forgives missing accents, and each language's field stems.
use tantivy::tokenizer::{
    AsciiFoldingFilter, Language, LowerCaser, RemoveLongFilter, SimpleTokenizer, Stemmer, TextAnalyzer, Token, TokenStream, Tokenizer, TokenizerManager,
};
use unicode_normalization::UnicodeNormalization;

pub const EXACT: &str = "memex_exact";
pub const FOLDED: &str = "memex_folded";

/// The tested languages, with their field and analyzer names.
pub const STEMMED: [(&str, &str, Language); 4] = [
    ("en", "memex_stem_en", Language::English),
    ("sv", "memex_stem_sv", Language::Swedish),
    ("de", "memex_stem_de", Language::German),
    ("es", "memex_stem_es", Language::Spanish),
];

/// Longer tokens are garbage, and the engine would refuse them anyway.
const MAX_TOKEN_BYTES: usize = 200;

/// Splits on whitespace and trims punctuation at both ends, so `(XK-42/B),` is one token. Keep in step with `memex_core::lexical::trim_word`.
#[derive(Clone, Default)]
pub struct WholeWordTokenizer {
    token: Token,
}

pub struct WholeWordStream<'a> {
    text: &'a str,
    offset: usize,
    position: usize,
    token: &'a mut Token,
}

impl Tokenizer for WholeWordTokenizer {
    type TokenStream<'a> = WholeWordStream<'a>;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> WholeWordStream<'a> {
        self.token.reset();
        WholeWordStream {
            text,
            offset: 0,
            position: 0,
            token: &mut self.token,
        }
    }
}

impl TokenStream for WholeWordStream<'_> {
    fn advance(&mut self) -> bool {
        while self.offset < self.text.len() {
            let rest = &self.text[self.offset..];
            let Some(start) = rest.find(|c: char| !c.is_whitespace()) else {
                self.offset = self.text.len();
                return false;
            };
            let end = rest[start..].find(char::is_whitespace).map_or(rest.len(), |e| start + e);
            let word = &rest[start..end];
            let base = self.offset + start;
            self.offset += end;
            let trimmed = word.trim_matches(|c: char| !c.is_alphanumeric());
            if trimmed.is_empty() {
                continue;
            }
            let lead = word.len() - word.trim_start_matches(|c: char| !c.is_alphanumeric()).len();
            self.token.text.clear();
            self.token.text.push_str(trimmed);
            self.token.offset_from = base + lead;
            self.token.offset_to = base + lead + trimmed.len();
            self.token.position = self.position;
            self.position += 1;
            return true;
        }
        false
    }

    fn token(&self) -> &Token {
        self.token
    }

    fn token_mut(&mut self) -> &mut Token {
        self.token
    }
}

pub fn register(tokenizers: &TokenizerManager) {
    tokenizers.register(
        EXACT,
        TextAnalyzer::builder(WholeWordTokenizer::default())
            .filter(RemoveLongFilter::limit(MAX_TOKEN_BYTES))
            .filter(LowerCaser)
            .build(),
    );
    tokenizers.register(
        FOLDED,
        TextAnalyzer::builder(SimpleTokenizer::default())
            .filter(RemoveLongFilter::limit(MAX_TOKEN_BYTES))
            .filter(LowerCaser)
            .filter(AsciiFoldingFilter)
            .build(),
    );
    for (_, name, language) in STEMMED {
        tokenizers.register(
            name,
            TextAnalyzer::builder(SimpleTokenizer::default())
                .filter(RemoveLongFilter::limit(MAX_TOKEN_BYTES))
                .filter(LowerCaser)
                .filter(Stemmer::new(language))
                .build(),
        );
    }
}

/// One spelling for each letter, and no invisible characters, so text from a PDF matches what someone types.
pub fn normalize(text: &str) -> String {
    text.nfc()
        .filter(|c| !matches!(c, '\u{ad}' | '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{2060}' | '\u{feff}'))
        .collect()
}

/// The tokens an analyzer makes of this text, in order.
pub fn tokens(analyzer: &mut TextAnalyzer, text: &str) -> Vec<String> {
    let mut stream = analyzer.token_stream(text);
    let mut out = vec![];
    while stream.advance() {
        out.push(stream.token().text.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyzed(name: &str, text: &str) -> Vec<String> {
        let manager = TokenizerManager::default();
        register(&manager);
        tokens(&mut manager.get(name).unwrap(), text)
    }

    #[test]
    fn whole_words_keep_inner_punctuation() {
        assert_eq!(analyzed(EXACT, "(XK-42/B), ORA-00904: ok."), ["xk-42/b", "ora-00904", "ok"]);
        assert_eq!(analyzed(EXACT, "  -- ?? "), Vec::<String>::new());
        assert_eq!(analyzed(EXACT, ""), Vec::<String>::new());
        assert_eq!(
            analyzed(EXACT, "«Göteborg»"),
            ["göteborg"],
            "multi-byte punctuation trims on a character boundary"
        );
    }

    #[test]
    fn folding_drops_accents() {
        assert_eq!(analyzed(FOLDED, "Göteborg Straße niño"), ["goteborg", "strasse", "nino"]);
        assert_eq!(analyzed(FOLDED, "XK-42/B"), ["xk", "42", "b"]);
    }

    #[test]
    fn stems_join_inflected_forms() {
        assert_eq!(analyzed("memex_stem_sv", "bilarna"), analyzed("memex_stem_sv", "bil"));
        assert_eq!(analyzed("memex_stem_de", "Häusern"), analyzed("memex_stem_de", "Häuser"));
        assert_eq!(analyzed("memex_stem_es", "niños"), analyzed("memex_stem_es", "niño"));
        assert_eq!(analyzed("memex_stem_en", "running"), analyzed("memex_stem_en", "run"));
    }

    #[test]
    fn a_huge_token_is_dropped() {
        assert_eq!(analyzed(EXACT, &format!("a {} b", "x".repeat(10_000))), ["a", "b"]);
    }

    #[test]
    fn normalize_composes_and_removes_invisible_characters() {
        assert_eq!(normalize("Go\u{308}te\u{ad}borg\u{200b}"), "Göteborg");
    }
}
