//! Turning parser output into canonical text, segments and anchors. All pure.
use memex_types::{Anchor, ObjectId, ParsedDocument, Rect, Word};

/// A word's place in the canonical text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WordSpan {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Canonical {
    pub text: String,
    /// Same length and order as the document's words.
    pub spans: Vec<WordSpan>,
}

fn same_line(a: &Rect, b: &Rect) -> bool {
    let overlap = a.y1.min(b.y1) - a.y0.max(b.y0);
    let shorter = (a.y1 - a.y0).min(b.y1 - b.y0).max(1);
    overlap * 2 > shorter
}

/// Builds the canonical text. Words on a line take a space, a new line takes a newline, and a wide gap or a new page takes a blank line. The same input always gives the same bytes.
pub fn canonical(words: &[Word]) -> Canonical {
    let mut text = String::new();
    let mut spans = Vec::with_capacity(words.len());
    let mut prev: Option<&Word> = None;
    for w in words {
        if let Some(p) = prev {
            let separator = if p.page != w.page {
                "\n\n"
            } else if same_line(&p.rect, &w.rect) && w.rect.x0 >= p.rect.x0 {
                " "
            } else {
                let height = (p.rect.y1 - p.rect.y0).min(w.rect.y1 - w.rect.y0).max(1);
                let gap = w.rect.y0 - p.rect.y1;
                // A jump back up the page means a new column, which also starts a new block.
                if gap * 10 > height * 8 || w.rect.y0 < p.rect.y0 { "\n\n" } else { "\n" }
            };
            text.push_str(separator);
        }
        let start = text.len();
        text.push_str(&w.text);
        spans.push(WordSpan { start, end: text.len() });
        prev = Some(w);
    }
    Canonical { text, spans }
}

/// Plain text and Markdown keep their text, with line endings normalized so offsets are stable.
pub fn canonical_plain(text: &str) -> Canonical {
    Canonical {
        text: text.replace("\r\n", "\n").replace('\r', "\n"),
        spans: vec![],
    }
}

/// A rough token count, near enough for sizing segments. Words times 4/3.
pub fn estimate_tokens(text: &str) -> usize {
    text.split_whitespace().count() * 4 / 3
}

pub const MIN_TOKENS: usize = 100;
pub const MAX_TOKENS: usize = 400;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentSpan {
    pub start: usize,
    pub end: usize,
    pub heading: Option<String>,
    pub page_from: Option<u32>,
    pub page_to: Option<u32>,
}

/// Blocks of text separated by blank lines, as byte ranges.
fn blocks(text: &str) -> Vec<(usize, usize)> {
    let (mut out, mut at) = (vec![], 0);
    for part in text.split("\n\n") {
        let trimmed = part.trim_matches('\n');
        if !trimmed.trim().is_empty() {
            let lead = part.len() - part.trim_start_matches('\n').len();
            out.push((at + lead, at + lead + trimmed.len()));
        }
        at += part.len() + 2;
    }
    out
}

/// Short, with no closing punctuation, and either larger than the body text or a Markdown heading.
fn is_heading(block: &str, tall: bool) -> bool {
    let line_count = block.lines().count();
    let word_count = block.split_whitespace().count();
    if block.starts_with('#') && line_count == 1 {
        return true;
    }
    tall && line_count <= 2 && word_count <= 14 && !block.trim_end().ends_with(['.', ',', ';', ':'])
}

/// Splits a long block at sentence ends where it can and at word boundaries where it can't.
fn split_long(text: &str, start: usize, end: usize, max_tokens: usize) -> Vec<(usize, usize)> {
    let (mut out, mut from) = (vec![], start);
    while estimate_tokens(&text[from..end]) > max_tokens {
        let budget_words = max_tokens * 3 / 4;
        let (mut last_space, mut last_sentence, mut words) = (None, None, 0);
        let mut prev_was_space = true;
        for (i, c) in text[from..end].char_indices() {
            let space = c.is_whitespace();
            if space && !prev_was_space {
                words += 1;
                last_space = Some(from + i);
                let before = text[from..from + i].chars().next_back();
                if matches!(before, Some('.' | '!' | '?')) {
                    last_sentence = Some(from + i);
                }
                if words >= budget_words {
                    break;
                }
            }
            prev_was_space = space;
        }
        let Some(cut) = last_sentence.filter(|s| estimate_tokens(&text[from..*s]) >= MIN_TOKENS).or(last_space) else {
            break;
        };
        out.push((from, cut));
        from = cut + text[cut..end].len() - text[cut..end].trim_start().len();
    }
    if from < end {
        out.push((from, end));
    }
    out
}

/// Segments of about 100 to 400 tokens along the document's structure: a heading starts a new one, and blocks join until the minimum.
pub fn segments(canon: &Canonical, words: &[Word]) -> Vec<SegmentSpan> {
    let text = &canon.text;
    let mut heights: Vec<i32> = words.iter().map(|w| w.rect.y1 - w.rect.y0).collect();
    heights.sort_unstable();
    let median = heights.get(heights.len() / 2).copied().unwrap_or(0);
    let tall_block = |start: usize, end: usize| -> bool {
        if canon.spans.is_empty() || median == 0 {
            return false;
        }
        let h: Vec<i32> = canon
            .spans
            .iter()
            .zip(words)
            .filter(|(s, _)| s.start >= start && s.end <= end)
            .map(|(_, w)| w.rect.y1 - w.rect.y0)
            .collect();
        !h.is_empty() && h.iter().sum::<i32>() * 100 / h.len() as i32 > median * 115
    };
    let pages_of = |start: usize, end: usize| -> (Option<u32>, Option<u32>) {
        let inside = canon.spans.iter().zip(words).filter(|(s, _)| s.end > start && s.start < end);
        let (mut lo, mut hi) = (None, None);
        for (_, w) in inside {
            lo = Some(lo.map_or(w.page, |l: u32| l.min(w.page)));
            hi = Some(hi.map_or(w.page, |h: u32| h.max(w.page)));
        }
        (lo, hi)
    };
    let mut out: Vec<SegmentSpan> = vec![];
    let mut heading: Option<String> = None;
    let mut current: Option<(usize, usize)> = None;
    let flush = |current: &mut Option<(usize, usize)>, heading: &Option<String>, out: &mut Vec<SegmentSpan>| {
        if let Some((s, e)) = current.take() {
            for (a, b) in split_long(text, s, e, MAX_TOKENS) {
                let (page_from, page_to) = pages_of(a, b);
                out.push(SegmentSpan {
                    start: a,
                    end: b,
                    heading: heading.clone(),
                    page_from,
                    page_to,
                });
            }
        }
    };
    for (start, end) in blocks(text) {
        let block = &text[start..end];
        if is_heading(block, tall_block(start, end)) {
            flush(&mut current, &heading, &mut out);
            heading = Some(block.trim_start_matches('#').trim().to_string());
            continue;
        }
        match current {
            None => current = Some((start, end)),
            Some((s, e)) => {
                if estimate_tokens(&text[s..e]) >= MIN_TOKENS {
                    flush(&mut current, &heading, &mut out);
                    current = Some((start, end));
                } else {
                    current = Some((s, end));
                }
            }
        }
    }
    flush(&mut current, &heading, &mut out);
    out
}

/// A language code when detection is confident, and nothing otherwise, because short text is often misdetected.
pub fn detect_language(text: &str) -> Option<String> {
    let info = whatlang::detect(text)?;
    if !info.is_reliable() && info.confidence() < 0.5 {
        return None;
    }
    use whatlang::Lang;
    let code = match info.lang() {
        Lang::Eng => "en",
        Lang::Swe => "sv",
        Lang::Deu => "de",
        Lang::Spa => "es",
        other => other.code(),
    };
    Some(code.to_string())
}

/// Text with runs of whitespace closed up, and a map from each kept byte to where it came from, so a quote matches across line breaks.
fn squeeze(text: &str) -> (String, Vec<usize>) {
    let (mut out, mut map) = (String::with_capacity(text.len()), Vec::with_capacity(text.len()));
    let mut pending_space = false;
    for (i, c) in text.char_indices() {
        if c.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            map.push(i);
            pending_space = false;
        }
        let at = out.len();
        out.push(c);
        map.extend((0..out.len() - at).map(|k| i + k));
    }
    map.push(text.len());
    (out, map)
}

/// How many characters of context an anchor keeps on each side.
pub const CONTEXT_CHARS: usize = 32;

fn floor_char(text: &str, mut at: usize) -> usize {
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

fn ceil_char(text: &str, mut at: usize) -> usize {
    while !text.is_char_boundary(at) {
        at += 1;
    }
    at
}

/// The byte range of a quote in the text. Whitespace differences don't matter. With several matches, the one whose surroundings best match the prefix and suffix wins, and a tie goes to the first.
pub fn find_quote(text: &str, quote: &str, prefix: &str, suffix: &str) -> Option<(usize, usize)> {
    let (hay, map) = squeeze(text);
    let needle = squeeze(quote).0;
    if needle.is_empty() {
        return None;
    }
    let want_before = squeeze(prefix).0;
    let want_after = squeeze(suffix).0;
    let mut best: Option<(usize, usize, usize)> = None;
    let mut from = 0;
    while let Some(i) = hay[from..].find(&needle) {
        let at = from + i;
        let end = at + needle.len();
        let context_before = hay[..at].trim_end();
        let context_after = hay[end..].trim_start();
        let before = &context_before[floor_char(context_before, context_before.len().saturating_sub(want_before.len()))..];
        let after = &context_after[..ceil_char(context_after, want_after.len().min(context_after.len()))];
        let common_suffix = before.chars().rev().zip(want_before.chars().rev()).take_while(|(a, b)| a == b).count();
        let common_prefix = after.chars().zip(want_after.chars()).take_while(|(a, b)| a == b).count();
        let score = common_suffix + common_prefix;
        if best.is_none_or(|(s, _, _)| score > s) {
            best = Some((score, at, end));
        }
        from = at + hay[at..].chars().next().map_or(1, char::len_utf8);
    }
    let (_, at, end) = best?;
    // The map has an entry per byte, so the byte after the last one kept is the end, including inside a multi-byte letter.
    Some((map[at], map[end - 1] + 1))
}

/// One rect per line, for the words a byte range covers.
pub fn rects_for(canon: &Canonical, words: &[Word], start: usize, end: usize) -> Vec<(u32, Rect)> {
    let mut out: Vec<(u32, Rect)> = vec![];
    for (span, w) in canon.spans.iter().zip(words) {
        if span.end <= start || span.start >= end {
            continue;
        }
        match out.last_mut() {
            Some((page, r)) if *page == w.page && same_line(r, &w.rect) => {
                r.x0 = r.x0.min(w.rect.x0);
                r.y0 = r.y0.min(w.rect.y0);
                r.x1 = r.x1.max(w.rect.x1);
                r.y1 = r.y1.max(w.rect.y1);
            }
            _ => out.push((w.page, w.rect)),
        }
    }
    out
}

/// The way back: the words whose centres sit in these rects, in reading order.
pub fn text_in_rects(words: &[Word], rects: &[(u32, Rect)]) -> String {
    let inside = |w: &Word| {
        let (cx, cy) = ((w.rect.x0 + w.rect.x1) / 2, (w.rect.y0 + w.rect.y1) / 2);
        rects.iter().any(|(p, r)| *p == w.page && r.x0 <= cx && cx <= r.x1 && r.y0 <= cy && cy <= r.y1)
    };
    words.iter().filter(|w| inside(w)).map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
}

/// Resolves a quote to a full anchor. None when the quote isn't in this rendition.
pub fn locate(canon: &Canonical, words: &[Word], source_sha256: &str, rendition_id: &ObjectId, quote: &str, prefix: &str, suffix: &str) -> Option<Anchor> {
    let (start, end) = find_quote(&canon.text, quote, prefix, suffix)?;
    Some(anchor_at(canon, words, source_sha256, rendition_id, start, end))
}

/// Resolves a quote to a full anchor within one segment's span, so the same words elsewhere in the document can't be taken for it.
pub fn locate_within(canon: &Canonical, words: &[Word], source_sha256: &str, rendition_id: &ObjectId, quote: &str, from: usize, to: usize) -> Option<Anchor> {
    let (start, end) = find_quote(canon.text.get(from..to)?, quote, "", "")?;
    Some(anchor_at(canon, words, source_sha256, rendition_id, from + start, from + end))
}

fn anchor_at(canon: &Canonical, words: &[Word], source_sha256: &str, rendition_id: &ObjectId, start: usize, end: usize) -> Anchor {
    let text = &canon.text;
    let before: String = {
        let chars: Vec<char> = text[..start].chars().rev().take(CONTEXT_CHARS).collect();
        chars.into_iter().rev().collect()
    };
    let after: String = text[end..].chars().take(CONTEXT_CHARS).collect();
    Anchor {
        source_sha256: source_sha256.into(),
        rendition_id: rendition_id.clone(),
        start: start as u64,
        end: end as u64,
        quote: text[start..end].to_string(),
        prefix: before,
        suffix: after,
        rects: rects_for(canon, words, start, end),
    }
}

/// A word table's stored form. Field order and number formats are fixed, so the same parse gives the same bytes.
pub fn word_table_bytes(doc: &ParsedDocument) -> Vec<u8> {
    serde_json::to_vec(doc).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(text: &str, page: u32, x0: i32, y0: i32, x1: i32, y1: i32) -> Word {
        Word {
            text: text.into(),
            page,
            rect: Rect { x0, y0, x1, y1 },
            confidence: None,
            quad: None,
        }
    }

    fn page_words() -> Vec<Word> {
        vec![
            w("Staden", 0, 0, 0, 500, 100),
            w("ligger", 0, 600, 0, 1100, 100),
            w("vid", 0, 0, 120, 300, 220),
            w("Göta", 0, 400, 120, 800, 220),
            w("älv.", 0, 900, 120, 1200, 220),
            w("Nytt", 0, 0, 500, 400, 600),
            w("stycke.", 0, 500, 500, 1000, 600),
            w("Sidan", 1, 0, 0, 500, 100),
        ]
    }

    #[test]
    fn canonical_text_has_spaces_lines_blocks_and_page_breaks() {
        let c = canonical(&page_words());
        assert_eq!(c.text, "Staden ligger\nvid Göta älv.\n\nNytt stycke.\n\nSidan");
        for (s, word) in c.spans.iter().zip(page_words()) {
            assert_eq!(&c.text[s.start..s.end], word.text);
        }
    }

    #[test]
    fn the_same_words_give_the_same_bytes() {
        assert_eq!(canonical(&page_words()), canonical(&page_words()));
    }

    #[test]
    fn a_new_column_starts_a_new_block() {
        let c = canonical(&[w("first", 0, 0, 1000, 100, 1100), w("second", 0, 5000, 0, 5500, 100)]);
        assert_eq!(c.text, "first\n\nsecond");
    }

    #[test]
    fn empty_input_is_empty() {
        let c = canonical(&[]);
        assert_eq!((c.text.as_str(), c.spans.len()), ("", 0));
        assert!(segments(&c, &[]).is_empty());
    }

    #[test]
    fn plain_text_normalizes_line_endings() {
        assert_eq!(canonical_plain("a\r\nb\rc").text, "a\nb\nc");
    }

    fn sentence(n: usize) -> String {
        (0..n).map(|i| format!("word{i}")).collect::<Vec<_>>().join(" ") + "."
    }

    #[test]
    fn short_blocks_join_until_the_minimum_and_long_ones_split_within_the_maximum() {
        let small = sentence(20);
        let long = (0..30).map(|_| sentence(30)).collect::<Vec<_>>().join(" ");
        let text = format!("{small}\n\n{small}\n\n{small}\n\n{small}\n\n{small}\n\n{long}");
        let c = canonical_plain(&text);
        let segs = segments(&c, &[]);
        assert!(segs.len() >= 3, "{}", segs.len());
        for s in &segs {
            let tokens = estimate_tokens(&text[s.start..s.end]);
            assert!(tokens <= MAX_TOKENS + 8, "{tokens}");
        }
        assert!(estimate_tokens(&text[segs[0].start..segs[0].end]) >= MIN_TOKENS);
        // Every word stays in exactly one segment.
        let joined: Vec<&str> = segs.iter().flat_map(|s| text[s.start..s.end].split_whitespace()).collect();
        assert_eq!(joined, text.split_whitespace().collect::<Vec<_>>());
    }

    #[test]
    fn a_long_block_with_no_sentence_ends_still_splits() {
        let text = (0..2000).map(|i| format!("w{i}")).collect::<Vec<_>>().join(" ");
        let segs = segments(&canonical_plain(&text), &[]);
        assert!(segs.len() >= 5);
        assert!(segs.iter().all(|s| estimate_tokens(&text[s.start..s.end]) <= MAX_TOKENS + 4));
    }

    #[test]
    fn a_markdown_heading_starts_a_segment_and_labels_it() {
        let text = format!("# Inledning\n\n{}\n\n## Metod\n\n{}", sentence(90), sentence(90));
        let segs = segments(&canonical_plain(&text), &[]);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].heading.as_deref(), Some("Inledning"));
        assert_eq!(segs[1].heading.as_deref(), Some("Metod"));
        assert!(!text[segs[0].start..segs[0].end].contains('#'));
    }

    #[test]
    fn a_tall_short_block_is_a_heading_in_a_pdf() {
        let mut words = vec![w("Results", 0, 0, 0, 1000, 300)];
        for i in 0..200 {
            let (line, col) = (i / 10, i % 10);
            words.push(w(&format!("w{i}"), 0, col * 300, 500 + line * 120, col * 300 + 250, 500 + line * 120 + 100));
        }
        let c = canonical(&words);
        let segs = segments(&c, &words);
        assert_eq!(segs[0].heading.as_deref(), Some("Results"));
        assert_eq!((segs[0].page_from, segs[0].page_to), (Some(0), Some(0)));
    }

    #[test]
    fn segment_offsets_on_non_ascii_text_are_valid() {
        let text = format!("{}\n\n{}", "Åäö ñ ß ".repeat(100), "€ 日本語 ".repeat(300));
        for s in segments(&canonical_plain(&text), &[]) {
            assert!(text.is_char_boundary(s.start) && text.is_char_boundary(s.end));
        }
    }

    #[test]
    fn a_quote_spanning_a_line_break_is_found() {
        let c = canonical(&page_words());
        let (a, b) = find_quote(&c.text, "ligger vid Göta", "", "").unwrap();
        assert_eq!(&c.text[a..b], "ligger\nvid Göta");
    }

    #[test]
    fn a_quote_that_appears_twice_resolves_by_prefix_and_suffix() {
        let t = "alfa pumpen väger 12 kg beta pumpen väger 12 kg gamma";
        let first = find_quote(t, "pumpen väger 12 kg", "alfa ", " beta").unwrap();
        let second = find_quote(t, "pumpen väger 12 kg", "beta ", " gamma").unwrap();
        assert_eq!(first.0, 5);
        assert_eq!(second.0, t.rfind("pumpen").unwrap());
        assert_eq!(find_quote(t, "pumpen väger 12 kg", "", "").unwrap().0, 5);
    }

    #[test]
    fn a_quote_ending_in_a_non_ascii_letter_has_a_valid_end() {
        let t = "på väg till Malmö är det långt";
        let (a, b) = find_quote(t, "till Malmö", "", "").unwrap();
        assert_eq!(&t[a..b], "till Malmö");
        let (a, b) = find_quote(t, "på", "", "").unwrap();
        assert_eq!(&t[a..b], "på");
    }

    #[test]
    fn a_missing_or_empty_quote_is_not_found() {
        assert_eq!(find_quote("abc", "xyz", "", ""), None);
        assert_eq!(find_quote("abc", "  ", "", ""), None);
        assert_eq!(find_quote("", "a", "", ""), None);
    }

    #[test]
    fn quotes_round_trip_through_rects_and_back() {
        let (words, c) = (page_words(), canonical(&page_words()));
        let a = locate(&c, &words, "sha", &ObjectId("r".into()), "ligger vid Göta", "", "").unwrap();
        assert_eq!(a.rects.len(), 2, "one rect per line");
        assert_eq!(text_in_rects(&words, &a.rects), "ligger vid Göta");
        assert_eq!((a.prefix.as_str(), a.suffix.as_str()), ("Staden ", " älv.\n\nNytt stycke.\n\nSidan"));
        assert_eq!(
            a.rects[0],
            (
                0,
                Rect {
                    x0: 600,
                    y0: 0,
                    x1: 1100,
                    y1: 100
                }
            )
        );
    }

    #[test]
    fn a_quote_across_a_page_gives_a_rect_on_each_page() {
        let (words, c) = (page_words(), canonical(&page_words()));
        let a = locate(&c, &words, "s", &ObjectId("r".into()), "stycke. Sidan", "", "").unwrap();
        assert_eq!(a.rects.iter().map(|r| r.0).collect::<Vec<_>>(), vec![0, 1]);
    }

    #[test]
    fn language_is_detected_in_the_four_tested_languages() {
        for (text, lang) in [
            (
                "The factory was moved to the coast after the fire, and the board decided that production would continue there for many years.",
                "en",
            ),
            (
                "Fabriken flyttades till kusten efter branden, och styrelsen beslutade att tillverkningen skulle fortsätta där under många år.",
                "sv",
            ),
            (
                "Das Werk zog nach dem Brand an die Küste um, und der Vorstand beschloss, dass die Fertigung dort viele Jahre weitergehen sollte.",
                "de",
            ),
            (
                "La fábrica se trasladó a la costa después del incendio, y el consejo decidió que la producción continuaría allí durante muchos años.",
                "es",
            ),
        ] {
            assert_eq!(detect_language(text).as_deref(), Some(lang), "{text}");
        }
    }

    #[test]
    fn very_short_text_is_stored_as_unknown() {
        assert_eq!(detect_language("XK-42"), None);
        assert_eq!(detect_language(""), None);
    }
}
