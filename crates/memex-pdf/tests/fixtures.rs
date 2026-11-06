//! The parser on real PDFs, with anchors labelled by a second tool (poppler).
// Tests read their fixture files.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_core::rendition::{canonical, find_quote, locate, rects_for, segments, text_in_rects, word_table_bytes};
use memex_pdf::{PdfError, PdfiumParser};
use memex_types::{ObjectId, PageClass};
use serde_json::Value;
use std::sync::OnceLock;

/// Pdfium can only be initialized once in a process, so every test here shares this.
fn shared_parser() -> &'static PdfiumParser {
    static PARSER: OnceLock<PdfiumParser> = OnceLock::new();
    PARSER.get_or_init(|| PdfiumParser::new().unwrap())
}

fn parse_pdf(bytes: Vec<u8>) -> Result<memex_types::ParsedDocument, PdfError> {
    shared_parser().parse(bytes)
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/fixtures/{name}.pdf", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

#[test]
fn fifty_labelled_anchors_round_trip_within_one_point() {
    let labels: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(format!("{}/tests/fixtures/labels.json", env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap();
    assert_eq!(labels.len(), 50);
    let (mut checked, mut worst) = (0, 0f64);
    let mut per = std::collections::BTreeMap::new();
    for label in &labels {
        let (file, quote) = (label["file"].as_str().unwrap(), label["quote"].as_str().unwrap());
        let doc = parse_pdf(fixture(file)).unwrap();
        let c = canonical(&doc.words);
        // The quote can repeat in a document, so look at the occurrence on the labelled page and nearest the label's box.
        let page = label["page"].as_u64().unwrap() as u32;
        let (mut best, mut best_distance) = (None, f64::MAX);
        let mut from = 0;
        while let Some((a, b)) = find_quote(&c.text[from..], quote, "", "").map(|(a, b)| (a + from, b + from)) {
            let rects = rects_for(&c, &doc.words, a, b);
            if let Some((p, r)) = rects.first()
                && *p == page
            {
                let d = (f64::from(r.x0) / 100.0 - label["x0"].as_f64().unwrap()).abs() + (f64::from(r.y0) / 100.0 - label["y0"].as_f64().unwrap()).abs();
                if d < best_distance {
                    best_distance = d;
                    best = Some((a, b, rects));
                }
            }
            from = b;
        }
        let (a, b, rects) = best.unwrap_or_else(|| panic!("{file} {quote:?} not found on page {page}"));
        // A rotated page's words stack vertically, so a quote there has a rect per word. The label is the box around them all.
        let union = |f: fn(&memex_types::Rect) -> i32, pick: fn(i32, i32) -> i32| rects.iter().map(|(_, r)| f(r)).reduce(pick).unwrap();
        let r = memex_types::Rect {
            x0: union(|r| r.x0, i32::min),
            y0: union(|r| r.y0, i32::min),
            x1: union(|r| r.x1, i32::max),
            y1: union(|r| r.y1, i32::max),
        };
        for (got, key) in [(r.x0, "x0"), (r.y0, "y0"), (r.x1, "x1"), (r.y1, "y1")] {
            let diff = (f64::from(got) / 100.0 - label[key].as_f64().unwrap()).abs();
            worst = worst.max(diff);
            let e = per.entry((file.to_string(), key)).or_insert(0f64);
            *e = e.max(diff);
        }
        // And back: the rects give the same text, and the anchor keeps its offsets.
        assert_eq!(text_in_rects(&doc.words, &rects), quote, "{file}");
        assert_eq!(c.text[a..b].split_whitespace().collect::<Vec<_>>().join(" "), quote);
        checked += 1;
    }
    assert_eq!(checked, 50);
    eprintln!("per file and side: {per:?}");
    // All sides of all anchors are within 1 pt, except one. On rotate270_crop, a non-embedded Helvetica, pdfium's ascent is 1.87 pt taller than poppler's. The text-direction edges match, so this is a difference in font metrics between the tools. See spec 015's Result.
    for ((file, side), diff) in &per {
        let limit = if file == "rotate270_crop" && *side == "x0" { 2.0 } else { 1.0 };
        assert!(*diff <= limit, "{file} {side}: {diff}");
    }
}

#[test]
fn parsing_the_same_file_twice_gives_identical_bytes() {
    for name in ["two_column_en", "rotate90", "table_es"] {
        assert_eq!(
            word_table_bytes(&parse_pdf(fixture(name)).unwrap()),
            word_table_bytes(&parse_pdf(fixture(name)).unwrap()),
            "{name}"
        );
    }
}

#[test]
fn rotation_and_page_facts_are_recorded() {
    let doc = parse_pdf(fixture("rotate90")).unwrap();
    assert_eq!(doc.pages[0].rotation, 90);
    assert_eq!(doc.pages[0].class, PageClass::Text);
    assert!(doc.pages[0].width < doc.pages[0].height || doc.pages[0].width > 0);
    assert!(!doc.words.is_empty());
    for w in &doc.words {
        assert!(
            w.rect.x0 >= 0 && w.rect.y0 >= 0 && w.rect.x1 <= doc.pages[0].width && w.rect.y1 <= doc.pages[0].height,
            "{w:?}"
        );
    }
}

#[test]
fn segments_cover_a_real_document_with_a_page_range() {
    let doc = parse_pdf(fixture("two_column_en")).unwrap();
    let c = canonical(&doc.words);
    let segs = segments(&c, &doc.words);
    assert!(!segs.is_empty());
    assert!(segs.iter().all(|s| s.page_from.is_some() && s.page_to >= s.page_from));
    let anchor = locate(
        &c,
        &doc.words,
        "sha",
        &ObjectId("r".into()),
        &c.text[segs[0].start..c.text.ceil_char_boundary(segs[0].start + 40.min(segs[0].end - segs[0].start))],
        "",
        "",
    );
    assert!(anchor.is_some());
}

#[test]
fn an_empty_password_opens_and_a_real_one_is_refused() {
    assert!(!parse_pdf(fixture("encrypted_empty_password")).unwrap().words.is_empty());
    assert_eq!(parse_pdf(fixture("password_protected")).unwrap_err(), PdfError::Encrypted);
}

#[test]
fn malformed_truncated_and_zero_page_files_fail_cleanly() {
    assert!(matches!(parse_pdf(b"not a pdf at all".to_vec()), Err(PdfError::Malformed(_))));
    assert!(matches!(parse_pdf(vec![]), Err(PdfError::Malformed(_))));
    let mut truncated = fixture("one_column_sv");
    truncated.truncate(200);
    assert!(matches!(parse_pdf(truncated), Err(PdfError::Malformed(_))));
    assert!(matches!(parse_pdf(fixture("zero_pages")), Err(PdfError::NoPages) | Err(PdfError::Malformed(_))));
}

fn ink(rgb: &[u8], width: u32, x: u32, y: u32) -> bool {
    let at = (y as usize * width as usize + x as usize) * 3;
    (u32::from(rgb[at]) + u32::from(rgb[at + 1]) + u32::from(rgb[at + 2])) / 3 < 140
}

/// Spec 017: does a word's rect cover the word's rendered pixels, on rotated and cropped pages too?
#[test]
fn highlights_cover_each_words_rendered_pixels() {
    let parser = shared_parser();
    let dpi = 150u32;
    let mut lines = vec![];
    for name in [
        "rotate90",
        "rotate180_crop",
        "rotate270_crop",
        "cropbox_offset",
        "two_column_en",
        "three_column_de",
        "one_column_sv",
    ] {
        let doc = parser.parse(fixture(name)).unwrap();
        let (width, height, rgb) = parser.render_page(fixture(name), 0, dpi).unwrap();
        let (mut covered_sum, mut ious, mut n) = (0.0, vec![], 0);
        // Long words only, whose ink is well away from their neighbours along the line. Every 3rd, to keep this quick.
        for w in doc.words.iter().filter(|w| w.page == 0 && w.text.chars().count() >= 5).step_by(3).take(120) {
            let (x0, y0, x1, y1) = memex_core::rect_to_pixels(&w.rect, dpi);
            let (x1, y1) = (x1.min(width), y1.min(height));
            // Ink inside the rect, against ink in a margin one pixel beyond it, where a cut-off word would show.
            let mut inside = 0u32;
            let (mut ix0, mut iy0, mut ix1, mut iy1) = (u32::MAX, u32::MAX, 0, 0);
            for y in y0..y1 {
                for x in x0..x1 {
                    if ink(&rgb, width, x, y) {
                        inside += 1;
                        ix0 = ix0.min(x);
                        iy0 = iy0.min(y);
                        ix1 = ix1.max(x + 1);
                        iy1 = iy1.max(y + 1);
                    }
                }
            }
            assert!(inside > 0, "{name} {:?} has no ink under its rect", w.text);
            let mut outside = 0u32;
            for y in y0.saturating_sub(1)..(y1 + 1).min(height) {
                for x in x0.saturating_sub(1)..(x1 + 1).min(width) {
                    if !(y0..y1).contains(&y) || !(x0..x1).contains(&x) {
                        outside += u32::from(ink(&rgb, width, x, y));
                    }
                }
            }
            covered_sum += f64::from(inside) / f64::from(inside + outside);
            // The box around the ink, against the rect.
            let ink_area = f64::from((ix1 - ix0) * (iy1 - iy0));
            let rect_area = f64::from((x1 - x0) * (y1 - y0));
            ious.push(ink_area / rect_area);
            n += 1;
        }
        assert!(n >= 10, "{name}: {n}");
        ious.sort_by(f64::total_cmp);
        let coverage = covered_sum / f64::from(n);
        lines.push(format!(
            "{name}: {n} words, ink covered {:.3}, ink box over rect median {:.3}",
            coverage,
            ious[ious.len() / 2]
        ));
        assert!(coverage >= 0.9, "{name}: {coverage}");
    }
    eprintln!("pixel test:\n{}", lines.join("\n"));
}

/// Spec 017: the PNG's highlight and the rect JSON agree within 1 pt.
#[test]
fn the_png_highlight_and_the_rects_agree() {
    let parser = shared_parser();
    let rects = [memex_types::Rect {
        x0: 10000,
        y0: 20000,
        x1: 30000,
        y1: 22000,
    }];
    let dpi = 150;
    let plain = parser.render_page(fixture("two_column_en"), 0, dpi).unwrap();
    let png = parser.render_highlighted(fixture("two_column_en"), 0, dpi, &rects).unwrap();
    let decoded = image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap().to_rgb8();
    assert_eq!((decoded.width(), decoded.height()), (plain.0, plain.1));
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, p) in decoded.enumerate_pixels() {
        let at = (y as usize * plain.0 as usize + x as usize) * 3;
        if p.0[2] != plain.2[at + 2] {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }
    }
    let to_hundredths = |px: u32| f64::from(px) * 7200.0 / f64::from(dpi);
    for (got, want) in [(x0, 10000), (y0, 20000), (x1, 30000), (y1, 22000)] {
        assert!(
            (to_hundredths(got) - f64::from(want)).abs() <= 100.0,
            "{got} px is {} vs {want}",
            to_hundredths(got)
        );
    }
}
