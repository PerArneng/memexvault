//! Edge: OCR on ONNX Runtime through oar-ocr, with the models spec 003 chose. Callers run this in a child process.
use memex_core::ocr::{OcrPage, OcrRegion};
use oar_ocr::core::config::OrtSessionConfig;
use oar_ocr::domain::tasks::text_detection::TextDetectionConfig;
use oar_ocr::oarocr::{OAROCR, OAROCRBuilder};
use std::path::Path;

pub const DETECTOR: &str = "pp-ocrv6_small_det.onnx";
pub const RECOGNIZER: &str = "latin_pp-ocrv5_mobile_rec.onnx";
pub const DICTIONARY: &str = "ppocrv5_latin_dict.txt";

pub struct Engine {
    ocr: OAROCR,
}

impl Engine {
    pub fn new(models_dir: &Path, threads: usize) -> Result<Self, String> {
        let path = |file: &str| models_dir.join(file).to_string_lossy().into_owned();
        let session = OrtSessionConfig::new().with_intra_threads(threads).with_inter_threads(1);
        // These detector settings are what kept columns apart in spec 003.
        let detection = TextDetectionConfig {
            score_threshold: 0.2,
            box_threshold: 0.45,
            unclip_ratio: 1.4,
            max_candidates: 3000,
            ..Default::default()
        };
        let ocr = OAROCRBuilder::new(path(DETECTOR), path(RECOGNIZER), path(DICTIONARY))
            .text_detection_config(detection)
            .ort_session(session)
            .return_word_box(true)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { ocr })
    }

    /// Reads one RGB image.
    pub fn read(&self, width: u32, height: u32, rgb: Vec<u8>, dpi: u32) -> Result<OcrPage, String> {
        let image = image::RgbImage::from_raw(width, height, rgb).ok_or("the image's size doesn't match its bytes")?;
        let results = self.ocr.predict(vec![image]).map_err(|e| e.to_string())?;
        let mut regions = vec![];
        for r in results.into_iter().flat_map(|x| x.text_regions) {
            let Some(text) = r.text.as_deref() else { continue };
            let p = &r.bounding_box.points;
            if p.len() < 4 {
                continue;
            }
            let char_boxes = r.word_boxes.as_ref().map(|boxes| {
                boxes
                    .iter()
                    .map(|b| {
                        let (xs, ys): (Vec<f32>, Vec<f32>) = b.points.iter().map(|q| (q.x, q.y)).unzip();
                        [
                            xs.iter().copied().fold(f32::MAX, f32::min),
                            ys.iter().copied().fold(f32::MAX, f32::min),
                            xs.iter().copied().fold(f32::MIN, f32::max),
                            ys.iter().copied().fold(f32::MIN, f32::max),
                        ]
                    })
                    .collect()
            });
            regions.push(OcrRegion {
                text: text.to_string(),
                confidence: r.confidence.unwrap_or(0.0),
                poly: [(p[0].x, p[0].y), (p[1].x, p[1].y), (p[2].x, p[2].y), (p[3].x, p[3].y)],
                char_boxes,
            });
        }
        Ok(OcrPage { dpi, regions })
    }
}
