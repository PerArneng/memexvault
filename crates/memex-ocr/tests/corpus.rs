//! Reads spec 003's corpus through the product's engine and writes that spike's output format, so its scorer can compare them. Run by hand.
// This test reads images and writes its results.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_ocr::Engine;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

#[test]
#[ignore = "needs MEMEX_TEST_MODELS, OCR_LIST and OCR_OUT"]
fn read_the_corpus() {
    let engine = Engine::new(Path::new(&std::env::var("MEMEX_TEST_MODELS").unwrap()), 4).unwrap();
    let list = std::fs::read_to_string(std::env::var("OCR_LIST").unwrap()).unwrap();
    let mut out = std::fs::File::create(std::env::var("OCR_OUT").unwrap()).unwrap();
    let (mut total, mut n) = (0.0, 0);
    for path in list.lines() {
        let image = image::open(path).unwrap().to_rgb8();
        let (w, h) = image.dimensions();
        let t = Instant::now();
        let page = engine.read(w, h, image.into_raw(), 300).unwrap();
        let s = t.elapsed().as_secs_f64();
        total += s;
        n += 1;
        let regions: Vec<String> = page
            .regions
            .iter()
            .map(|r| {
                let poly: Vec<String> = r.poly.iter().map(|(x, y)| format!("[{x},{y}]")).collect();
                format!(
                    "{{\"t\":{:?},\"c\":{},\"poly\":[{}],\"word_boxes\":{}}}",
                    r.text,
                    r.confidence,
                    poly.join(","),
                    r.char_boxes.as_ref().map_or(0, Vec::len)
                )
            })
            .collect();
        writeln!(out, "{{\"path\":{path:?},\"seconds\":{s},\"regions\":[{}]}}", regions.join(",")).unwrap();
    }
    writeln!(
        out,
        "{{\"summary\":true,\"pages\":{n},\"seconds\":{total},\"ppm\":{}}}",
        f64::from(n) * 60.0 / total
    )
    .unwrap();
    println!("{n} pages, {:.1} pages/min", f64::from(n) * 60.0 / total);
}
