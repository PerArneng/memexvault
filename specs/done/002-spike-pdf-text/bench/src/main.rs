//! Spec 002 throwaway bench. Each run handles one PDF in its own process so a parent can time out hangs and record panics.
//! words <oxide|pdfium|lopdf> <pdf>: JSON {page: [ {t,x0,y0,x1,y1} ]} in displayed coordinates, plus page sizes and time.
//! render <hayro|oxide|pdfium> <pdf> <dpi> <outdir>: one PNG per page (first `MAX_PAGES` pages).
use serde_json::{json, Value};
use std::time::Instant;

const MAX_PAGES: usize = 5;

/// Map a raw user-space rectangle to displayed coordinates: origin top-left of the crop box, after /Rotate, y down.
fn displayed(raw: (f32, f32, f32, f32), crop: (f32, f32, f32, f32), rot: i32) -> (f32, f32, f32, f32) {
    let (x0, y0, x1, y1) = raw;
    let (cx0, cy0, cx1, cy1) = crop;
    let (a, b, c, d) = match rot.rem_euclid(360) {
        90 => (y0 - cy0, x0 - cx0, y1 - cy0, x1 - cx0),
        180 => (cx1 - x1, y0 - cy0, cx1 - x0, y1 - cy0),
        270 => (cy1 - y1, cx1 - x1, cy1 - y0, cx1 - x0),
        _ => (x0 - cx0, cy1 - y1, x1 - cx0, cy1 - y0),
    };
    (a.min(c), b.min(d), a.max(c), b.max(d))
}

fn word(t: &str, r: (f32, f32, f32, f32)) -> Value {
    json!({"t": t, "x0": r.0, "y0": r.1, "x1": r.2, "y1": r.3})
}

fn oxide_words(bytes: Vec<u8>) -> Value {
    let doc = pdf_oxide::PdfDocument::from_bytes(bytes).expect("open");
    let _ = doc.authenticate(b"");
    let n = doc.page_count().expect("pages");
    let mut pages = vec![];
    for i in 0..n {
        let info = doc.get_page_info(i).expect("info");
        let m = info.media_box;
        let c = info.crop_box.unwrap_or(m);
        let crop = (c.x, c.y, c.x + c.width, c.y + c.height);
        let rot = info.rotation;
        let words = doc.extract_words(i).expect("words");
        let list: Vec<Value> = words
            .iter()
            .map(|w| {
                let b = w.bbox;
                word(&w.text, displayed((b.x, b.y, b.x + b.width, b.y + b.height), crop, rot))
            })
            .collect();
        pages.push(json!({"words": list}));
    }
    json!({"pages": pages})
}

fn pdfium_lib() -> pdfium_render::prelude::Pdfium {
    use pdfium_render::prelude::*;
    let dir = std::env::var("PDFIUM_DIR").expect("PDFIUM_DIR");
    Pdfium::new(Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(&dir)).expect("bind"))
}

fn pdfium_words(bytes: Vec<u8>) -> Value {
    use pdfium_render::prelude::*;
    let pdfium = pdfium_lib();
    let doc = pdfium.load_pdf_from_byte_vec(bytes, Some("")).expect("open");
    let mut pages = vec![];
    for page in doc.pages().iter() {
        let text = page.text().expect("text");
        let w = page.width().value as i32 * 100;
        let h = page.height().value as i32 * 100;
        let cfg = PdfRenderConfig::new().set_target_size(w, h);
        let to = |x: PdfPoints, y: PdfPoints| page.points_to_pixels(x, y, &cfg).expect("pt");
        let mut list = vec![];
        let (mut cur, mut bx): (String, Option<(f32, f32, f32, f32)>) = (String::new(), None);
        let mut prev: Option<(f32, f32, f32, f32)> = None;
        let flush = |cur: &mut String, bx: &mut Option<(f32, f32, f32, f32)>, list: &mut Vec<Value>| {
            if let Some(b) = bx.take() {
                list.push(word(cur, b));
            }
            cur.clear();
        };
        for ch in text.chars().iter() {
            let s = ch.unicode_string().unwrap_or_default();
            if s.chars().all(char::is_whitespace) {
                flush(&mut cur, &mut bx, &mut list);
                continue;
            }
            let Ok(r) = ch.loose_bounds() else { continue };
            let hyphen = s == "\u{2}";
            let (ax, ay) = to(r.left(), r.top());
            let (bx2, by2) = to(r.right(), r.bottom());
            let q = (ax.min(bx2) as f32 / 100.0, ay.min(by2) as f32 / 100.0, ax.max(bx2) as f32 / 100.0, ay.max(by2) as f32 / 100.0);
            // Characters of one word share a line, in the horizontal or the vertical direction. Neither means a line break.
            if let Some(o) = prev {
                let over_x = o.0 < q.2 && q.0 < o.2;
                let over_y = o.1 < q.3 && q.1 < o.3;
                if !over_x && !over_y {
                    flush(&mut cur, &mut bx, &mut list);
                }
            }
            prev = Some(q);
            bx = Some(match bx {
                None => q,
                Some(o) => (o.0.min(q.0), o.1.min(q.1), o.2.max(q.2), o.3.max(q.3)),
            });
            if hyphen {
                cur.push('-');
                flush(&mut cur, &mut bx, &mut list);
            } else {
                cur.push_str(&s);
            }
        }
        flush(&mut cur, &mut bx, &mut list);
        pages.push(json!({"words": list}));
    }
    json!({"pages": pages})
}

/// Facts for routing a page to OCR: characters, visible text objects, hidden text objects and image coverage.
fn pdfium_classify(bytes: Vec<u8>) -> Value {
    use pdfium_render::prelude::*;
    let pdfium = pdfium_lib();
    let doc = pdfium.load_pdf_from_byte_vec(bytes, Some("")).expect("open");
    let mut pages = vec![];
    for page in doc.pages().iter() {
        let area = page.width().value * page.height().value;
        let (mut visible, mut hidden, mut image_area) = (0, 0, 0.0f32);
        for o in page.objects().iter() {
            match o.as_text_object() {
                Some(t) => {
                    if matches!(t.render_mode(), PdfPageTextRenderMode::Invisible) { hidden += 1 } else { visible += 1 }
                }
                None => {
                    if o.as_image_object().is_some() {
                        if let Ok(b) = o.bounds() { image_area += b.width().value * b.height().value; }
                    }
                }
            }
        }
        let chars = page.text().map(|t| t.chars().len()).unwrap_or(0);
        let class = if visible == 0 && image_area / area > 0.5 { "image-only" } else if visible == 0 && chars == 0 { "blank" } else { "text" };
        pages.push(json!({"chars": chars, "visible": visible, "hidden": hidden, "image_share": image_area / area, "class": class}));
    }
    json!({"pages": pages})
}

fn lopdf_words(bytes: Vec<u8>) -> Value {
    // lopdf has text extraction but no word coordinates: words only, with no boxes.
    let mut doc = lopdf::Document::load_mem(&bytes).expect("open");
    if doc.is_encrypted() {
        doc.decrypt("").expect("decrypt");
    }
    let mut pages = vec![];
    for (n, _) in doc.get_pages() {
        let text = doc.extract_text(&[n]).unwrap_or_default();
        let list: Vec<Value> = text.split_whitespace().map(|t| json!({"t": t})).collect();
        pages.push(json!({"words": list}));
    }
    json!({"pages": pages})
}

fn render_hayro(bytes: Vec<u8>, dpi: f32, out: &str) -> usize {
    use hayro::hayro_interpret::InterpreterSettings;
    use hayro::vello_cpu::color::palette::css::WHITE;
    let pdf = hayro::hayro_syntax::Pdf::new(bytes).expect("open");
    let cache = hayro::RenderCache::new();
    let s = hayro::PixmapSettings { x_scale: dpi / 72.0, y_scale: dpi / 72.0, bg_color: WHITE };
    let is = InterpreterSettings::default();
    let rs = hayro::RenderSettings::default();
    let mut n = 0;
    for (i, page) in pdf.pages().iter().enumerate().take(MAX_PAGES) {
        let p = hayro::render(page, &cache, &is, &rs, &s);
        std::fs::write(format!("{out}/p{i}.png"), p.into_png().expect("png")).expect("write");
        n += 1;
    }
    n
}

fn render_oxide(bytes: Vec<u8>, dpi: u32, out: &str) -> usize {
    let doc = pdf_oxide::PdfDocument::from_bytes(bytes).expect("open");
    let _ = doc.authenticate(b"");
    let n = doc.page_count().expect("pages").min(MAX_PAGES);
    for i in 0..n {
        let o = pdf_oxide::rendering::RenderOptions::with_dpi(dpi);
        let img = pdf_oxide::rendering::render_page(&doc, i, &o).expect("render");
        std::fs::write(format!("{out}/p{i}.png"), img.data).expect("write");
    }
    n
}

fn render_pdfium(bytes: Vec<u8>, dpi: f32, out: &str) -> usize {
    use pdfium_render::prelude::*;
    let pdfium = pdfium_lib();
    let doc = pdfium.load_pdf_from_byte_vec(bytes, Some("")).expect("open");
    let mut n = 0;
    for (i, page) in doc.pages().iter().enumerate().take(MAX_PAGES) {
        let cfg = PdfRenderConfig::new().scale_page_by_factor(dpi / 72.0);
        let img = page.render_with_config(&cfg).expect("render").as_image().expect("image");
        img.into_rgb8().save(format!("{out}/p{i}.png")).expect("save");
        n += 1;
    }
    n
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[3]).expect("read");
    let t = Instant::now();
    match a[1].as_str() {
        "words" => {
            let mut v = match a[2].as_str() {
                "oxide" => oxide_words(bytes),
                "pdfium" => pdfium_words(bytes),
                "lopdf" => lopdf_words(bytes),
                c => panic!("{c}"),
            };
            v["seconds"] = json!(t.elapsed().as_secs_f64());
            println!("{v}");
        }
        "classify" => println!("{}", pdfium_classify(bytes)),
        "render" => {
            let dpi: u32 = a[4].parse().expect("dpi");
            let out = &a[5];
            std::fs::create_dir_all(out).expect("mkdir");
            let n = match a[2].as_str() {
                "hayro" => render_hayro(bytes, dpi as f32, out),
                "oxide" => render_oxide(bytes, dpi, out),
                "pdfium" => render_pdfium(bytes, dpi as f32, out),
                c => panic!("{c}"),
            };
            println!("{}", json!({"pages": n, "seconds": t.elapsed().as_secs_f64()}));
        }
        c => panic!("{c}"),
    }
}
