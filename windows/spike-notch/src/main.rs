//! Spike: can a layered Win32 window replace the WebView2 notch?
//!
//! Two questions, one binary:
//!   1. How much RAM does a per-pixel-alpha always-on-top window cost, against
//!      WebView2's measured 415 MB for the same job?
//!   2. Is a smooth 60 fps arc affordable here? The web version had to be capped
//!      to about 10 fps because the SVG transform re-rasterised on the main thread.
//!
//! Draws the pill and one turning arc, and prints its own working set every second.

use std::f32::consts::PI;
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const W: i32 = 90;
const H: i32 = 220;
const FPS: u64 = 60;

/// Pill geometry, mirroring notch.html: 70 px wide, rounded on the left only.
const PILL_W: f32 = 70.0;
const RADIUS: f32 = 20.0;

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Signed distance to a rounded rectangle, used for a 1 px anti-aliased edge.
fn sd_round_rect(px: f32, py: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = px.abs() - hw + r;
    let qy = py.abs() - hh + r;
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// Source-over into premultiplied BGRA, which is what ULW_ALPHA expects.
fn put(buf: &mut [u8], x: i32, y: i32, rgb: [f32; 3], a: f32) {
    if x < 0 || y < 0 || x >= W || y >= H || a <= 0.0 {
        return;
    }
    let i = ((y * W + x) * 4) as usize;
    let a = a.clamp(0.0, 1.0);
    let db = buf[i] as f32 / 255.0;
    let dg = buf[i + 1] as f32 / 255.0;
    let dr = buf[i + 2] as f32 / 255.0;
    let da = buf[i + 3] as f32 / 255.0;
    let blend = |s: f32, d: f32| s * a + d * (1.0 - a);
    buf[i] = (blend(rgb[2], db) * 255.0) as u8;
    buf[i + 1] = (blend(rgb[1], dg) * 255.0) as u8;
    buf[i + 2] = (blend(rgb[0], dr) * 255.0) as u8;
    buf[i + 3] = ((a + da * (1.0 - a)) * 255.0) as u8;
}

fn render(buf: &mut [u8], phase: f32) {
    buf.fill(0);
    let cx = W as f32 - PILL_W / 2.0; // the pill hugs the right edge, as on screen
    let cy = H as f32 / 2.0;

    // Pill body, pushed right so only the left corners round.
    for y in 0..H {
        for x in 0..W {
            let px = x as f32 + 0.5 - cx;
            let py = y as f32 + 0.5 - cy;
            let d = sd_round_rect(px - RADIUS, py, PILL_W / 2.0 + RADIUS, H as f32 / 2.0 - 10.0, RADIUS);
            put(buf, x, y, [0.0, 0.0, 0.0], 1.0 - smoothstep(-0.5, 0.5, d));
            // Hairline stroke: the outline cue the web version keeps for black wallpapers
            let stroke = (1.0 - smoothstep(0.0, 1.2, (d + 0.6).abs())) * 0.65;
            put(buf, x, y, [0.18, 0.18, 0.18], stroke);
        }
    }

    // The turning arc: 28 % of a circle, the fraction notch.html uses.
    let rr = 19.0_f32;
    let thick = 2.5_f32;
    let sweep = 2.0 * PI * 0.28;
    let edge = 0.06;
    for y in 0..H {
        for x in 0..W {
            let px = x as f32 + 0.5 - cx;
            let py = y as f32 + 0.5 - cy;
            let dist = px.hypot(py);
            let band = 1.0 - smoothstep(thick / 2.0 - 0.5, thick / 2.0 + 0.5, (dist - rr).abs());
            if band <= 0.0 {
                continue;
            }
            let ang = (py.atan2(px) + PI / 2.0 - phase).rem_euclid(2.0 * PI);
            let inside = smoothstep(0.0, edge, ang) * (1.0 - smoothstep(sweep - edge, sweep, ang));
            put(buf, x, y, [0.91, 0.91, 0.92], band * inside);
        }
    }
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if m == WM_DESTROY {
        PostQuitMessage(0);
        return LRESULT(0);
    }
    DefWindowProcW(h, m, w, l)
}

/// Read from the process itself, so the number needs no external tool.
fn working_set_mb() -> f64 {
    use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::GetCurrentProcess;
    let mut c = PROCESS_MEMORY_COUNTERS::default();
    unsafe {
        let _ = K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut c,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        );
    }
    c.WorkingSetSize as f64 / (1024.0 * 1024.0)
}

fn main() {
    unsafe {
        let hinst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap();
        let cls = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: w!("SpikeNotch"),
            ..Default::default()
        };
        RegisterClassW(&cls);

        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("SpikeNotch"),
            w!("spike"),
            WS_POPUP,
            1900,
            300,
            W,
            H,
            None,
            None,
            hinst,
            None,
        )
        .unwrap();
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);

        let screen = GetDC(None);
        let memdc = CreateCompatibleDC(screen);
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: W,
                biHeight: -H, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(screen, &bi, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
        let old = SelectObject(memdc, dib);
        let buf = std::slice::from_raw_parts_mut(bits as *mut u8, (W * H * 4) as usize);

        let start = std::time::Instant::now();
        let mut last_report = start;
        let mut frames = 0u32;
        let frame_time = std::time::Duration::from_millis(1000 / FPS);

        loop {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    let _ = SelectObject(memdc, old);
                    let _ = DeleteObject(dib);
                    let _ = DeleteDC(memdc);
                    ReleaseDC(None, screen);
                    return;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            let t = start.elapsed().as_secs_f32();
            render(buf, (t / 1.2) * 2.0 * PI); // one turn per 1.2 s, as in the web version

            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
                ..Default::default()
            };
            let size = SIZE { cx: W, cy: H };
            let src = POINT { x: 0, y: 0 };
            let _ = UpdateLayeredWindow(
                hwnd,
                screen,
                None,
                Some(&size),
                memdc,
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );

            frames += 1;
            if last_report.elapsed().as_secs() >= 1 {
                println!(
                    "{:.0}s  {} fps  working set {:.1} MB",
                    start.elapsed().as_secs_f32(),
                    frames,
                    working_set_mb()
                );
                frames = 0;
                last_report = std::time::Instant::now();
            }
            std::thread::sleep(frame_time);
        }
    }
}
