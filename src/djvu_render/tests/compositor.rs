use super::*;

// ── Compositor hot-path unit tests ───────────────────────────────────────
//
// The three `composite_rows_*_one` functions are the compositor's hot
// paths. They were previously exercised only through full-page
// `render_pixmap` / `render_into` against decoded fixtures, so a compositor
// bug could not be isolated from a decode bug. These tests drive each path
// directly off a synthetic `CompositeContext` (built from layers we control,
// not decoded from a file), making the compositor an independently-tested
// surface. Solid / block-uniform colours keep the assertions exact and free
// of sampling-rounding brittleness.

pub(super) fn identity_lut() -> [u8; 256] {
    core::array::from_fn(|i| i as u8)
}

/// Build a `CompositeContext` from synthetic layers, mirroring the q24 /
/// gamma wiring that [`CompositeContext::from_layers`] performs, but without
/// needing a `DjVuPage`. Foreground palette / blit map are unused here.
#[allow(clippy::too_many_arguments)]
pub(super) fn synth_ctx<'a>(
    opts: &'a RenderOptions,
    page_w: u32,
    page_h: u32,
    bg: Option<&'a Pixmap>,
    mask: Option<&'a crate::bitmap::Bitmap>,
    gamma_lut: &'a [u8; 256],
    out_w: u32,
    out_h: u32,
) -> CompositeContext<'a> {
    let (fg_x_q24, fg_y_q24) = fg_q24(None, page_w, page_h);
    let fg_red = 0;
    let bg = bg.map(PlaneView::whole);
    let (bg_x_q24, bg_y_q24) = bg_q24(bg.map(|b| (b.width(), b.height())), page_w, page_h);
    let bg_red = bg.map_or(0, |b| {
        native_bg_red(
            (page_w, page_h),
            (opts.width, opts.height),
            (b.width(), b.height()),
        )
    });
    CompositeContext {
        opts,
        page_w,
        page_h,
        bg,
        bg_x_q24,
        bg_y_q24,
        bg_red,
        mask,
        mask_shift: 0,
        fg_palette: None,
        blit_map: None,
        fg44: None,
        fg_x_q24,
        fg_y_q24,
        fg_red,
        gamma_lut,
        // Compute from the lut (mirroring CompositeContext::from_layers)
        // rather than hard-coding, so a future test passing a non-identity
        // lut is not silently routed down the identity fast path.
        gamma_is_identity: gamma_lut.iter().enumerate().all(|(i, &v)| v == i as u8),
        offset_x: 0,
        offset_y: 0,
        out_w,
        out_h,
    }
}

#[test]
fn composite_bilevel_one_maps_mask_to_black_and_white() {
    // Foreground bits → opaque black; background → opaque white.
    let opts = RenderOptions::default();
    let mut mask = crate::bitmap::Bitmap::new(8, 1);
    mask.set_black(0, 0);
    mask.set_black(3, 0);
    let lut = identity_lut();
    let ctx = synth_ctx(&opts, 8, 1, None, Some(&mask), &lut, 8, 1);

    let mut row = vec![0u8; 8 * 4];
    composite_rows_bilevel_one(&ctx, 0, Step::new(1, 1), Step::new(1, 1), &mut row);

    for x in 0..8usize {
        let p = &row[x * 4..x * 4 + 4];
        if x == 0 || x == 3 {
            assert_eq!(p, [0, 0, 0, 255], "foreground pixel at x={x}");
        } else {
            assert_eq!(p, [255, 255, 255, 255], "background pixel at x={x}");
        }
    }
}

#[test]
fn composite_bilinear_one_copies_background_1to1_unsubsampled() {
    // page_w == bg_w ⇒ bg_x_q24 == 1<<24, so this drives the A2 tight
    // mask-LUT-expand fast path (the common corpus case where the
    // background is at page resolution). Identity gamma + all-background
    // mask ⇒ the bg pixels must be reproduced exactly.
    let opts = RenderOptions::default();
    let bg = Pixmap::try_new(4, 2, 10, 20, 30, 255).expect("fits the pixmap limit");
    let mask = crate::bitmap::Bitmap::new(4, 2); // all background
    let lut = identity_lut();
    let ctx = synth_ctx(&opts, 4, 2, Some(&bg), Some(&mask), &lut, 4, 2);

    let mut row = vec![0u8; 4 * 4];
    let one = Step::new(1, 1);
    composite_rows_bilinear_one(&ctx, 0, one, one, &mut row, None, &mut Vec::new());

    for x in 0..4usize {
        let p = &row[x * 4..x * 4 + 4];
        assert_eq!(&p[..3], &[10, 20, 30], "background colour at x={x}");
        assert_eq!(p[3], 255, "opaque at x={x}");
    }
}

#[test]
fn composite_bilinear_one_upsamples_subsampled_background() {
    // page_w (8) > bg_w (4) ⇒ bg_x_q24 != 1<<24, so the A2 tight path is
    // skipped and the real bilinear sampler (`bilinear_from_rows`) runs to
    // upscale the subsampled background. A solid bg interpolates to itself,
    // so every output pixel must equal the bg colour exactly — proving the
    // sampler addresses the bg correctly without corrupting it.
    let opts = RenderOptions::default();
    let bg = Pixmap::try_new(4, 2, 70, 90, 110, 255).expect("fits the pixmap limit"); // half page resolution
    let mask = crate::bitmap::Bitmap::new(8, 2); // page-res, all background
    let lut = identity_lut();
    let ctx = synth_ctx(&opts, 8, 2, Some(&bg), Some(&mask), &lut, 8, 2);
    // Sanity: this configuration must NOT take the 1:1 tight path.
    assert_ne!(
        ctx.bg_x_q24,
        1 << 24,
        "test must exercise the subsampled path"
    );

    let mut row = vec![0u8; 8 * 4];
    let one = Step::new(1, 1);
    composite_rows_bilinear_one(&ctx, 0, one, one, &mut row, None, &mut Vec::new());

    for x in 0..8usize {
        let p = &row[x * 4..x * 4 + 4];
        assert_eq!(&p[..3], &[70, 90, 110], "upsampled bg colour at x={x}");
        assert_eq!(p[3], 255, "opaque at x={x}");
    }
}

#[test]
fn composite_area_avg_one_averages_uniform_background_on_downscale() {
    // 2× downscale of a solid background: every 2×2 source block averages to
    // the same colour, so the output cells equal that colour exactly.
    let opts = RenderOptions::default();
    let bg = Pixmap::try_new(4, 2, 40, 80, 120, 255).expect("fits the pixmap limit");
    let mask = crate::bitmap::Bitmap::new(4, 2); // all background
    let lut = identity_lut();
    let ctx = synth_ctx(&opts, 4, 2, Some(&bg), Some(&mask), &lut, 2, 1);

    let fx_step = Step::new(2, 1);
    let fy_step = Step::new(2, 1);
    let xs = precompute_area_avg_x(&ctx, fx_step);

    let mut row = vec![0u8; 2 * 4];
    composite_rows_area_avg_one(&ctx, 0, fx_step, fy_step, &mut row, Some(&xs));

    for x in 0..2usize {
        let p = &row[x * 4..x * 4 + 4];
        assert_eq!(&p[..3], &[40, 80, 120], "averaged background at x={x}");
        assert_eq!(p[3], 255, "opaque at x={x}");
    }
}

// ── TDD: failing tests written first ─────────────────────────────────────

/// Issue #199 regression: page-space FRACBITS coords must be scaled into
/// FG44-space using the `fg_x_q24` / `fg_y_q24` ratios. With page_w=2260
/// and fg_w=189 a Q24 ratio of `(189 << 24) / 2260` maps the rightmost
/// column to `fg_w - 1` instead of clamping every column past x=189.
#[test]
fn fg_q24_maps_endpoints_into_fg_space() {
    let fg = Pixmap::white(189, 306);
    let (qx, qy) = fg_q24(Some(&fg), 2260, 3669);
    assert!(qx > 0 && qy > 0);
    let frac = 1u64 << FRACBITS;
    let last_x = 2259u64 * frac;
    let fg_fx = (last_x * qx) >> 24;
    let fg_px = fg_fx >> FRACBITS;
    assert_eq!(fg_px, (fg.width as u64) - 1);
    let last_y = 3668u64 * frac;
    let fg_fy = (last_y * qy) >> 24;
    let fg_py = fg_fy >> FRACBITS;
    assert_eq!(fg_py, (fg.height as u64) - 1);
}

#[test]
fn fg_q24_returns_zero_when_fg_is_none() {
    assert_eq!(fg_q24(None, 100, 100), (0, 0));
}

/// Issue #199 second-half regression: BG plane is often stored at a
/// non-power-of-2 fraction of the page (1/3 is common for 400dpi colour
/// scans). Without `bg_x_q24` / `bg_y_q24` page→bg-space scaling the BG
/// sampler clamped most of the page to the rightmost BG column.
#[test]
fn bg_q24_maps_non_pow2_subsample() {
    // Page 2260×3669 with BG plane 754×1223 (DjVu's padded 1/3-page layout).
    let (qx, qy) = bg_q24(Some((754, 1223)), 2260, 3669);
    assert_eq!(qx, (1u64 << 24) / 3);
    assert_eq!(qy, (1u64 << 24) / 3);

    let last_x = 2259u32 * FRAC;
    let bg_px = (map_plane_center_frac(last_x, qx) as u64) >> FRACBITS;
    assert!(bg_px < 754);
    let last_y = 3668u32 * FRAC;
    let bg_py = (map_plane_center_frac(last_y, qy) as u64) >> FRACBITS;
    assert!(bg_py < 1223);
}

#[test]
fn bg_q24_returns_zero_when_bg_is_none() {
    assert_eq!(bg_q24(None, 100, 100), (0, 0));
}

#[test]
fn plane_q24_some_branch_with_zero_dimension_plane() {
    // Lines 508-512: plane_q24 Some(p) arm, reached via fg_q24 when fg has
    // zero width (so fg_q24's inner guard p.width > 0 fails, falling through
    // to plane_q24). With page_w > 0 && page_h > 0, plane_q24 takes its
    // Some arm and returns (0/page_w, 0/page_h) = (0, 0).
    let zero_width_fg = Pixmap::try_new(0, 10, 0, 0, 0, 0).expect("fits the pixmap limit");
    let (qx, qy) = fg_q24(Some(&zero_width_fg), 100, 100);
    assert_eq!(qx, 0); // (0 << 24) / 100 = 0
    assert_eq!(qy, (10u64 << 24) / 100); // height-based ratio
}

#[test]
fn fg_q24_uses_integer_horizontal_cell_pitch() {
    let fg = Pixmap::white(189, 306);
    let (qx, qy) = fg_q24(Some(&fg), 2260, 3669);
    assert_eq!(qx, (1u64 << 24) / 12);
    assert_eq!(qy, ((306u64) << 24) / 3669);
}

#[test]
fn bg_q24_uses_integer_cell_pitch_for_padded_edges() {
    let (qx, qy) = bg_q24(Some((754, 1223)), 2260, 3669);
    assert_eq!(qx, (1u64 << 24) / 3);
    assert_eq!(qy, (1u64 << 24) / 3);
}

#[test]
fn scaler_coord_matches_djvulibre_prepare_coord() {
    // GScaler::prepare_coord(in=1, out=3): beg = 19/6 - 8 = -5, then
    // beg + (1 + 16k) / 3.
    let got: Vec<i32> = (0..6).map(|k| scaler_coord(k, 3, 100)).collect();
    assert_eq!(got, [-5, 0, 6, 11, 16, 22]);
    // Clamped to the last plane pixel, (len - 1) * 16.
    assert_eq!(scaler_coord(5, 3, 2), 16);
    // red 2: beg = 9/2 - 8 = -4, then (1 + 16k) / 2.
    let got: Vec<i32> = (0..4).map(|k| scaler_coord(k, 2, 100)).collect();
    assert_eq!(got, [-4, 4, 12, 20]);
}

#[test]
fn scaler_rows_count_from_the_bottom() {
    // 3646-row page, 1216-row plane, red 3: 3646 % 3 == 1, so a
    // top-origin mapping is off by one page row.
    // Bottom page row -> coordinate -5 -> both rows are the last one.
    assert_eq!(scaler_rows(3645, 3646, 3, 1216), (1215, 1215, 11));
    // From-bottom 1 -> coordinate 0 -> last row, no blend.
    assert_eq!(scaler_rows(3644, 3646, 3, 1216), (1215, 1214, 0));
    // Top page row: from-bottom 3645 -> -5 + 58321 / 3 = 19435, just
    // below the clamp 1215 * 16 = 19440: plane rows 1214 and 1215 from
    // the bottom, i.e. top-origin rows 1 and 0, weight 11.
    assert_eq!(scaler_rows(0, 3646, 3, 1216), (1, 0, 11));
}

#[test]
fn scaler_lerp_rounds_with_arithmetic_shift() {
    assert_eq!(scaler_lerp(0, 255, 8), 128);
    assert_eq!(scaler_lerp(255, 0, 8), 128);
    // (-255 + 8) >> 4 is -16 (floor), not -15 (truncation).
    assert_eq!(scaler_lerp(255, 0, 1), 239);
    assert_eq!(scaler_lerp(10, 20, 0), 10);
    assert_eq!(scaler_lerp(10, 20, 15), 19);
}

#[test]
fn fg_native_frac_uses_bottom_origin_cells() {
    // 3646-row page, 304-row FG44 plane, red 12: 3646 % 12 == 10.
    let fg = Pixmap::try_new(184, 304, 0, 0, 0, 255).expect("fits the pixmap limit");
    // The bottom 12 page rows read the last FG row.
    assert_eq!(fg_native_frac(0, 3645, 3646, 12, &fg), (0, 303 << FRACBITS));
    assert_eq!(fg_native_frac(0, 3634, 3646, 12, &fg), (0, 303 << FRACBITS));
    assert_eq!(fg_native_frac(0, 3633, 3646, 12, &fg), (0, 302 << FRACBITS));
    // The top 10 page rows form the partial first cell.
    assert_eq!(fg_native_frac(25, 9, 3646, 12, &fg), (2 << FRACBITS, 0));
    assert_eq!(
        fg_native_frac(25, 10, 3646, 12, &fg),
        (2 << FRACBITS, 1 << FRACBITS)
    );
}

#[test]
fn compute_red_matches_djvulibre() {
    assert_eq!(compute_red((2208, 3646), (736, 1216)), Some(3));
    assert_eq!(compute_red((2208, 3646), (184, 304)), Some(12));
    assert_eq!(compute_red((2208, 3646), (2208, 3646)), Some(1));
    assert_eq!(compute_red((2208, 3646), (700, 1216)), None);
}

#[test]
fn map_plane_center_frac_aligns_pixel_centers() {
    // Destination page is twice the source plane size.  Page pixel x=1 has
    // centre 1.5; mapped to source centre space that is 1.5 * 0.5 - 0.5 = 0.25.
    let q24 = (1u64 << 24) / 2;
    assert_eq!(map_plane_center_frac(0, q24), 0);
    assert_eq!(map_plane_center_frac(FRAC, q24), FRAC / 4);
}

#[test]
fn sample_bilinear_rounds_to_nearest() {
    let mut pm = Pixmap::try_new(2, 2, 0, 0, 0, 255).expect("fits the pixmap limit");
    pm.set_rgb(1, 1, 255, 255, 255);

    // At the exact centre, bilinear interpolation is 63.75, which should
    // round to 64 instead of truncating to 63.
    assert_eq!(sample_bilinear(&pm, FRAC / 2, FRAC / 2), (64, 64, 64));
}

#[test]
fn sample_nearest_rounds_to_nearest_pixel() {
    let mut pm = Pixmap::try_new(2, 1, 10, 20, 30, 255).expect("fits the pixmap limit");
    pm.set_rgb(1, 0, 200, 210, 220);

    assert_eq!(sample_nearest(&pm, FRAC / 2 - 1, 0), (10, 20, 30));
    assert_eq!(sample_nearest(&pm, FRAC / 2, 0), (200, 210, 220));
}

#[test]
fn mask_box_coverage_values() {
    use crate::bitmap::Bitmap;
    // 4×1 mask: bits [1,0,1,1] → 3 out of 4 → coverage = (3*255+2)/4 = 191
    let mut bm = Bitmap::new(4, 1);
    bm.set(0, 0, true);
    bm.set(2, 0, true);
    bm.set(3, 0, true);
    let step = 4 * FRAC;
    assert_eq!(mask_box_coverage(&bm, 0, 0, step, FRAC), 191);
    // all foreground → 255
    let mut bm_full = Bitmap::new(2, 2);
    bm_full.set(0, 0, true);
    bm_full.set(1, 0, true);
    bm_full.set(0, 1, true);
    bm_full.set(1, 1, true);
    assert_eq!(mask_box_coverage(&bm_full, 0, 0, 2 * FRAC, 2 * FRAC), 255);
    // all background → 0
    let bm_empty = Bitmap::new(2, 2);
    assert_eq!(mask_box_coverage(&bm_empty, 0, 0, 2 * FRAC, 2 * FRAC), 0);
}

/// #963: `FgNativeRow::at` replaces `fg_native_frac` + `sample_bilinear` on
/// native renders. Check it against them for every reduction `compute_red`
/// allows and every column of a 16-bit-wide page, plus a truncated row.
#[test]
fn fg_native_row_matches_whole_cell_bilinear() {
    for red in 1..=12u32 {
        let page_w = u16::MAX as u32;
        let fg = Pixmap {
            width: page_w.div_ceil(red),
            height: 2,
            data: (0..page_w.div_ceil(red) * 2 * 4)
                .map(|i| (i * 7 % 251) as u8)
                .collect(),
        };
        let row = fg_native_frac(0, 0, 2 * red, red, &fg).1 >> FRACBITS;
        let stride = fg.width as usize * 4;
        let n = FgNativeRow::new(&fg.data[row as usize * stride..], fg.width, red);
        for px in 0..page_w {
            let (fx, fy) = fg_native_frac(px, 0, 2 * red, red, &fg);
            assert_eq!(n.at(px), sample_bilinear(&fg, fx, fy), "red {red} px {px}");
        }
    }
    // A truncated plane reads as black, as `Pixmap::get_rgb` does.
    let short = FgNativeRow::new(&[1, 2, 3, 4, 5, 6], 4, 1);
    assert_eq!(short.at(0), (1, 2, 3));
    assert_eq!(short.at(1), (0, 0, 0));
}

/// The output column of the darkest pixel of a one-row composite.
fn darkest_column(ctx: &CompositeContext<'_>) -> usize {
    let mut row = vec![0u8; ctx.out_w as usize * 4];
    composite_into(ctx, &mut row).unwrap();
    let luma = |px: &[u8]| u32::from(px[0]) + u32::from(px[1]) + u32::from(px[2]);
    (0..ctx.out_w as usize)
        .min_by_key(|&x| luma(&row[x * 4..x * 4 + 4]))
        .unwrap()
}

/// A scaled composite puts page column `x` at output column
/// `x * out_w / page_w`, at a scale whose step is not a whole number of
/// sixteenths: 2539 → 1270 (just under 2) and 1000 → 1030 (just under 1).
/// Truncated to sixteenths, those steps put it 38 and 60 columns off.
#[test]
fn scaled_composites_keep_page_columns_in_place() {
    let lut = identity_lut();
    for (page_w, out_w, dark) in [(2539u32, 1270u32, 2400u32), (1000, 1030, 900)] {
        let expected = (dark as usize * out_w as usize) / page_w as usize;
        let opts = RenderOptions {
            width: out_w,
            height: 1,
            ..RenderOptions::default()
        };
        let mut bg = Pixmap::white(page_w, 1);
        bg.set_rgb(dark, 0, 0, 0, 0);
        bg.set_rgb(dark + 1, 0, 0, 0, 0);
        let ctx = synth_ctx(&opts, page_w, 1, Some(&bg), None, &lut, out_w, 1);
        let got = darkest_column(&ctx);
        assert!(
            got.abs_diff(expected) <= 1,
            "background {page_w} -> {out_w}: column {dark} at {got}, expected {expected}"
        );
        let mut mask = crate::bitmap::Bitmap::new(page_w, 1);
        mask.set_black(dark, 0);
        mask.set_black(dark + 1, 0);
        let ctx = synth_ctx(&opts, page_w, 1, None, Some(&mask), &lut, out_w, 1);
        let got = darkest_column(&ctx);
        assert!(
            got.abs_diff(expected) <= 1,
            "mask {page_w} -> {out_w}: column {dark} at {got}, expected {expected}"
        );
    }
}

/// Every source pixel of a downscale lands in some output pixel's footprint:
/// one black column (or row) of a background or a mask stays visible, near
/// where the scale puts it, at 26 → 19 and 2539 → 1270.
#[test]
fn downscale_keeps_a_single_source_column_and_row() {
    let lut = identity_lut();
    for (page, out, dark) in [(26u32, 19u32, 14u32), (2539, 1270, 2382)] {
        let expected = (dark as usize * out as usize) / page as usize;
        for across in [true, false] {
            let (page_w, page_h, out_w, out_h) = if across {
                (page, 1, out, 1)
            } else {
                (1, page, 1, out)
            };
            let opts = RenderOptions {
                width: out_w,
                height: out_h,
                ..RenderOptions::default()
            };
            let (x, y) = if across { (dark, 0) } else { (0, dark) };
            let mut bg = Pixmap::white(page_w, page_h);
            bg.set_rgb(x, y, 0, 0, 0);
            let mut mask = crate::bitmap::Bitmap::new(page_w, page_h);
            mask.set_black(x, y);
            for (layer, ctx) in [
                (
                    "background",
                    synth_ctx(&opts, page_w, page_h, Some(&bg), None, &lut, out_w, out_h),
                ),
                (
                    "mask",
                    synth_ctx(&opts, page_w, page_h, None, Some(&mask), &lut, out_w, out_h),
                ),
            ] {
                let mut buf = vec![0u8; (out_w * out_h) as usize * 4];
                composite_into(&ctx, &mut buf).unwrap();
                let darkest = buf
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, px)| u32::from(px[0]) + u32::from(px[1]) + u32::from(px[2]))
                    .map(|(i, px)| (i, px[0]));
                assert!(
                    darkest.is_some_and(|(i, value)| value < 255 && i.abs_diff(expected) <= 1),
                    "{layer} {page} -> {out} {}: source {dark} gives {darkest:?}, expected near {expected}",
                    if across { "across" } else { "down" },
                );
            }
        }
    }
}

/// The last output pixel's footprint ends at the page's edge, so the last
/// source column and row are read: through a full-size background, one at a
/// third of the page, the bilevel mask and FG44 under an all-black mask, at
/// steps that do not divide the page.
#[test]
fn downscale_keeps_the_last_source_column_and_row() {
    let lut = identity_lut();
    for (page, out) in [(26u32, 19u32), (27, 19), (2539, 1270)] {
        let third = page.div_ceil(3);
        for across in [true, false] {
            let along = |extent: u32| if across { (extent, 1) } else { (1, extent) };
            let at = |i: u32| if across { (i, 0) } else { (0, i) };
            let ((page_w, page_h), (out_w, out_h)) = (along(page), along(out));
            let opts = RenderOptions {
                width: out_w,
                height: out_h,
                ..RenderOptions::default()
            };
            let (x, y) = at(page - 1);
            let mut bg = Pixmap::white(page_w, page_h);
            bg.set_rgb(x, y, 0, 0, 0);
            let (third_w, third_h) = along(third);
            let mut reduced = Pixmap::white(third_w, third_h);
            let (rx, ry) = at(third - 1);
            reduced.set_rgb(rx, ry, 0, 0, 0);
            let mut mask = crate::bitmap::Bitmap::new(page_w, page_h);
            mask.set_black(x, y);
            let mut black = crate::bitmap::Bitmap::new(page_w, page_h);
            for i in 0..page {
                let (bx, by) = at(i);
                black.set_black(bx, by);
            }
            let mut fg = synth_ctx(
                &opts,
                page_w,
                page_h,
                None,
                Some(&black),
                &lut,
                out_w,
                out_h,
            );
            fg.fg44 = Some(&bg);
            (fg.fg_x_q24, fg.fg_y_q24) = fg_q24(Some(&bg), page_w, page_h);
            for (layer, ctx) in [
                (
                    "background",
                    synth_ctx(&opts, page_w, page_h, Some(&bg), None, &lut, out_w, out_h),
                ),
                (
                    "background at a third",
                    synth_ctx(
                        &opts,
                        page_w,
                        page_h,
                        Some(&reduced),
                        None,
                        &lut,
                        out_w,
                        out_h,
                    ),
                ),
                (
                    "mask",
                    synth_ctx(&opts, page_w, page_h, None, Some(&mask), &lut, out_w, out_h),
                ),
                ("FG44", fg),
            ] {
                let mut buf = vec![0u8; (out_w * out_h) as usize * 4];
                composite_into(&ctx, &mut buf).unwrap();
                let last = &buf[buf.len() - 4..];
                assert!(
                    last[0] < 255,
                    "{layer} {page} -> {out} {}: the last output pixel is {last:?}",
                    if across { "across" } else { "down" },
                );
            }
        }
    }
}

/// A region of an upscaled render is the matching crop of the whole render,
/// also over a subsampled background at a ratio that is no power of two:
/// page 577 wide, background 97 wide in alternating black and white, output
/// 1155 wide, compared pixel by pixel with one-pixel regions.
#[test]
fn upscaled_region_matches_the_whole_render() {
    let lut = identity_lut();
    let (page_w, out_w) = (577u32, 1155u32);
    let opts = RenderOptions {
        width: out_w,
        height: 1,
        ..RenderOptions::default()
    };
    let mut bg = Pixmap::white(97, 1);
    for x in (0..97).step_by(2) {
        bg.set_rgb(x, 0, 0, 0, 0);
    }
    let whole = synth_ctx(&opts, page_w, 1, Some(&bg), None, &lut, out_w, 1);
    let mut row = vec![0u8; out_w as usize * 4];
    composite_into(&whole, &mut row).unwrap();
    for x in [1, 433, 866, 1154] {
        let mut region = synth_ctx(&opts, page_w, 1, Some(&bg), None, &lut, 1, 1);
        region.offset_x = x;
        let mut px = [0u8; 4];
        composite_into(&region, &mut px).unwrap();
        let start = x as usize * 4;
        assert_eq!(px.as_slice(), &row[start..start + 4], "pixel {x}");
    }
}
