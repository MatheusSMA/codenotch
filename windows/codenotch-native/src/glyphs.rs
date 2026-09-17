//! The provider marks, rasterised from the same SVGs the web build ships.
//!
//! They are monochrome by design — every one is a single path filled with `currentColor` — so only
//! the alpha channel is kept and the colour is applied when the mark is composited. That means one
//! raster serves the normal and the dimmed state, and the cache is a quarter the size.

// Both come through resvg's re-exports, so the versions can never drift apart.
use resvg::{tiny_skia, usvg};
use std::collections::HashMap;

// Embedded rather than read from disk: the binary has to run from anywhere, and these are the
// files the upstream app already vendors. Most of each file is a C2PA metadata blob that the
// parser skips, which is why 9 KB of SVG describes one small path.
const CLAUDE: &str = include_str!("../../codenotch/glyphs/claude.svg");
const CODEX: &str = include_str!("../../codenotch/glyphs/codex.svg");
const CURSOR: &str = include_str!("../../codenotch/glyphs/cursor.svg");
const GEMINI: &str = include_str!("../../codenotch/glyphs/gemini.svg");

pub fn svg_for(provider: &str) -> Option<&'static str> {
    Some(match provider {
        "claude" => CLAUDE,
        "codex" => CODEX,
        "cursor" => CURSOR,
        // Antigravity is Gemini underneath, and ships the Gemini mark upstream too
        "antigravity" => GEMINI,
        _ => return None,
    })
}

/// A square alpha mask: `size` by `size`, one byte per pixel, 0 where the mark is absent.
pub struct Mark {
    pub size: u32,
    pub alpha: Vec<u8>,
}

/// Rasterising is milliseconds but not free, and the size only changes when the display scale or
/// the user's size preference does, so each (provider, size) pair is kept once.
#[derive(Default)]
pub struct Marks {
    cache: HashMap<(String, u32), Option<Mark>>,
}

impl Marks {
    pub fn get(&mut self, provider: &str, size: u32) -> Option<&Mark> {
        let key = (provider.to_string(), size);
        if !self.cache.contains_key(&key) {
            let mark = svg_for(provider).and_then(|svg| rasterize(svg, size));
            self.cache.insert(key.clone(), mark);
        }
        self.cache.get(&key).and_then(|m| m.as_ref())
    }
}

/// Render one mark to an alpha mask.
///
/// Two substitutions first. `currentColor` has no meaning outside a document that defines it, so it
/// becomes opaque white and the alpha is all that survives. The `1em` width and height would be
/// resolved against a default font size, which would quietly rasterise these at 16 px whatever was
/// asked for; dropping them leaves the viewBox as the only size, which is what we want to scale.
fn rasterize(svg: &str, size: u32) -> Option<Mark> {
    if size == 0 {
        return None;
    }
    let prepared = svg
        .replace("currentColor", "#ffffff")
        .replace("width=\"1em\"", "")
        .replace("height=\"1em\"", "");

    let tree = usvg::Tree::from_str(&prepared, &usvg::Options::default()).ok()?;
    let src = tree.size();
    let longest = src.width().max(src.height());
    if longest <= 0.0 {
        return None;
    }

    let mut pixmap = tiny_skia::Pixmap::new(size, size)?;
    let k = size as f32 / longest;
    // Centred, so a mark whose viewBox is not square still sits in the middle of its cell
    let dx = (size as f32 - src.width() * k) / 2.0;
    let dy = (size as f32 - src.height() * k) / 2.0;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_translate(dx, dy).pre_scale(k, k),
        &mut pixmap.as_mut(),
    );

    Some(Mark { size, alpha: pixmap.data().chunks(4).map(|px| px[3]).collect() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_provider_the_pill_can_show_has_a_mark() {
        for p in ["claude", "codex", "cursor", "antigravity"] {
            assert!(svg_for(p).is_some(), "{p} has no mark");
        }
        assert!(svg_for("nonsense").is_none());
    }

    #[test]
    fn a_mark_rasterises_to_something_visible() {
        let m = Marks::default().get("claude", 26).map(|m| m.alpha.clone()).unwrap();
        assert_eq!(m.len(), 26 * 26);
        let ink = m.iter().filter(|a| **a > 32).count();
        assert!(ink > 40, "the Claude mark should cover a fair part of its box, got {ink} px");
        assert!(ink < 26 * 26, "it should not be a solid square");
    }

    #[test]
    fn marks_fill_their_box_at_any_size() {
        // The 1em width/height must not pin the raster to a default font size.
        let mut marks = Marks::default();
        for size in [16u32, 26, 52] {
            let m = marks.get("codex", size).unwrap();
            assert_eq!(m.size, size);
            assert_eq!(m.alpha.len() as u32, size * size);
            assert!(m.alpha.iter().any(|a| *a > 32), "nothing drawn at {size} px");
        }
    }

    #[test]
    fn the_cache_hands_back_the_same_raster() {
        let mut marks = Marks::default();
        let first = marks.get("claude", 26).unwrap().alpha.clone();
        let again = marks.get("claude", 26).unwrap().alpha.clone();
        assert_eq!(first, again);
    }

    #[test]
    fn an_unknown_provider_is_absent_rather_than_a_panic() {
        assert!(Marks::default().get("nonsense", 26).is_none());
    }
}
