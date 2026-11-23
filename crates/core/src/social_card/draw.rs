//! Drawing laid-out lines onto the background.

use ab_glyph::{point, Font, FontVec, GlyphId, ScaleFont};
use image::RgbaImage;

use super::layout::{px, scale};

/// Draws `lines` from the top-left corner `(x, y)` at `size`, one line
/// every 1.2 x size, each glyph's baseline centred in its line box the
/// way CSS half-leading does. Pixels outside the image are skipped.
pub(super) fn lines(
    img: &mut RgbaImage,
    font: &FontVec,
    lines: &[String],
    x: u32,
    y: u32,
    size: u32,
    color: [u8; 3],
) {
    let scale = scale(font, size);
    let sf = font.as_scaled(scale);
    let line_h = px(size) * 1.2;
    let half_leading = (line_h - (sf.ascent() - sf.descent())) / 2.0;
    for (n, line) in lines.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)] // at most a few lines
        let top = px(y) + line_h * n as f32;
        let baseline = top + half_leading + sf.ascent();
        let mut caret = px(x);
        let mut prev: Option<GlyphId> = None;
        for c in line.chars() {
            let id = sf.glyph_id(c);
            if let Some(p) = prev {
                caret += sf.kern(p, id);
            }
            prev = Some(id);
            let glyph = id.with_scale_and_position(scale, point(caret, baseline));
            caret += sf.h_advance(id);
            if let Some(outline) = font.outline_glyph(glyph) {
                let bounds = outline.px_bounds();
                outline.draw(|gx, gy, coverage| {
                    blend(img, bounds.min.x, bounds.min.y, gx, gy, coverage, color);
                });
            }
        }
    }
}

#[allow(clippy::cast_possible_truncation)] // floor of an on-canvas coordinate
fn blend(img: &mut RgbaImage, ox: f32, oy: f32, gx: u32, gy: u32, coverage: f32, color: [u8; 3]) {
    let x = i64::from(ox.floor() as i32) + i64::from(gx);
    let y = i64::from(oy.floor() as i32) + i64::from(gy);
    let (Ok(x), Ok(y)) = (u32::try_from(x), u32::try_from(y)) else {
        return;
    };
    let Some(p) = img.get_pixel_mut_checked(x, y) else {
        return;
    };
    let a = coverage.clamp(0.0, 1.0);
    for (dst, src) in p.0.iter_mut().zip(color) {
        *dst = mix(*dst, src, a);
    }
    p.0[3] = mix(p.0[3], 255, a);
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a blend of two bytes stays in 0..=255
fn mix(dst: u8, src: u8, a: f32) -> u8 {
    (f32::from(dst) * (1.0 - a) + f32::from(src) * a).round() as u8
}
