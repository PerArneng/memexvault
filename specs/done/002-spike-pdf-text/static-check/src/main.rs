//! Spec 002: prove pdfium links statically and works with no dylib on the search path.
use pdfium_render::prelude::*;
fn main() {
    let path = std::env::args().nth(1).expect("pdf");
    let pdfium = Pdfium::new(Pdfium::bind_to_statically_linked_library().expect("bind"));
    let doc = pdfium.load_pdf_from_file(&path, Some("")).expect("open");
    let chars: usize = doc.pages().iter().map(|p| p.text().map(|t| t.chars().len()).unwrap_or(0)).sum();
    let page = doc.pages().get(0).expect("page");
    let img = page.render_with_config(&PdfRenderConfig::new().scale_page_by_factor(150.0 / 72.0)).expect("render").as_image().expect("img");
    println!("pages={} chars={} first_page_px={}x{}", doc.pages().len(), chars, img.width(), img.height());
}
