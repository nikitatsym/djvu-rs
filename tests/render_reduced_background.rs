//! Re-subsampling a third-size background blurred carte.djvu by 2.8 grey levels.

use djvu_rs::Pixmap;
use djvu_rs::djvu_document::DjVuDocument;
use djvu_rs::djvu_render::{RenderOptions, render_pixmap};

fn luma(pm: &Pixmap, x: u32, y: u32) -> f64 {
    let i = (y as usize * pm.width as usize + x as usize) * 4;
    0.299 * f64::from(pm.data[i])
        + 0.587 * f64::from(pm.data[i + 1])
        + 0.114 * f64::from(pm.data[i + 2])
}

/// Mean grey difference of page 1 at half size from its full render averaged 2x2.
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

/// Both store their backgrounds at a third and halve to whole pixels.
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
