//! Edge: PDF text and word coordinates on a statically linked pdfium. Callers run this in a child process, because it's a C++ parser reading untrusted files. See spec 002.
use memex_types::{PageClass, PageFacts, ParsedDocument, Rect, Word};
use pdfium_render::prelude::*;
use std::fmt;

/// Part of a Rendition's identity, so a change here makes new renditions.
pub const PARSER: &str = "pdfium-8046d+memex-1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdfError {
    /// It needs a password we don't have.
    Encrypted,
    /// It isn't a readable PDF.
    Malformed(String),
    /// It's readable but holds no pages.
    NoPages,
}

impl fmt::Display for PdfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encrypted => f.write_str("the PDF is encrypted with a password"),
            Self::Malformed(m) => write!(f, "not a readable PDF: {m}"),
            Self::NoPages => f.write_str("the PDF has no pages"),
        }
    }
}

impl std::error::Error for PdfError {}

fn classify(page: &PdfPage<'_>) -> (PageClass, u32) {
    let area = page.width().value * page.height().value;
    let (mut visible, mut hidden, mut image_area) = (0u32, 0u32, 0f32);
    for object in page.objects().iter() {
        if let Some(text) = object.as_text_object() {
            if matches!(text.render_mode(), PdfPageTextRenderMode::Invisible) {
                hidden += 1;
            } else {
                visible += 1;
            }
        } else if object.as_image_object().is_some()
            && let Ok(b) = object.bounds()
        {
            image_area += b.width().value * b.height().value;
        }
    }
    let characters = page.text().map(|t| t.chars().len()).unwrap_or(0);
    let class = if visible == 0 && area > 0.0 && image_area / area > 0.5 {
        PageClass::ImageOnly
    } else if visible == 0 && characters == 0 {
        PageClass::Blank
    } else {
        PageClass::Text
    };
    (class, hidden)
}

/// Pdfium's characters, grouped into words our way: split on whitespace, on a soft hyphen at a line end, and when two characters overlap on neither axis.
fn page_words(page: &PdfPage<'_>, index: u32, out: &mut Vec<Word>) {
    let Ok(text) = page.text() else { return };
    let (width, height) = ((page.width().value * 100.0) as i32, (page.height().value * 100.0) as i32);
    let config = PdfRenderConfig::new().set_target_size(width, height);
    let mut current = String::new();
    let mut bounds: Option<Rect> = None;
    let mut previous: Option<Rect> = None;
    let flush = |current: &mut String, bounds: &mut Option<Rect>, out: &mut Vec<Word>| {
        if let Some(rect) = bounds.take() {
            // A word crossing the crop or outside it isn't on the visible page.
            if rect.x0 >= 0 && rect.y0 >= 0 && rect.x1 <= width && rect.y1 <= height {
                out.push(Word {
                    text: current.clone(),
                    page: index,
                    rect,
                    confidence: None,
                    quad: None,
                });
            }
        }
        current.clear();
    };
    for ch in text.chars().iter() {
        let s = ch.unicode_string().unwrap_or_default();
        if s.chars().all(char::is_whitespace) {
            flush(&mut current, &mut bounds, out);
            continue;
        }
        let Ok(r) = ch.loose_bounds() else { continue };
        let (Ok((ax, ay)), Ok((bx, by))) = (
            page.points_to_pixels(r.left(), r.top(), &config),
            page.points_to_pixels(r.right(), r.bottom(), &config),
        ) else {
            continue;
        };
        let q = Rect {
            x0: ax.min(bx),
            y0: ay.min(by),
            x1: ax.max(bx),
            y1: ay.max(by),
        };
        if let Some(p) = previous {
            let over_x = p.x0 < q.x1 && q.x0 < p.x1;
            let over_y = p.y0 < q.y1 && q.y0 < p.y1;
            if !over_x && !over_y {
                flush(&mut current, &mut bounds, out);
            }
        }
        previous = Some(q);
        bounds = Some(match bounds {
            None => q,
            Some(o) => Rect {
                x0: o.x0.min(q.x0),
                y0: o.y0.min(q.y0),
                x1: o.x1.max(q.x1),
                y1: o.y1.max(q.y1),
            },
        });
        if s == "\u{2}" {
            current.push('-');
            flush(&mut current, &mut bounds, out);
        } else {
            current.push_str(&s);
        }
    }
    flush(&mut current, &mut bounds, out);
}

/// The parser. Pdfium's library can be initialized once per process, so make one of these and share it.
pub struct PdfiumParser {
    pdfium: Pdfium,
}

impl PdfiumParser {
    pub fn new() -> Result<Self, PdfError> {
        let bindings = Pdfium::bind_to_statically_linked_library().map_err(|e| PdfError::Malformed(e.to_string()))?;
        Ok(Self { pdfium: Pdfium::new(bindings) })
    }

    /// Parses a PDF from its bytes. A PDF encrypted with an empty password opens as usual.
    pub fn parse(&self, bytes: Vec<u8>) -> Result<ParsedDocument, PdfError> {
        parse_with(&self.pdfium, bytes)
    }
}

impl PdfiumParser {
    /// Renders one page to RGB, for OCR. The caller chooses a resolution that bounds memory.
    pub fn render_page(&self, bytes: Vec<u8>, page: u32, dpi: u32) -> Result<(u32, u32, Vec<u8>), PdfError> {
        let doc = self.pdfium.load_pdf_from_byte_vec(bytes, Some("")).map_err(|e| match e {
            PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => PdfError::Encrypted,
            other => PdfError::Malformed(other.to_string()),
        })?;
        let page = doc.pages().get(page as i32).map_err(|e| PdfError::Malformed(e.to_string()))?;
        let config = PdfRenderConfig::new().scale_page_by_factor(dpi as f32 / 72.0);
        let rendered = page.render_with_config(&config).map_err(|e| PdfError::Malformed(e.to_string()))?;
        let image = rendered.as_image().map_err(|e| PdfError::Malformed(e.to_string()))?.into_rgb8();
        Ok((image.width(), image.height(), image.into_raw()))
    }
}

impl PdfiumParser {
    /// A page as a PNG, with translucent highlights over the rects. Rects are in displayed coordinates, which is what the page renders in.
    pub fn render_highlighted(&self, bytes: Vec<u8>, page: u32, dpi: u32, rects: &[Rect]) -> Result<Vec<u8>, PdfError> {
        let (width, height, mut rgb) = self.render_page(bytes, page, dpi)?;
        for r in rects {
            let px = |v: i32| f64::from(v.max(0)) * f64::from(dpi) / 7200.0;
            let (x0, y0, x1, y1) = (
                px(r.x0).floor() as u32,
                px(r.y0).floor() as u32,
                (px(r.x1).ceil() as u32).min(width),
                (px(r.y1).ceil() as u32).min(height),
            );
            for y in y0..y1 {
                for x in x0..x1 {
                    let at = (y as usize * width as usize + x as usize) * 3;
                    // Yellow, multiplied, so the text under it stays readable.
                    rgb[at + 2] = (u16::from(rgb[at + 2]) * 90 / 255) as u8;
                }
            }
        }
        let mut png = vec![];
        let encoder = image::codecs::png::PngEncoder::new(&mut png);
        image::ImageEncoder::write_image(encoder, &rgb, width, height, image::ExtendedColorType::Rgb8).map_err(|e| PdfError::Malformed(e.to_string()))?;
        Ok(png)
    }
}

fn parse_with(pdfium: &Pdfium, bytes: Vec<u8>) -> Result<ParsedDocument, PdfError> {
    let doc = pdfium.load_pdf_from_byte_vec(bytes, Some("")).map_err(|e| match e {
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => PdfError::Encrypted,
        other => PdfError::Malformed(other.to_string()),
    })?;
    if doc.pages().is_empty() {
        return Err(PdfError::NoPages);
    }
    let (mut pages, mut words) = (vec![], vec![]);
    for (i, page) in doc.pages().iter().enumerate() {
        let index = i as u32;
        let (class, hidden_text_objects) = classify(&page);
        let rotation = page.rotation().map(|r| r.as_degrees() as i32).unwrap_or(0);
        pages.push(PageFacts {
            index,
            width: (page.width().value * 100.0) as i32,
            height: (page.height().value * 100.0) as i32,
            rotation,
            class,
            hidden_text_objects,
        });
        page_words(&page, index, &mut words);
    }
    Ok(ParsedDocument {
        parser: PARSER.into(),
        pages,
        words,
    })
}
