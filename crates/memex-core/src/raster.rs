use memex_types::Rect;
use std::fmt;

/// A rendered page as a PNG, with its size in pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageImage {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RasterError(pub String);

impl fmt::Display for RasterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RasterError {}

/// Renders a PDF page, with highlights drawn over the given rects. Bytes go in, so it works with any blob store.
pub trait Rasterizer {
    fn render(&self, pdf: &[u8], page: u32, dpi: u32, highlights: &[Rect]) -> Result<PageImage, RasterError>;
}

/// The resolutions a client may ask for. Thumbnails use the lowest.
pub const ALLOWED_DPI: [u32; 4] = [36, 72, 150, 300];
pub const THUMBNAIL_DPI: u32 = 36;

/// A rect in 1/100 pt, in pixels at a resolution. Pure, and the only place this conversion happens.
pub fn rect_to_pixels(r: &Rect, dpi: u32) -> (u32, u32, u32, u32) {
    let px = |v: i32| f64::from(v.max(0)) * f64::from(dpi) / 7200.0;
    (px(r.x0).floor() as u32, px(r.y0).floor() as u32, px(r.x1).ceil() as u32, px(r.y1).ceil() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rects_become_pixels_rounding_outward() {
        assert_eq!(
            rect_to_pixels(
                &Rect {
                    x0: 7200,
                    y0: 3600,
                    x1: 14400,
                    y1: 7300
                },
                72
            ),
            (72, 36, 144, 73)
        );
        assert_eq!(
            rect_to_pixels(
                &Rect {
                    x0: 7200,
                    y0: 0,
                    x1: 7201,
                    y1: 7200
                },
                300
            ),
            (300, 0, 301, 300)
        );
        assert_eq!(
            rect_to_pixels(
                &Rect {
                    x0: -100,
                    y0: -100,
                    x1: 100,
                    y1: 100
                },
                72
            ),
            (0, 0, 1, 1)
        );
    }
}
