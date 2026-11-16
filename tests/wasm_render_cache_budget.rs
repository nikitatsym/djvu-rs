//! The WebAssembly binding sets the process-wide render cache ceiling. A page
//! keeps what it decoded for its next render, up to `render_cache::budget()`
//! (256 MiB unless set) across all pages; wasm linear memory never shrinks,
//! so a viewer reading a book page after page grows to that ceiling. Through
//! the binding it sets a smaller one, and reading stays under it.
#![cfg(feature = "wasm")]

use djvu_rs::djvu_document::DjVuDocument;
use djvu_rs::djvu_render::{RenderOptions, render_pixmap};
use djvu_rs::render_cache;
use djvu_rs::wasm::{render_cache_bytes, set_render_cache_budget};

#[test]
fn wasm_binding_bounds_the_render_caches() {
    let budget = 16 * 1024 * 1024;
    set_render_cache_budget(budget as f64);
    assert_eq!(render_cache::budget(), budget);
    let data = std::fs::read("tests/fixtures/colorbook.djvu").unwrap();
    let doc = DjVuDocument::parse(&data).unwrap();
    let mut largest_page = 0;
    for i in 0..12 {
        let page = doc.page(i).unwrap();
        let opts = RenderOptions::fit_to_width(page, page.width() as u32);
        render_pixmap(page, &opts).unwrap();
        largest_page = largest_page.max(page.render_cache_bytes());
        // The sweep may overshoot by the layer being filled.
        assert!(
            render_cache::resident_bytes() <= budget + largest_page,
            "after page {i}: {} bytes cached",
            render_cache::resident_bytes()
        );
    }
    assert_eq!(render_cache_bytes(), render_cache::resident_bytes() as f64);
    set_render_cache_budget(render_cache::DEFAULT_BUDGET as f64);
}
