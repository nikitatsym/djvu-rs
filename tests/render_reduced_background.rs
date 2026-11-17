//! A page whose background is stored at a fraction of the page's resolution
//! (BG44 at a third of it is common in scans) keeps that background's detail
//! when rendered smaller than the page. At half size the render is compared
//! with the full-size render averaged over 2x2 blocks; decoding the reduced
//! plane at half its resolution again (and upscaling it) made carte.djvu
//! differ by 2.8 grey levels and history.djvu by 13.3.

use djvu_rs::Pixmap;
use djvu_rs::djvu_document::DjVuDocument;
use djvu_rs::djvu_render::{RenderOptions, render_pixmap};

fn luma(pm: &Pixmap, x: u32, y: u32) -> f64 {
    let i = (y as usize * pm.width as usize + x as usize) * 4;
    0.299 * f64::from(pm.data[i])
        + 0.587 * f64::from(pm.data[i + 1])
        + 0.114 * f64::from(pm.data[i + 2])
}

/// Mean grey-level difference of page 1 of `fixture` at half size from its
/// full-size render averaged over 2x2 blocks.
fn half_size_difference(fixture: &str) -> f64 {
    let data = std::fs::read(format!("tests/fixtures/{fixture}")).unwrap();
    let doc = DjVuDocument::parse(&data).unwrap();
    let page = doc.page(0).unwrap();
    let full = render_pixmap(
        page,
        &RenderOptions::fit_to_width(page, page.width() as u32),
    )
    .unwrap();
    let half = render_pixmap(page, &RenderOptions::fit_to_width(page, full.width / 2)).unwrap();
    let (w, h) = (
        half.width.min(full.width / 2),
        half.height.min(full.height / 2),
    );
    let mut sum = 0.0;
    for y in 0..h {
        for x in 0..w {
            let block = (luma(&full, 2 * x, 2 * y)
                + luma(&full, 2 * x + 1, 2 * y)
                + luma(&full, 2 * x, 2 * y + 1)
                + luma(&full, 2 * x + 1, 2 * y + 1))
                / 4.0;
            sum += (luma(&half, x, y) - block).abs();
        }
    }
    sum / f64::from(w * h)
}

/// carte.djvu and history.djvu store their backgrounds at a third of the
/// page's resolution, and both halve to whole pixels.
#[test]
fn reduced_background_keeps_its_detail_at_half_size() {
    for (fixture, limit) in [("carte.djvu", 2.3), ("history.djvu", 11.5)] {
        let diff = half_size_difference(fixture);
        assert!(
            diff < limit,
            "{fixture} at half size: mean difference {diff:.2} from the full-size render averaged"
        );
    }
}
