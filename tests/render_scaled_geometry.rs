//! A render at a size other than the page's native one keeps the page's
//! geometry: it is the native render, scaled. The box around the ink (pixels
//! darker than mid grey) of a scaled render must therefore lie within a pixel
//! or two of the native ink box scaled by the same factor, on every side.

use djvu_rs::Pixmap;
use djvu_rs::djvu_document::DjVuDocument;
use djvu_rs::djvu_render::{RenderOptions, render_pixmap};

/// `(left, top, right, bottom)` of the pixels darker than mid grey, inclusive.
fn ink_box(pm: &Pixmap) -> Option<(u32, u32, u32, u32)> {
    let mut found: Option<(u32, u32, u32, u32)> = None;
    for (i, px) in pm.data.as_chunks::<4>().0.iter().enumerate() {
        let luma = 299 * u32::from(px[0]) + 587 * u32::from(px[1]) + 114 * u32::from(px[2]);
        if luma >= 128_000 {
            continue;
        }
        let (x, y) = (i as u32 % pm.width, i as u32 / pm.width);
        found = Some(match found {
            None => (x, y, x, y),
            Some((l, t, r, b)) => (l.min(x), t.min(y), r.max(x), b.max(y)),
        });
    }
    found
}

/// JB2 text over a background at 1270 x 1648 (a step just under 2), a slight
/// downscale, a phone's width and a slight upscale: the ink box scales with
/// the page. With steps rounded to sixteenths, the 1270-wide box ended 51
/// rows low.
#[test]
fn text_page_keeps_its_geometry_at_other_sizes() {
    let data = std::fs::read("tests/fixtures/DjVu3Spec_bundled.djvu").unwrap();
    let doc = DjVuDocument::parse(&data).unwrap();
    let page = doc.page(0).unwrap();
    let full = render_pixmap(
        page,
        &RenderOptions::fit_to_width(page, page.width() as u32),
    )
    .unwrap();
    let ink = ink_box(&full).unwrap();
    for width in [1270, 2463, 1080, 2615] {
        let scaled = render_pixmap(page, &RenderOptions::fit_to_width(page, width)).unwrap();
        let got = ink_box(&scaled).unwrap();
        let sx = f64::from(scaled.width) / f64::from(full.width);
        let sy = f64::from(scaled.height) / f64::from(full.height);
        let expected = (
            (f64::from(ink.0) * sx) as u32,
            (f64::from(ink.1) * sy) as u32,
            (f64::from(ink.2) * sx) as u32,
            (f64::from(ink.3) * sy) as u32,
        );
        let off = [
            got.0.abs_diff(expected.0),
            got.1.abs_diff(expected.1),
            got.2.abs_diff(expected.2),
            got.3.abs_diff(expected.3),
        ];
        assert!(
            off.iter().all(|&d| d <= 2),
            "at {}x{}: ink box {got:?}, expected about {expected:?}",
            scaled.width,
            scaled.height,
        );
    }
}
