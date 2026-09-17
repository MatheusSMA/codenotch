//! Everything that turns numbers into pixels: the pill, the fillets, the rings and the arcs.
//!
//! No GDI anywhere. GDI does not write the alpha channel, and a layered window is per-pixel alpha
//! from end to end, so every shape here is rasterised by hand into a premultiplied BGRA buffer.
//! Text lives in `text.rs`, rasterised from the system UI font.

/// The three bands the rings share with the tray: comfortable, getting close, nearly spent.
pub fn band(used: f32) -> [f32; 3] {
    if used < 0.5 {
        [0.29, 0.87, 0.50] // green  #4ade80
    } else if used < 0.8 {
        [0.98, 0.80, 0.08] // amber  #facc15
    } else {
        [0.97, 0.44, 0.44] // red    #f87171
    }
}

/// A canvas of premultiplied-ready BGRA. Alpha is composited source-over as each shape lands, and
/// the buffer is handed to UpdateLayeredWindow untouched.
pub struct Canvas {
    pub w: i32,
    pub h: i32,
    pub buf: Vec<u8>,
}

impl Canvas {
    pub fn new(w: i32, h: i32) -> Self {
        Canvas { w, h, buf: vec![0; (w * h * 4) as usize] }
    }

    pub fn clear(&mut self) {
        self.buf.fill(0);
    }

    pub fn put(&mut self, x: i32, y: i32, rgb: [f32; 3], a: f32) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h || a <= 0.0 {
            return;
        }
        let i = ((y * self.w + x) * 4) as usize;
        let a = a.clamp(0.0, 1.0);
        let db = self.buf[i] as f32 / 255.0;
        let dg = self.buf[i + 1] as f32 / 255.0;
        let dr = self.buf[i + 2] as f32 / 255.0;
        let da = self.buf[i + 3] as f32 / 255.0;
        let mix = |s: f32, d: f32| s * a + d * (1.0 - a);
        self.buf[i] = (mix(rgb[2], db) * 255.0) as u8;
        self.buf[i + 1] = (mix(rgb[1], dg) * 255.0) as u8;
        self.buf[i + 2] = (mix(rgb[0], dr) * 255.0) as u8;
        self.buf[i + 3] = ((a + da * (1.0 - a)) * 255.0) as u8;
    }

    /// Filled rounded rectangle with a 1 px anti-aliased edge, plus an optional hairline stroke.
    /// `alpha` multiplies the whole shape, so a panel and everything drawn into it can fade
    /// together instead of the contents arriving first.
    pub fn round_rect(&mut self, cx: f32, cy: f32, hw: f32, hh: f32, r: f32, rgb: [f32; 3], stroke: Option<[f32; 3]>, alpha: f32) {
        let x0 = (cx - hw - 2.0).floor().max(0.0) as i32;
        let x1 = (cx + hw + 2.0).ceil().min(self.w as f32) as i32;
        let y0 = (cy - hh - 2.0).floor().max(0.0) as i32;
        let y1 = (cy + hh + 2.0).ceil().min(self.h as f32) as i32;
        for y in y0..y1 {
            for x in x0..x1 {
                let px = x as f32 + 0.5 - cx;
                let py = y as f32 + 0.5 - cy;
                let d = sd_round_rect(px, py, hw, hh, r);
                self.put(x, y, rgb, (1.0 - smoothstep(-0.5, 0.5, d)) * alpha);
                if let Some(s) = stroke {
                    let edge = (1.0 - smoothstep(0.0, 1.2, (d + 0.6).abs())) * 0.8 * alpha;
                    self.put(x, y, s, edge);
                }
            }
        }
    }

    /// A ring, or an arc of one. `from`/`to` are turns clockwise from 12 o'clock, in 0..1.
    pub fn arc(&mut self, cx: f32, cy: f32, r: f32, width: f32, rgb: [f32; 3], alpha: f32, from: f32, to: f32) {
        use std::f32::consts::PI;
        if to <= from {
            return;
        }
        let full = to - from >= 1.0;
        let reach = r + width / 2.0 + 2.0;
        let x0 = (cx - reach).floor().max(0.0) as i32;
        let x1 = (cx + reach).ceil().min(self.w as f32) as i32;
        let y0 = (cy - reach).floor().max(0.0) as i32;
        let y1 = (cy + reach).ceil().min(self.h as f32) as i32;
        let (a0, a1) = (from * 2.0 * PI, to * 2.0 * PI);
        let feather = 0.5 / r.max(1.0); // roughly one pixel, expressed as an angle
        for y in y0..y1 {
            for x in x0..x1 {
                let px = x as f32 + 0.5 - cx;
                let py = y as f32 + 0.5 - cy;
                let dist = px.hypot(py);
                let band = 1.0 - smoothstep(width / 2.0 - 0.5, width / 2.0 + 0.5, (dist - r).abs());
                if band <= 0.0 {
                    continue;
                }
                let cover = if full {
                    1.0
                } else {
                    // Angle measured clockwise from 12 o'clock, which is how usage arcs read
                    let ang = (px.atan2(-py)).rem_euclid(2.0 * PI);
                    let a = if ang < a0 { ang + 2.0 * PI } else { ang };
                    smoothstep(a0, a0 + feather, a) * (1.0 - smoothstep(a1 - feather, a1, a))
                };
                self.put(x, y, rgb, band * cover * alpha);
            }
        }
    }

    /// A concave corner filling the gap between a panel edge and the screen edge: an r-by-r square
    /// hugging the right edge with a circle of radius `r` punched out of it, centred on the corner
    /// furthest from the screen. What survives is the inward-curving arc, and the panel stops
    /// looking like a rectangle parked on top of the desktop.
    ///
    /// `edge_y` is the panel's own top (with `above` true) or bottom edge; the square sits outside
    /// it. `stroke` continues the panel's hairline around the curve so there is no seam.
    pub fn fillet(&mut self, right: f32, edge_y: f32, r: f32, above: bool, rgb: [f32; 3], stroke: [f32; 3]) {
        if r <= 0.0 {
            return;
        }
        // Overlapping the panel's own edge, not stopping at it. The panel draws a hairline all the
        // way round, including along the edge the fillet joins, and leaving that line exposed puts
        // a visible rule across the join. notch.html solves it the same way: "covers the 1 px
        // stroke along the pill's top edge so there is no seam".
        const OVERLAP: f32 = 1.5;

        // Everything below is a function of `dy`: how far the pixel is from the panel's edge, going
        // outward. Writing it this way rather than in absolute coordinates is what makes the two
        // ends mirror images. They did not used to: with `py > edge_y` as the test, the half-pixel
        // sample offset put the edge row inside the overlap at the top and outside it at the
        // bottom, and one row of difference showed up as tens of pixels of width.
        let ccx = right - r;
        let x0 = ccx.floor().max(0.0) as i32;
        let x1 = right.ceil().min(self.w as f32) as i32;
        let (y0, y1) = if above {
            (edge_y - r, edge_y + OVERLAP)
        } else {
            (edge_y - OVERLAP, edge_y + r)
        };
        for y in y0.floor().max(0.0) as i32..y1.ceil().min(self.h as f32) as i32 {
            let py = y as f32 + 0.5;
            let dy = if above { edge_y - py } else { py - edge_y };
            for x in x0..x1 {
                // The punched-out circle sits `r` out from the edge, inset `r` from the screen.
                let d = (x as f32 + 0.5 - ccx).hypot(dy - r);
                // Inside the panel, in the overlap strip, the fill is unconditional: that strip
                // exists to bury the hairline, not to be shaped by the arc.
                let cover = if dy < 0.0 { 1.0 } else { smoothstep(r - 0.5, r + 0.5, d) };
                self.put(x, y, rgb, cover);
                // The arc carries the panel's outline onward, so the shape keeps its edge against a
                // dark wallpaper. Only along the curve, never across the overlap strip.
                if dy >= 0.0 {
                    let seam = 1.0 - smoothstep(0.0, 1.0, (d - r).abs());
                    self.put(x, y, stroke, seam * 0.55);
                }
            }
        }
    }

    /// Composite an alpha mask — a rasterised provider mark — tinted to `rgb`. The mask carries the
    /// shape, the colour comes from here, so one raster serves both the normal and the dimmed
    /// state. `ox`/`oy` are the mask's top-left corner on this canvas.
    pub fn mask(&mut self, alpha: &[u8], size: i32, ox: i32, oy: i32, rgb: [f32; 3], a: f32) {
        self.mask_rect(alpha, size, size, ox, oy, rgb, a);
    }

    /// The same, for a mask that is not square — a rasterised glyph, whose box is whatever the
    /// letter needs.
    pub fn mask_rect(&mut self, alpha: &[u8], w: i32, h: i32, ox: i32, oy: i32, rgb: [f32; 3], a: f32) {
        if w <= 0 || h <= 0 || alpha.len() < (w * h) as usize {
            return;
        }
        for my in 0..h {
            for mx in 0..w {
                let v = alpha[(my * w + mx) as usize];
                if v > 0 {
                    self.put(ox + mx, oy + my, rgb, a * v as f32 / 255.0);
                }
            }
        }
    }

}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn sd_round_rect(px: f32, py: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = px.abs() - hw + r;
    let qy = py.abs() - hh + r;
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_change_at_the_documented_thresholds() {
        assert_eq!(band(0.49), band(0.0));
        assert_ne!(band(0.51), band(0.49));
        assert_ne!(band(0.81), band(0.79));
    }

    #[test]
    fn a_full_arc_paints_and_an_empty_one_does_not() {
        let mut c = Canvas::new(40, 40);
        c.arc(20.0, 20.0, 15.0, 3.0, [1.0, 1.0, 1.0], 1.0, 0.0, 0.0);
        assert!(c.buf.iter().all(|b| *b == 0), "a zero-length arc must draw nothing");
        c.arc(20.0, 20.0, 15.0, 3.0, [1.0, 1.0, 1.0], 1.0, 0.0, 1.0);
        assert!(c.buf.iter().any(|b| *b > 0), "a full ring must draw something");
    }

    #[test]
    fn a_quarter_arc_lands_on_the_right_hand_side() {
        // 0..0.25 turns clockwise from 12 o'clock covers the top-right quadrant only
        let mut c = Canvas::new(60, 60);
        c.arc(30.0, 30.0, 20.0, 4.0, [1.0, 1.0, 1.0], 1.0, 0.0, 0.25);
        let at = |x: i32, y: i32| c.buf[((y * 60 + x) * 4 + 3) as usize];
        // Sampled mid-arc, not at 3 o'clock: that is the arc's own end edge, where the one-pixel
        // feather takes coverage to zero by design.
        assert!(at(44, 16) > 0, "the 45 degree point should be painted");
        assert!(at(30, 10) > 0, "12 o'clock, the start, should be painted");
        assert!(at(10, 30) == 0, "9 o'clock should be bare");
        assert!(at(30, 50) == 0, "6 o'clock should be bare");
    }
}
