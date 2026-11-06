//! Routing scanned pages to OCR, turning OCR output into words, and spotting hidden text. All pure.
use memex_types::{PageClass, PageFacts, PageFlag, PageOutcome, PageRoute, ParsedDocument, Rect, Word};
use serde::{Deserialize, Serialize};

/// One line of text as an OCR engine reports it, in pixels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrRegion {
    pub text: String,
    pub confidence: f32,
    /// Four corners, clockwise from top left.
    pub poly: [(f32, f32); 4],
    /// One box per character of `text`, spaces included, when the engine gives them.
    pub char_boxes: Option<Vec<[f32; 4]>>,
}

/// What an OCR worker returns for a page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrPage {
    pub dpi: u32,
    pub regions: Vec<OcrRegion>,
}

/// Pages rendered for OCR use this, unless the page is so large that memory would suffer.
pub const OCR_DPI: u32 = 300;
/// The longest side we render, which bounds memory on a huge page.
pub const MAX_RENDER_SIDE_PX: u32 = 6000;
/// Below this mean confidence a page is marked, and claims from it count for less.
pub const LOW_CONFIDENCE_PERMILLE: u16 = 800;

/// The render resolution for a page of this size in 1/100 pt.
pub fn dpi_for(width: i32, height: i32) -> u32 {
    let longest_pt = (width.max(height).max(1) as f64) / 100.0;
    let at_full = longest_pt * f64::from(OCR_DPI) / 72.0;
    if at_full <= f64::from(MAX_RENDER_SIDE_PX) {
        OCR_DPI
    } else {
        ((f64::from(MAX_RENDER_SIDE_PX) * 72.0 / longest_pt) as u32).max(1)
    }
}

/// A text layer full of unmapped or replacement characters can't be trusted, however present it looks.
pub fn text_layer_is_doubtful(words: &[&Word]) -> bool {
    let (mut total, mut bad) = (0usize, 0usize);
    for w in words {
        for c in w.text.chars() {
            total += 1;
            let private_use = ('\u{e000}'..='\u{f8ff}').contains(&c);
            if c == '\u{fffd}' || private_use || c.is_control() {
                bad += 1;
            }
        }
    }
    total >= 20 && bad * 5 > total
}

/// Which way each page goes. An image-only page and a doubtful text layer go to OCR, and an empty page goes nowhere.
pub fn route_pages(doc: &ParsedDocument) -> Vec<PageRoute> {
    doc.pages
        .iter()
        .map(|p: &PageFacts| {
            let words: Vec<&Word> = doc.words.iter().filter(|w| w.page == p.index).collect();
            match p.class {
                PageClass::Blank => PageRoute::Blank,
                PageClass::ImageOnly if words.is_empty() => PageRoute::Ocr,
                _ if text_layer_is_doubtful(&words) => PageRoute::Ocr,
                _ if words.is_empty() => PageRoute::Blank,
                _ => PageRoute::TextLayer,
            }
        })
        .collect()
}

/// How many text pages get compared against OCR for hidden text: every page declaring invisible text objects, plus a sample of the rest.
pub const HIDDEN_CHECK_SAMPLE_PAGES: usize = 3;
pub const HIDDEN_CHECK_EVERY_NTH: usize = 10;

/// The text-layer pages to check. Deterministic for a document: the first few, every tenth, and any with invisible objects.
pub fn hidden_check_pages(doc: &ParsedDocument, routes: &[PageRoute]) -> Vec<u32> {
    let mut text_seen = 0;
    let mut out = vec![];
    for (p, route) in doc.pages.iter().zip(routes) {
        if *route != PageRoute::TextLayer {
            continue;
        }
        if p.hidden_text_objects > 0 || text_seen < HIDDEN_CHECK_SAMPLE_PAGES || text_seen % HIDDEN_CHECK_EVERY_NTH == 0 {
            out.push(p.index);
        }
        text_seen += 1;
    }
    out
}

fn to_hundredths_pt(px: f32, dpi: u32) -> i32 {
    (f64::from(px) * 7200.0 / f64::from(dpi)).round() as i32
}

fn rect_of_poly(poly: &[(f32, f32); 4], dpi: u32) -> Rect {
    let xs = poly.iter().map(|p| p.0);
    let ys = poly.iter().map(|p| p.1);
    Rect {
        x0: to_hundredths_pt(xs.clone().fold(f32::MAX, f32::min), dpi),
        y0: to_hundredths_pt(ys.clone().fold(f32::MAX, f32::min), dpi),
        x1: to_hundredths_pt(xs.fold(f32::MIN, f32::max), dpi),
        y1: to_hundredths_pt(ys.fold(f32::MIN, f32::max), dpi),
    }
}

/// Lines in reading order: split into columns where a vertical gap runs down the page, then top to bottom within each.
pub fn reading_order(regions: &mut [OcrRegion]) {
    if regions.is_empty() {
        return;
    }
    let span = |r: &OcrRegion| {
        let xs = r.poly.iter().map(|p| p.0);
        (xs.clone().fold(f32::MAX, f32::min), xs.fold(f32::MIN, f32::max))
    };
    // Merge horizontal extents. Lines in one column overlap, and columns don't. A line spanning columns, such as a title, joins them, so a page with one is treated as one column below it only if nothing separates it.
    let mut spans: Vec<(f32, f32)> = regions.iter().map(span).collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut columns: Vec<(f32, f32)> = vec![];
    for (a, b) in spans {
        match columns.last_mut() {
            Some(c) if a <= c.1 => c.1 = c.1.max(b),
            _ => columns.push((a, b)),
        }
    }
    let column_of = |r: &OcrRegion| -> usize {
        let (a, _) = span(r);
        columns.iter().position(|c| a >= c.0 && a <= c.1).unwrap_or(0)
    };
    let top = |r: &OcrRegion| r.poly.iter().map(|p| p.1).fold(f32::MAX, f32::min);
    regions.sort_by(|p, q| {
        column_of(p)
            .cmp(&column_of(q))
            .then(top(p).total_cmp(&top(q)))
            .then(span(p).0.total_cmp(&span(q).0))
    });
}

/// Words from an OCR page: grouped from character boxes at the spaces, or the whole line when there are none. The line's corners stay as a quad, for skewed text.
pub fn words_from_ocr(page: u32, ocr: &OcrPage) -> Vec<Word> {
    let mut regions = ocr.regions.clone();
    reading_order(&mut regions);
    let mut out = vec![];
    let dpi = ocr.dpi;
    for r in &regions {
        let confidence = Some((r.confidence.clamp(0.0, 1.0) * 1000.0).round() as u16);
        let quad = Some(r.poly.map(|(x, y)| (to_hundredths_pt(x, dpi), to_hundredths_pt(y, dpi))));
        let chars: Vec<char> = r.text.chars().collect();
        match &r.char_boxes {
            Some(boxes) if boxes.len() == chars.len() => {
                let (mut current, mut bounds): (String, Option<[f32; 4]>) = (String::new(), None);
                let mut flush = |current: &mut String, bounds: &mut Option<[f32; 4]>| {
                    if let Some(b) = bounds.take() {
                        out.push(Word {
                            text: std::mem::take(current),
                            page,
                            rect: Rect {
                                x0: to_hundredths_pt(b[0], dpi),
                                y0: to_hundredths_pt(b[1], dpi),
                                x1: to_hundredths_pt(b[2], dpi),
                                y1: to_hundredths_pt(b[3], dpi),
                            },
                            confidence,
                            quad,
                        });
                    }
                };
                for (c, b) in chars.iter().zip(boxes) {
                    if c.is_whitespace() {
                        flush(&mut current, &mut bounds);
                        continue;
                    }
                    current.push(*c);
                    bounds = Some(match bounds {
                        None => *b,
                        Some(o) => [o[0].min(b[0]), o[1].min(b[1]), o[2].max(b[2]), o[3].max(b[3])],
                    });
                }
                flush(&mut current, &mut bounds);
            }
            _ => {
                if !r.text.trim().is_empty() {
                    out.push(Word {
                        text: r.text.trim().to_string(),
                        page,
                        rect: rect_of_poly(&r.poly, dpi),
                        confidence,
                        quad,
                    });
                }
            }
        }
    }
    out
}

pub fn mean_confidence(words: &[Word]) -> Option<u16> {
    let known: Vec<u32> = words.iter().filter_map(|w| w.confidence.map(u32::from)).collect();
    if known.is_empty() {
        None
    } else {
        Some((known.iter().sum::<u32>() / known.len() as u32) as u16)
    }
}

fn normalized(t: &str) -> String {
    t.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            // `prev` is the diagonal: the old value of row[j], before this pass overwrote it.
            let next = (prev + cost).min(row[j] + 1).min(row[j + 1] + 1);
            prev = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1
}

/// Text-layer words OCR can't see: no OCR text overlaps them, or what overlaps reads differently. OCR splits and merges words, so all overlapping OCR text is joined before comparing.
pub fn hidden_words(layer: &[&Word], ocr: &[Word]) -> usize {
    let mut hidden = 0;
    for w in layer {
        let want = normalized(&w.text);
        if want.chars().count() < 2 {
            continue;
        }
        let seen: String = ocr.iter().filter(|o| overlaps(&o.rect, &w.rect)).map(|o| normalized(&o.text)).collect();
        let readable = if seen.contains(&want) {
            true
        } else {
            !seen.is_empty() && edit_distance(&want, &seen) * 2 <= want.chars().count().max(seen.chars().count())
        };
        if !readable {
            hidden += 1;
        }
    }
    hidden
}

/// Flag a page when at least this many words are invisible, and they're at least this share of it. OCR misreads a few words on any page.
pub const HIDDEN_MIN_WORDS: usize = 3;
pub const HIDDEN_MIN_PERCENT: usize = 5;

/// Words with no real size, which show up in a text layer but never on the page.
pub fn zero_size_words(layer: &[&Word]) -> usize {
    layer
        .iter()
        .filter(|w| !normalized(&w.text).is_empty() && (w.rect.y1 - w.rect.y0 < 100 || w.rect.x1 <= w.rect.x0))
        .count()
}

/// Zero-width characters, Unicode tags and bidi overrides, which hide text from a reader but not from a model.
pub fn invisible_unicode(layer: &[&Word]) -> usize {
    layer
        .iter()
        .flat_map(|w| w.text.chars())
        .filter(|c| matches!(*c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{feff}' | '\u{e0000}'..='\u{e007f}'))
        .count()
}

/// Everything a rendition records about one page. `ocr_words` is what OCR read on it, if it looked.
pub fn page_outcome(index: u32, route: PageRoute, layer: &[&Word], ocr_words: Option<&[Word]>, ocr_failure: Option<&str>) -> PageOutcome {
    let mut flags = vec![];
    if let Some(reason) = ocr_failure {
        flags.push(PageFlag::OcrFailed { reason: reason.into() });
    }
    let zero = zero_size_words(layer);
    if zero > 0 {
        flags.push(PageFlag::ZeroSizeText { words: zero as u32 });
    }
    let invisible = invisible_unicode(layer);
    if invisible > 0 {
        flags.push(PageFlag::InvisibleUnicode { chars: invisible as u32 });
    }
    if let (PageRoute::TextLayer, Some(ocr)) = (route, ocr_words) {
        let hidden = hidden_words(layer, ocr);
        if hidden >= HIDDEN_MIN_WORDS && hidden * 100 >= layer.len().max(1) * HIDDEN_MIN_PERCENT {
            flags.push(PageFlag::HiddenText { words: hidden as u32 });
        }
    }
    let mean_confidence_permille = match route {
        PageRoute::Ocr => ocr_words.and_then(mean_confidence),
        _ => None,
    };
    if mean_confidence_permille.is_some_and(|m| m < LOW_CONFIDENCE_PERMILLE) {
        flags.push(PageFlag::LowConfidence);
    }
    PageOutcome {
        index,
        route,
        mean_confidence_permille,
        flags,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(text: &str, x0: i32, y0: i32) -> Word {
        Word {
            text: text.into(),
            page: 0,
            rect: Rect {
                x0,
                y0,
                x1: x0 + 500,
                y1: y0 + 200,
            },
            confidence: None,
            quad: None,
        }
    }

    fn page(index: u32, class: PageClass, hidden: u32) -> PageFacts {
        PageFacts {
            index,
            width: 60000,
            height: 80000,
            rotation: 0,
            class,
            hidden_text_objects: hidden,
        }
    }

    fn doc(pages: Vec<PageFacts>, words: Vec<Word>) -> ParsedDocument {
        ParsedDocument {
            parser: "p".into(),
            pages,
            words,
        }
    }

    #[test]
    fn pages_route_by_what_they_hold() {
        let d = doc(
            vec![page(0, PageClass::Text, 0), page(1, PageClass::ImageOnly, 0), page(2, PageClass::Blank, 0)],
            vec![w("ord", 0, 0)],
        );
        assert_eq!(route_pages(&d), vec![PageRoute::TextLayer, PageRoute::Ocr, PageRoute::Blank]);
    }

    #[test]
    fn a_garbled_text_layer_goes_to_ocr() {
        let garbled: Vec<Word> = (0..30).map(|i| w("\u{fffd}\u{e001}x", i * 600, 0)).collect();
        assert_eq!(route_pages(&doc(vec![page(0, PageClass::Text, 0)], garbled)), vec![PageRoute::Ocr]);
        let fine: Vec<Word> = (0..30).map(|i| w("vanliga", i * 600, 0)).collect();
        assert_eq!(route_pages(&doc(vec![page(0, PageClass::Text, 0)], fine)), vec![PageRoute::TextLayer]);
    }

    #[test]
    fn an_image_page_with_an_ocr_layer_keeps_it() {
        let d = doc(vec![page(0, PageClass::ImageOnly, 40)], vec![w("hidden", 0, 0)]);
        assert_eq!(route_pages(&d), vec![PageRoute::TextLayer]);
    }

    #[test]
    fn the_hidden_check_takes_a_sample_and_every_page_declaring_invisible_text() {
        let mut pages: Vec<PageFacts> = (0..50).map(|i| page(i, PageClass::Text, 0)).collect();
        pages[17].hidden_text_objects = 2;
        let routes = [PageRoute::TextLayer; 50];
        let got = hidden_check_pages(&doc(pages, vec![]), &routes);
        assert_eq!(got, vec![0, 1, 2, 10, 17, 20, 30, 40]);
    }

    #[test]
    fn huge_pages_get_a_lower_resolution_so_memory_stays_bounded() {
        assert_eq!(dpi_for(61200, 79200), 300);
        let huge = dpi_for(1_440_000, 1_440_000);
        assert!(huge < 300 && f64::from(huge) * 14400.0 / 72.0 <= f64::from(MAX_RENDER_SIDE_PX) + 1.0);
        assert!(dpi_for(100_000_000, 10) >= 1);
    }

    fn region(text: &str, x: f32, y: f32, w_px: f32) -> OcrRegion {
        OcrRegion {
            text: text.into(),
            confidence: 0.9,
            poly: [(x, y), (x + w_px, y), (x + w_px, y + 30.0), (x, y + 30.0)],
            char_boxes: None,
        }
    }

    #[test]
    fn two_columns_read_down_the_first_and_then_the_second() {
        let mut r = vec![
            region("c2-a", 700.0, 100.0, 400.0),
            region("c1-b", 100.0, 200.0, 400.0),
            region("c2-b", 700.0, 200.0, 400.0),
            region("c1-a", 100.0, 100.0, 400.0),
        ];
        reading_order(&mut r);
        assert_eq!(r.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(), vec!["c1-a", "c1-b", "c2-a", "c2-b"]);
    }

    #[test]
    fn one_column_reads_top_to_bottom() {
        let mut r = vec![region("b", 100.0, 200.0, 400.0), region("a", 120.0, 100.0, 300.0)];
        reading_order(&mut r);
        assert_eq!(r[0].text, "a");
        reading_order(&mut []);
    }

    #[test]
    fn words_group_at_spaces_and_convert_to_points() {
        let mut r = region("ab c", 0.0, 0.0, 400.0);
        r.char_boxes = Some(vec![
            [0.0, 0.0, 10.0, 30.0],
            [10.0, 0.0, 20.0, 30.0],
            [20.0, 0.0, 30.0, 30.0],
            [30.0, 0.0, 40.0, 30.0],
        ]);
        let words = words_from_ocr(3, &OcrPage { dpi: 300, regions: vec![r] });
        assert_eq!(words.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(), vec!["ab", "c"]);
        // 300 px at 300 dpi is 72 pt, or 7200 in 1/100 pt. So 20 px is 480.
        assert_eq!((words[0].page, words[0].rect.x1), (3, 480));
        assert_eq!(words[0].confidence, Some(900));
        assert!(words[0].quad.is_some());
    }

    #[test]
    fn a_line_with_no_character_boxes_stays_one_word() {
        let words = words_from_ocr(
            0,
            &OcrPage {
                dpi: 150,
                regions: vec![region("hela raden", 0.0, 0.0, 150.0)],
            },
        );
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].rect.x1, 7200);
    }

    #[test]
    fn mismatched_character_boxes_fall_back_to_the_line() {
        let mut r = region("abc", 0.0, 0.0, 100.0);
        r.char_boxes = Some(vec![[0.0, 0.0, 1.0, 1.0]]);
        assert_eq!(words_from_ocr(0, &OcrPage { dpi: 300, regions: vec![r] })[0].text, "abc");
    }

    #[test]
    fn hidden_words_are_those_ocr_can_not_read() {
        let layer = [w("visible", 0, 0), w("secret", 0, 1000), w("instructions", 0, 2000)];
        let refs: Vec<&Word> = layer.iter().collect();
        assert_eq!(hidden_words(&refs, &[w("visible", 0, 0)]), 2);
        assert_eq!(
            hidden_words(&refs, &[w("visib1e", 0, 0), w("secret", 0, 1000), w("instructions", 0, 2000)]),
            0,
            "OCR's small errors aren't hiding"
        );
        assert_eq!(hidden_words(&refs, &[w("visible", 0, 0), w("totally", 0, 1000), w("different", 0, 2000)]), 2);
    }

    #[test]
    fn edit_distance_counts_each_kind_of_change() {
        for (a, b, want) in [
            ("", "", 0),
            ("abc", "abc", 0),
            ("visible", "visib1e", 1),
            ("abc", "", 3),
            ("", "abc", 3),
            ("kitten", "sitting", 3),
            ("göteborg", "goteborg", 1),
            ("abc", "abdc", 1),
            ("abdc", "abc", 1),
        ] {
            assert_eq!(edit_distance(a, b), want, "{a} {b}");
        }
    }

    #[test]
    fn ocr_splitting_a_word_is_not_hiding() {
        let layer = [w("Göteborg", 0, 0)];
        let refs: Vec<&Word> = layer.iter().collect();
        assert_eq!(hidden_words(&refs, &[w("Göte", 0, 0), w("borg", 100, 0)]), 0);
    }

    #[test]
    fn a_page_is_flagged_for_a_real_amount_of_hidden_text_but_not_for_a_misread_or_two() {
        let layer: Vec<Word> = (0..100).map(|i| w(&format!("ord{i}x"), 0, i * 300)).collect();
        let refs: Vec<&Word> = layer.iter().collect();
        let all: Vec<Word> = layer.clone();
        let flagged = |ocr: &[Word]| page_outcome(0, PageRoute::TextLayer, &refs, Some(ocr), None).flags;
        assert!(flagged(&all).is_empty());
        assert!(flagged(&all[2..]).is_empty(), "two misses out of a hundred");
        assert!(matches!(flagged(&all[10..])[0], PageFlag::HiddenText { words: 10 }));
    }

    #[test]
    fn zero_size_text_and_invisible_unicode_are_flagged() {
        let mut tiny = w("instruktion", 0, 0);
        tiny.rect.y1 = tiny.rect.y0 + 10;
        let layer = [tiny, w("nor\u{200b}mal", 0, 500), w("\u{e0041}\u{e0042}", 0, 1000)];
        let refs: Vec<&Word> = layer.iter().collect();
        let flags = page_outcome(0, PageRoute::TextLayer, &refs, None, None).flags;
        assert!(flags.contains(&PageFlag::ZeroSizeText { words: 1 }));
        assert!(flags.contains(&PageFlag::InvisibleUnicode { chars: 3 }));
    }

    #[test]
    fn low_confidence_and_failed_pages_are_marked() {
        let mut low = w("suddigt", 0, 0);
        low.confidence = Some(500);
        let out = page_outcome(0, PageRoute::Ocr, &[], Some(&[low]), None);
        assert_eq!(out.mean_confidence_permille, Some(500));
        assert!(out.flags.contains(&PageFlag::LowConfidence));
        let failed = page_outcome(1, PageRoute::Ocr, &[], None, Some("timed out"));
        assert_eq!(failed.flags, vec![PageFlag::OcrFailed { reason: "timed out".into() }]);
    }
}
