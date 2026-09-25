//! Real text, rasterised from the system UI font.
//!
//! Why ab_glyph and not fontdue: fontdue converts every glyph outline in the file the moment the
//! font is loaded. Measured on this machine, Segoe UI Semibold is a 0.93 MB file that cost 19.4 MB
//! of resident memory to parse — for a pill that draws about six distinct characters. ab_glyph
//! outlines a glyph only when it is asked for, so the cost is the file plus the handful of glyphs
//! actually used.
//!
//! Glyphs are cached per (character, whole pixel size): the pill redraws on every animation frame
//! while the content scale moves, and rasterising "21%" sixty times a second would be silly.

use crate::paint::Canvas;
use ab_glyph::{Font, FontVec, ScaleFont};
use std::collections::HashMap;

/// Segoe UI, in the weight notch.html asks for. Semibold first (`font-weight:600` on `.pct`),
/// falling back to regular, then to Arial — a machine without any of these is not one this app runs
/// on, but a missing font must not take the pill down with it.
const CANDIDATES: [&str; 3] = [
    r"C:\Windows\Fonts\seguisb.ttf",
    r"C:\Windows\Fonts\segoeui.ttf",
    r"C:\Windows\Fonts\arial.ttf",
];

pub struct Text {
    font: FontVec,
    cache: HashMap<(char, u32), Glyph>,
}

#[derive(Clone)]
struct Glyph {
    /// Coverage, one byte per pixel, `w` by `h`. Empty for a space.
    alpha: Vec<u8>,
    w: i32,
    h: i32,
    /// Offset from the pen position (on the baseline) to the bitmap's top-left corner.
    left: i32,
    top: i32,
    advance: f32,
}

impl Text {
    pub fn system() -> Option<Text> {
        Self::first_of(&CANDIDATES)
    }

    /// Segoe UI Regular, for the card's body text (`Typography.cardBody` is `.regular`).
    pub fn regular() -> Option<Text> {
        Self::first_of(&CANDIDATES[1..])
    }

    fn first_of(paths: &[&str]) -> Option<Text> {
        for path in paths.iter().copied() {
            let Ok(bytes) = std::fs::read(path) else { continue };
            if let Ok(font) = FontVec::try_from_vec(bytes) {
                return Some(Text { font, cache: HashMap::new() });
            }
        }
        None
    }

    fn glyph(&mut self, ch: char, px: f32) -> Glyph {
        // Keyed on whole pixels: the content scale moves continuously, and re-rasterising for a
        // quarter-pixel difference nobody can see would defeat the cache entirely.
        let key = (ch, px.round().max(1.0) as u32);
        if let Some(g) = self.cache.get(&key) {
            return g.clone();
        }
        // `px` is an em size, as CSS and SwiftUI mean it. ab_glyph's scale is the height of the
        // font's ascent-to-descent box instead, which for Segoe UI is 1.33 em: passing the em size
        // straight through drew every string at three quarters of the size asked for.
        let em = self.font.units_per_em().unwrap_or(1.0);
        let size = key.1 as f32 * self.font.height_unscaled() / em;
        let scaled = self.font.as_scaled(size);
        let id = self.font.glyph_id(ch);
        let advance = scaled.h_advance(id);

        let mut g = Glyph { alpha: Vec::new(), w: 0, h: 0, left: 0, top: 0, advance };
        if let Some(outlined) = self.font.outline_glyph(id.with_scale(size)) {
            let b = outlined.px_bounds();
            let (w, h) = ((b.width().ceil()) as i32, (b.height().ceil()) as i32);
            if w > 0 && h > 0 {
                let mut alpha = vec![0u8; (w * h) as usize];
                outlined.draw(|x, y, c| {
                    let (x, y) = (x as i32, y as i32);
                    if x >= 0 && x < w && y >= 0 && y < h {
                        alpha[(y * w + x) as usize] = (c.clamp(0.0, 1.0) * 255.0) as u8;
                    }
                });
                // ab_glyph reports bounds relative to the pen on the baseline: min.y is negative
                // above it, which is exactly the offset the canvas wants.
                g = Glyph { alpha, w, h, left: b.min.x as i32, top: b.min.y as i32, advance };
            }
        }
        self.cache.insert(key, g.clone());
        g
    }

    /// Total advance of `s`, for centring before anything is drawn.
    pub fn width(&mut self, s: &str, px: f32) -> f32 {
        s.chars().map(|c| self.glyph(c, px).advance).sum()
    }

    /// Draws `s` with its baseline at `baseline` and its centre at `cx`.
    pub fn centred(&mut self, c: &mut Canvas, s: &str, cx: f32, baseline: f32, px: f32, rgb: [f32; 3], a: f32) {
        let left = cx - self.width(s, px) / 2.0;
        self.left_aligned(c, s, left, baseline, px, rgb, a);
    }

    /// Draws `s` from `left`, baseline at `baseline`. Returns where the pen ended.
    pub fn left_aligned(&mut self, c: &mut Canvas, s: &str, left: f32, baseline: f32, px: f32, rgb: [f32; 3], a: f32) -> f32 {
        let mut pen = left;
        for ch in s.chars() {
            let g = self.glyph(ch, px);
            if !g.alpha.is_empty() {
                c.mask_rect(
                    &g.alpha,
                    g.w,
                    g.h,
                    (pen + g.left as f32).round() as i32,
                    (baseline + g.top as f32).round() as i32,
                    rgb,
                    a,
                );
            }
            pen += g.advance;
        }
        pen
    }

    /// Cuts `s` down to fit `max` px, ending in an ellipsis when it had to.
    pub fn elide(&mut self, s: &str, px: f32, max: f32) -> String {
        if self.width(s, px) <= max {
            return s.to_string();
        }
        let dots = self.width("…", px);
        let mut out = String::new();
        let mut used = 0.0;
        for ch in s.chars() {
            let w = self.glyph(ch, px).advance;
            if used + w + dots > max {
                break;
            }
            out.push(ch);
            used += w;
        }
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> Text {
        Text::system().expect("no system UI font on this machine")
    }

    /// Guards the reason this module uses ab_glyph at all. fontdue parsed the same file into 19.4 MB
    /// of glyph geometry up front; lazy outlining should stay close to the file's own size.
    #[test]
    fn loading_a_font_costs_about_the_file_and_no_more() {
        fn rss_mb() -> f64 {
            use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
            use windows::Win32::System::Threading::GetCurrentProcess;
            let mut c = PROCESS_MEMORY_COUNTERS::default();
            unsafe {
                let _ = K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32);
            }
            c.WorkingSetSize as f64 / (1024.0 * 1024.0)
        }
        let base = rss_mb();
        let mut t = font();
        let _ = t.width("100%", 15.0);
        let cost = rss_mb() - base;
        println!("carregar a fonte custou {cost:.1} MB");
        assert!(cost < 8.0, "the font should not cost 19 MB again, cost {cost:.1} MB");
    }

    #[test]
    fn a_wider_string_measures_wider() {
        let mut t = font();
        let short = t.width("2%", 16.0);
        let long = t.width("100%", 16.0);
        assert!(long > short, "100% should be wider than 2%: {long} vs {short}");
        assert!(short > 0.0);
    }

    #[test]
    fn bigger_type_measures_bigger() {
        let mut t = font();
        assert!(t.width("21%", 24.0) > t.width("21%", 12.0));
    }

    #[test]
    fn text_lands_inside_the_canvas_and_is_centred() {
        let mut t = font();
        let mut c = Canvas::new(80, 40);
        t.centred(&mut c, "21%", 40.0, 28.0, 18.0, [1.0, 1.0, 1.0], 1.0);

        let lit: Vec<usize> = c.buf.chunks(4).enumerate().filter(|(_, px)| px[3] > 32).map(|(i, _)| i).collect();
        assert!(!lit.is_empty(), "nothing was drawn");
        let min_x = lit.iter().map(|i| i % 80).min().unwrap();
        let max_x = lit.iter().map(|i| i % 80).max().unwrap();
        let mid = (min_x + max_x) / 2;
        assert!((mid as i32 - 40).abs() <= 3, "text centre drifted to {mid}, expected about 40");
    }

    #[test]
    fn text_sits_above_its_baseline() {
        // Catches the sign of the vertical offset: get it wrong and the digits hang below the line.
        let mut t = font();
        let mut c = Canvas::new(60, 60);
        t.centred(&mut c, "8", 30.0, 40.0, 20.0, [1.0, 1.0, 1.0], 1.0);
        let rows: Vec<usize> = c.buf.chunks(4).enumerate().filter(|(_, px)| px[3] > 32).map(|(i, _)| i / 60).collect();
        assert!(!rows.is_empty(), "nothing was drawn");
        assert!(*rows.iter().max().unwrap() <= 41, "ink fell below the baseline");
        assert!(*rows.iter().min().unwrap() < 40, "nothing was drawn above the baseline");
    }

    #[test]
    fn the_cache_returns_identical_glyphs() {
        let mut t = font();
        let first = t.width("88%", 17.0);
        let again = t.width("88%", 17.0);
        assert_eq!(first, again);
        assert!(t.cache.len() <= 3, "three distinct characters should cache three glyphs");
    }

    #[test]
    fn a_space_advances_without_drawing() {
        let mut t = font();
        let mut c = Canvas::new(40, 24);
        t.centred(&mut c, " ", 20.0, 18.0, 14.0, [1.0, 1.0, 1.0], 1.0);
        assert!(c.buf.iter().all(|b| *b == 0), "a space should leave no ink");
        assert!(t.width(" ", 14.0) > 0.0, "but it should still take room");
    }

    #[test]
    fn eliding_only_happens_when_it_has_to() {
        let mut t = font();
        let short = "ok";
        assert_eq!(t.elide(short, 14.0, 500.0), short, "text that fits is left alone");

        let long = "a very long session title that will not fit";
        let cut = t.elide(long, 14.0, 80.0);
        assert!(cut.ends_with('…'), "a cut string should say so: {cut}");
        assert!(cut.chars().count() < long.chars().count());
        assert!(t.width(&cut, 14.0) <= 80.0, "the elided string must fit: {cut}");
    }
}
