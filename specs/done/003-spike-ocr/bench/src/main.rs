//! Spec 003 throwaway bench. Usage: ocr-bench <config> <threads> <list-file> (run with cwd = models dir).
//! Prints one JSON line per image, then a final summary line with seconds and pages/min.
use oar_ocr::oarocr::{OAROCRBuilder, PpOcrV6Size};
use serde_json::json;
use oar_ocr::domain::tasks::text_detection::TextDetectionConfig;
use std::time::Instant;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (config, threads, list) = (a[1].as_str(), a[2].parse::<usize>().expect("threads"), &a[3]);
    let cfg = oar_ocr::core::config::OrtSessionConfig::new().with_intra_threads(threads).with_inter_threads(1);
    let b = match config {
        "v5m" => OAROCRBuilder::new("pp-ocrv5_mobile_det.onnx", "latin_pp-ocrv5_mobile_rec.onnx", "ppocrv5_latin_dict.txt"),
        "v5s" => OAROCRBuilder::new("pp-ocrv5_server_det.onnx", "latin_pp-ocrv5_mobile_rec.onnx", "ppocrv5_latin_dict.txt"),
        "v6s_l" => OAROCRBuilder::new("pp-ocrv6_small_det.onnx", "latin_pp-ocrv5_mobile_rec.onnx", "ppocrv5_latin_dict.txt"),
        "v6m_l" => OAROCRBuilder::new("pp-ocrv6_medium_det.onnx", "latin_pp-ocrv5_mobile_rec.onnx", "ppocrv5_latin_dict.txt"),
        "v6s_lp" => OAROCRBuilder::new("pp-ocrv6_small_det.onnx", "latin_pp-ocrv5_mobile_rec.onnx", "ppocrv5_latin_dict.txt")
            .text_detection_config(TextDetectionConfig { score_threshold: 0.2, box_threshold: 0.45, unclip_ratio: 1.4, max_candidates: 3000, ..Default::default() }),
        "v6s" => OAROCRBuilder::pp_ocrv6(PpOcrV6Size::Small),
        "v6m" => OAROCRBuilder::pp_ocrv6(PpOcrV6Size::Medium),
        c => panic!("{c}"),
    };
    let ocr = b.ort_session(cfg).return_word_box(true).build().expect("build");
    let mut total = 0.0;
    let mut n = 0;
    for path in std::fs::read_to_string(list).expect("list").lines() {
        let img = image::open(path).expect("image").to_rgb8();
        let t = Instant::now();
        let res = ocr.predict(vec![img]).expect("predict");
        let s = t.elapsed().as_secs_f64();
        total += s;
        n += 1;
        let regions: Vec<_> = res[0].text_regions.iter().map(|r| {
            let pts: Vec<_> = r.bounding_box.points.iter().map(|p| [p.x, p.y]).collect();
            let words = r.word_boxes.as_ref().map(|w| w.len());
            json!({"t": r.text.as_deref().unwrap_or(""), "c": r.confidence, "poly": pts, "word_boxes": words})
        }).collect();
        println!("{}", json!({"path": path, "seconds": s, "regions": regions}));
    }
    println!("{}", json!({"summary": true, "pages": n, "seconds": total, "ppm": n as f64 * 60.0 / total}));
}
