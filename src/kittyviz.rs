//! Wave visualiser drawn as a real image through the kitty graphics protocol.
//! The frame is written to a temp file and the terminal reads it by path (t=t),
//! so no big base64 payload goes over the tty each frame.
use crossterm::terminal::window_size;
use std::{
    io::{self, Write},
    path::PathBuf,
};

pub fn supported(mode: &str) -> bool {
    if mode == "off" {
        return false;
    }
    std::env::var_os("KITTY_WINDOW_ID").is_some()
        || std::env::var("TERM")
            .map(|t| t.contains("kitty"))
            .unwrap_or(false)
}

/// Pixel size of one terminal cell (falls back to a typical 9x18).
pub fn cell_px() -> (u32, u32) {
    if let Ok(ws) = window_size() {
        if ws.width > 0 && ws.height > 0 && ws.columns > 0 && ws.rows > 0 {
            return ((ws.width / ws.columns) as u32, (ws.height / ws.rows) as u32);
        }
    }
    (9, 18)
}

fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn frame_path() -> PathBuf {
    // kitty only accepts temp files whose name contains this marker and deletes them after reading
    std::env::temp_dir().join(format!("tty-graphics-protocol-rmp-{}", std::process::id()))
}

fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t.clamp(0.0, 1.0)) as u8;
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2])]
}

/// Stroke geometry, derived from the physical cell height so the line looks
/// the same on a 1080p screen and on a HiDPI 4K panel.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    /// Half thickness of the solid core, in physical pixels.
    pub half_width: f32,
    /// Radius of the outer glow, in physical pixels.
    pub glow: f32,
    /// Half thickness of the fading previous frame.
    pub ghost_width: f32,
}

impl Style {
    pub fn for_cell(cell_h: u32, scale: f32) -> Self {
        let scale = scale.clamp(0.3, 3.0);
        let hw = (cell_h as f32 * 0.09).clamp(1.1, 4.5) * scale;
        Self {
            half_width: hw,
            glow: (cell_h as f32 * 0.55).clamp(4.0, 18.0),
            ghost_width: (hw * 0.85).max(0.9),
        }
    }
}

impl Default for Style {
    fn default() -> Self {
        Self::for_cell(18, 1.0)
    }
}

/// Anti-aliased oscilloscope line on a transparent RGBA canvas, with glow and a fading ghost.
#[cfg(test)]
pub fn render_wave(w: u32, h: u32, wave: &[f32], ghost: &[f32], rgb: [u8; 3]) -> Vec<u8> {
    render_wave_styled(w, h, wave, ghost, rgb, Style::default())
}

/// Reuses `buf` to avoid allocating a multi-megabyte frame every tick.
pub fn render_wave_styled(
    w: u32,
    h: u32,
    wave: &[f32],
    ghost: &[f32],
    rgb: [u8; 3],
    style: Style,
) -> Vec<u8> {
    let mut buf = vec![0u8; (w * h * 4) as usize];
    render_wave_into(&mut buf, w, h, wave, ghost, rgb, style);
    buf
}

pub fn render_wave_into(
    buf: &mut Vec<u8>,
    w: u32,
    h: u32,
    wave: &[f32],
    ghost: &[f32],
    rgb: [u8; 3],
    style: Style,
) {
    let need = (w * h * 4) as usize;
    if buf.len() != need {
        buf.clear();
        buf.resize(need, 0);
    } else {
        buf.fill(0);
    }

    let n = wave.len().min(ghost.len());
    if n < 2 || w < 2 || h < 2 {
        return;
    }

    let (wf, hf) = (w as f32, h as f32);
    let amp = hf * 0.42;
    let xs = (n - 1) as f32 / (wf - 1.0);
    let core = style.half_width;
    let glow = style.glow;
    let ghost_hw = style.ghost_width;

    for x in 0..w {
        let t = x as f32 * xs;
        let i = (t.floor() as usize).min(n - 2);
        let f = t - i as f32;

        let av = wave[i] + (wave[i + 1] - wave[i]) * f;
        let gv = ghost[i] + (ghost[i + 1] - ghost[i]) * f;
        let line_y = hf * 0.5 - av * amp;
        let ghost_y = hf * 0.5 - gv * amp;

        // Slope correction so a steep line keeps a constant visual width.
        let norm = (1.0 + ((wave[i + 1] - wave[i]) * xs * amp).powi(2)).sqrt();
        let gnorm = (1.0 + ((ghost[i + 1] - ghost[i]) * xs * amp).powi(2)).sqrt();

        let col = mix(rgb, [255, 255, 255], 0.2 + 0.6 * av.abs().min(1.0));

        // Only touch the band the stroke can possibly reach.
        let top = (line_y.min(ghost_y) - glow - core - 2.0).floor().max(0.0) as u32;
        let bottom = (line_y.max(ghost_y) + glow + core + 2.0)
            .ceil()
            .min(hf - 1.0) as u32;

        for y in top..=bottom {
            let py = y as f32 + 0.5;
            let d = (py - line_y).abs() / norm;
            // Solid core with a one-pixel linear shoulder.
            let core_a = (core + 0.6 - d).clamp(0.0, 1.0);
            let glow_a = if d < core + glow {
                (1.0 - (d - core).max(0.0) / glow).powi(2) * 0.38
            } else {
                0.0
            };
            let gd = (py - ghost_y).abs() / gnorm;
            let ghost_a = if gd < ghost_hw + glow * 0.6 {
                (1.0 - (gd / (ghost_hw + glow * 0.6)).min(1.0)).powi(2) * 0.22
            } else {
                0.0
            };

            let alpha = (core_a.max(glow_a) + ghost_a).min(1.0);
            if alpha <= 0.003 {
                continue;
            }
            let idx = ((y * w + x) * 4) as usize;
            buf[idx..idx + 3].copy_from_slice(&col);
            buf[idx + 3] = (alpha * 255.0) as u8;
        }
    }
}

/// Place a frame over `cols` x `rows` cells starting at (col, row), without
/// deleting the previous one — the caller removes the old id after this returns,
/// which avoids a blank frame flicker between updates.
#[allow(clippy::too_many_arguments)]
pub fn show(
    out: &mut impl Write,
    col: u16,
    row: u16,
    cols: u16,
    rows: u16,
    w: u32,
    h: u32,
    rgba: &[u8],
    id: u32,
) -> io::Result<()> {
    let path = frame_path();
    std::fs::write(&path, rgba)?;
    let payload = b64(path.to_string_lossy().as_bytes());
    write!(out, "\x1b[{};{}H", row + 1, col + 1)?;
    write!(
        out,
        "\x1b_Ga=T,f=32,t=t,s={w},v={h},c={cols},r={rows},i={id},C=1,q=2;{payload}\x1b\\"
    )?;
    out.flush()
}

/// Show a PNG file over `cols` x `rows` cells (used for cover art). Kitty reads the file itself.
pub fn show_png(
    out: &mut impl Write,
    col: u16,
    row: u16,
    cols: u16,
    rows: u16,
    path: &std::path::Path,
    id: u32,
) -> io::Result<()> {
    let payload = b64(path.to_string_lossy().as_bytes());
    write!(out, "\x1b[{};{}H", row + 1, col + 1)?;
    write!(out, "\x1b_Ga=d,d=i,i={id},q=2\x1b\\")?;
    write!(
        out,
        "\x1b_Ga=T,f=100,t=f,c={cols},r={rows},i={id},C=1,q=2;{payload}\x1b\\"
    )?;
    out.flush()
}

/// Compute how many terminal cells an image of `iw`x`ih` pixels should occupy to
/// fit into a `box_cols`x`box_rows` area without distorting its aspect ratio.
pub fn fit_cells(
    iw: u32,
    ih: u32,
    box_cols: u16,
    box_rows: u16,
    cell_w: u32,
    cell_h: u32,
) -> (u16, u16) {
    if iw == 0 || ih == 0 || box_cols == 0 || box_rows == 0 {
        return (box_cols.max(1), box_rows.max(1));
    }
    let avail_w = box_cols as f32 * cell_w.max(1) as f32;
    let avail_h = box_rows as f32 * cell_h.max(1) as f32;
    let scale = (avail_w / iw as f32).min(avail_h / ih as f32);
    let cols = (iw as f32 * scale / cell_w.max(1) as f32).round() as u16;
    let rows = (ih as f32 * scale / cell_h.max(1) as f32).round() as u16;
    (cols.clamp(1, box_cols), rows.clamp(1, box_rows))
}

/// Remove a specific image id (cover art uses ids >= 2).
pub fn clear_id(out: &mut impl Write, id: u32) -> io::Result<()> {
    write!(out, "\x1b_Ga=d,d=i,i={id},q=2\x1b\\")?;
    out.flush()
}

pub fn clear_all(out: &mut impl Write) -> io::Result<()> {
    write!(out, "\x1b_Ga=d,d=A,q=2\x1b\\")?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_known_values() {
        assert_eq!(b64(b"Man"), "TWFu");
        assert_eq!(b64(b"Ma"), "TWE=");
        assert_eq!(b64(b"M"), "TQ==");
    }

    fn wave(n: usize) -> Vec<f32> {
        (0..n).map(|i| (i as f32 / 20.0).sin() * 0.8).collect()
    }

    #[test]
    fn wave_renders_expected_size_and_pixels() {
        let w = wave(512);
        let ghost = vec![0.0; 512];
        let (width, height) = (300, 120);
        let buf = render_wave(width, height, &w, &ghost, [127, 214, 190]);
        assert_eq!(buf.len(), (width * height * 4) as usize);
        assert!(
            buf.chunks(4).any(|p| p[3] > 200),
            "line should be drawn opaque somewhere"
        );
        assert_eq!(buf[3], 0, "corner far from line stays transparent");
    }

    #[test]
    fn thicker_style_paints_more_pixels() {
        let w = wave(512);
        let ghost = vec![0.0; 512];
        let count = |scale: f32| {
            let style = Style::for_cell(20, scale);
            let buf = render_wave_styled(400, 160, &w, &ghost, [255, 255, 255], style);
            buf.chunks(4).filter(|p| p[3] > 8).count()
        };
        let thin = count(0.5);
        let thick = count(1.6);
        assert!(
            thick > thin,
            "scale should change stroke thickness ({thin} vs {thick})"
        );
    }

    #[test]
    fn style_scales_with_cell_height() {
        let small = Style::for_cell(16, 1.0);
        let big = Style::for_cell(40, 1.0);
        assert!(big.half_width > small.half_width);
        assert!(big.glow > small.glow);
    }

    #[test]
    fn fit_cells_preserves_aspect_and_box() {
        // Square image fills the box without distortion (cells are 2x taller than wide).
        assert_eq!(fit_cells(512, 512, 20, 10, 10, 20), (20, 10));
        // Wide image uses the full width.
        assert_eq!(fit_cells(1000, 500, 20, 10, 10, 20), (20, 5));
        // Never exceeds the box.
        let (c, r) = fit_cells(4000, 4000, 10, 4, 8, 16);
        assert!(c <= 10 && r <= 4);
    }

    #[test]
    fn reuse_buffer_same_length() {
        let mut buf = Vec::new();
        let w = wave(64);
        let g = wave(64);
        render_wave_into(&mut buf, 50, 10, &w, &g, [1, 2, 3], Style::default());
        assert_eq!(buf.len(), 50 * 10 * 4);
        render_wave_into(&mut buf, 10, 5, &w, &g, [1, 2, 3], Style::default());
        assert_eq!(buf.len(), 10 * 5 * 4);
    }
}
