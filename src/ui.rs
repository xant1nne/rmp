use crate::{
    app::{App, Dl, InputKind, Mode, Tab},
    track::{Source, Track},
    visualizer::{Visualizer, VizMode, BARS, WAVE},
};
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    prelude::*,
    symbols::Marker,
    widgets::{
        canvas::{Canvas, Line as CLine},
        Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs,
    },
};

fn rgb(c: [u8; 3]) -> Color {
    Color::Rgb(c[0], c[1], c[2])
}

// ---------- helpers ----------

fn lerp(a: Color, b: Color, t: f32) -> Color {
    let (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) = (a, b) else {
        return a;
    };
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t.clamp(0.0, 1.0)) as u8;
    Color::Rgb(m(ar, br), m(ag, bg), m(ab, bb))
}

fn fit(s: &str, n: usize) -> String {
    let c = s.chars().count();
    if c <= n {
        s.to_string()
    } else if n == 0 {
        String::new()
    } else {
        s.chars().take(n - 1).collect::<String>() + "…"
    }
}

fn fmt_ms(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, secs % 3600 / 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

fn panel(title: Line<'static>, focused: bool, accent: Color, border: Color) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if focused { accent } else { border }))
        .title(title)
}

fn accent_of(app: &App) -> Color {
    rgb(app.theme.accent)
}

fn hash64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn hsv(h: f32, s: f32, v: f32) -> Color {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - f * s), v * (1.0 - (1.0 - f) * s));
    let (r, g, b) = match i as i32 % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    Color::Rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

// ---------- main draw ----------

pub fn draw(f: &mut Frame, app: &App) {
    let acc = accent_of(app);
    app.hits.borrow_mut().viz = Rect::default();
    let r = crate::layouts::regions(app.preset, f.size());
    let eff = app.preset.effective(f.size());
    draw_top(f, r.top, app, acc);
    draw_list(f, r.list, app, acc);
    if let Some(v) = r.viz {
        draw_viz(f, v, app, acc);
    }
    if let Some(l) = r.lyrics {
        draw_lyrics(f, l, app, acc);
    }
    if eff == crate::layouts::Preset::Minimal {
        draw_now_compact(f, r.now, app, acc);
    } else {
        draw_now_playing(f, r.now, app, acc);
    }
    draw_footer(f, r.footer, app, acc);
    if app.mode == Mode::Help {
        draw_help(f, f.size(), acc, &app.theme);
    }
}

fn draw_top(f: &mut Frame, area: Rect, app: &App, acc: Color) {
    let titles: Vec<String> = Tab::ALL
        .iter()
        .map(|t| format!(" {} {} ", t.idx() + 1, t.title()))
        .collect();
    let tabs_w: u16 = titles.iter().map(|t| t.chars().count() as u16).sum::<u16>() + 5;
    let cols = Layout::horizontal([
        Constraint::Length(7),
        Constraint::Length(tabs_w),
        Constraint::Min(0),
    ])
    .split(area);

    f.render_widget(
        Paragraph::new(Span::styled(
            " ♪ rmp ",
            Style::default().fg(acc).add_modifier(Modifier::BOLD),
        )),
        cols[0],
    );
    {
        let mut x = cols[1].x;
        let mut rects = vec![];
        for (i, t) in titles.iter().enumerate() {
            let w = t.chars().count() as u16;
            rects.push((
                Rect {
                    x,
                    y: cols[1].y,
                    width: w,
                    height: 1,
                },
                i,
            ));
            x += w;
        }
        app.hits.borrow_mut().tabs = rects;
    }
    f.render_widget(
        Tabs::new(titles.into_iter().map(Line::from).collect::<Vec<_>>())
            .select(app.tab.idx())
            .divider("")
            .style(Style::default().fg(rgb(app.theme.dim)))
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(acc)
                    .add_modifier(Modifier::BOLD),
            ),
        cols[1],
    );

    let rpc = if app.cfg.discord_app_id.is_empty() {
        Span::styled("rpc off", Style::default().fg(rgb(app.theme.dim)))
    } else if app.rpc_connected() {
        Span::styled("● rpc", Style::default().fg(acc))
    } else {
        Span::styled("○ rpc", Style::default().fg(rgb(app.theme.dim)))
    };
    let sep = || Span::styled("  ·  ", Style::default().fg(rgb(app.theme.outline)));
    let right = Line::from(vec![
        Span::styled(
            format!("vol {}%", (app.player.volume * 100.0).round()),
            Style::default().fg(rgb(app.theme.dim)),
        ),
        sep(),
        Span::styled(app.viz_mode.name(), Style::default().fg(rgb(app.theme.dim))),
        sep(),
        Span::styled(app.provider.name(), Style::default().fg(rgb(app.theme.dim))),
        sep(),
        rpc,
        Span::raw(" "),
    ]);
    f.render_widget(Paragraph::new(right).alignment(Alignment::Right), cols[2]);
}

// ---------- left panel ----------

fn draw_list(f: &mut Frame, area: Rect, app: &App, acc: Color) {
    let tab = app.tab;
    let count = match tab {
        Tab::Library => app.library.len(),
        Tab::Search => app.results.len(),
        Tab::Queue => app.queue.len(),
        Tab::Downloads => app.downloads.len(),
        Tab::Playlists => app.playlists.len(),
        Tab::Settings => app.settings_rows().len(),
    };
    let title = Line::from(vec![
        Span::styled(
            format!(" {} ", tab.title()),
            Style::default().fg(acc).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("{count} "), Style::default().fg(rgb(app.theme.dim))),
    ]);
    let block = panel(title, true, acc, rgb(app.theme.outline));
    app.hits.borrow_mut().list = area;
    app.hits.borrow_mut().list_offset = 0;
    let inner_w = area.width.saturating_sub(3) as usize; // borders + highlight bar

    let mut items: Vec<ListItem> = vec![];
    let mut empty_hint = "";

    match tab {
        Tab::Library | Tab::Search | Tab::Queue => {
            let playing_key = app.current.as_ref().map(Track::key);
            let qpos = app.queue.pos();
            for (i, t) in app.items(tab).iter().enumerate() {
                let is_cur = if tab == Tab::Queue {
                    qpos == Some(i)
                } else {
                    playing_key.as_deref() == Some(t.key().as_str())
                };
                let dur = t.duration.map(fmt_ms).unwrap_or_default();
                let body = inner_w.saturating_sub(2 + 7);
                let tw = if t.artist.is_empty() {
                    body
                } else {
                    body * 6 / 10
                };
                let aw = body.saturating_sub(tw + 1);
                let (tc, ac) = if is_cur {
                    (acc, acc)
                } else {
                    (rgb(app.theme.on_surface), rgb(app.theme.dim))
                };
                let mut spans = vec![
                    Span::styled(
                        if is_cur { "♫ " } else { "♪ " },
                        Style::default().fg(if is_cur { acc } else { rgb(app.theme.dim) }),
                    ),
                    Span::styled(
                        format!("{:<w$}", fit(&t.title, tw), w = tw),
                        Style::default().fg(tc).add_modifier(if is_cur {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                    ),
                ];
                if !t.artist.is_empty() {
                    spans.push(Span::styled(
                        format!(" {:<w$}", fit(&t.artist, aw), w = aw),
                        Style::default().fg(ac),
                    ));
                }
                spans.push(Span::styled(
                    format!(" {dur:>6}"),
                    Style::default().fg(rgb(app.theme.dim)),
                ));
                items.push(ListItem::new(Line::from(spans)));
            }
            empty_hint = match tab {
                Tab::Library => "\n  Library is empty.\n\n  /  search YouTube / SoundCloud\n  D  save a result to your music folder",
                Tab::Search => "\n  Nothing yet.\n\n  /  type a query and press Enter\n  P  switch YouTube / SoundCloud",
                _ => "\n  Queue is empty.\n\n  a  add the selected track\n  Enter  play a list from the selected track",
            };
        }
        Tab::Downloads => {
            for d in &app.downloads {
                let (icon, color, extra) = match &d.state {
                    Dl::Active => ("⬇", acc, String::new()),
                    Dl::Done => ("✓", acc, String::new()),
                    Dl::Failed(e) => ("✗", rgb(app.theme.error), format!("  {e}")),
                };
                let text = fit(&format!("{} {}{}", icon, d.label, extra), inner_w);
                items.push(ListItem::new(text).style(Style::default().fg(color)));
            }
            empty_hint = "\n  No downloads yet.\n\n  Press D on a search result.";
        }
        Tab::Playlists => {
            for p in &app.playlists {
                items.push(ListItem::new(format!(
                    "≡ {}",
                    fit(p, inner_w.saturating_sub(2))
                )));
            }
            empty_hint = "\n  No playlists.\n\n  w  save the current queue as a playlist";
        }
        Tab::Settings => {
            for (k, v) in app.settings_rows() {
                items.push(ListItem::new(vec![
                    Line::from(Span::styled(
                        fit(&k, inner_w),
                        Style::default().fg(rgb(app.theme.dim)),
                    )),
                    Line::from(Span::styled(
                        format!("  {}", fit(&v, inner_w.saturating_sub(2))),
                        Style::default().fg(acc),
                    )),
                ]));
            }
        }
    }

    if items.is_empty() {
        f.render_widget(
            Paragraph::new(empty_hint)
                .style(Style::default().fg(rgb(app.theme.dim)))
                .block(block),
            area,
        );
        return;
    }
    let mut state = ListState::default().with_selected(Some(app.sel[tab.idx()]));
    let list = List::new(items)
        .block(block)
        .highlight_symbol("▌")
        .highlight_style(
            Style::default()
                .bg(rgb(app.theme.surface_hi))
                .fg(acc)
                .add_modifier(Modifier::BOLD),
        );
    f.render_stateful_widget(list, area, &mut state);
    app.hits.borrow_mut().list_offset = state.offset();
}

// ---------- visualizer ----------

fn draw_viz(f: &mut Frame, area: Rect, app: &App, acc: Color) {
    let title = Line::from(vec![
        Span::styled(
            " Visualiser ",
            Style::default().fg(acc).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{} ", app.viz_mode.name()),
            Style::default().fg(rgb(app.theme.dim)),
        ),
    ]);
    let block = panel(title, false, acc, rgb(app.theme.outline));
    let inner = block.inner(area);
    f.render_widget(block, area);
    app.hits.borrow_mut().viz = inner;
    if app.wave_graphics() {
        return; // drawn as a kitty image after the frame
    }
    viz_body(
        f,
        inner,
        app.viz_mode,
        &app.viz,
        acc,
        rgb(app.theme.on_surface),
    );
}

fn viz_body(f: &mut Frame, inner: Rect, mode: VizMode, viz: &Visualizer, acc: Color, fg: Color) {
    if inner.width < 4 || inner.height < 2 {
        return;
    }
    let bright = lerp(acc, fg, 0.45);

    match mode {
        VizMode::Wave => {
            let dim = lerp(acc, Color::Black, 0.65);
            let (w, g) = (&viz.wave, &viz.ghost);
            let canvas = Canvas::default()
                .marker(Marker::Braille)
                .x_bounds([0.0, (WAVE - 1) as f64])
                .y_bounds([-1.0, 1.0])
                .paint(|ctx| {
                    for i in 0..WAVE - 1 {
                        ctx.draw(&CLine {
                            x1: i as f64,
                            y1: g[i] as f64,
                            x2: (i + 1) as f64,
                            y2: g[i + 1] as f64,
                            color: dim,
                        });
                    }
                    for i in 0..WAVE - 1 {
                        let c = lerp(acc, bright, (w[i].abs() + w[i + 1].abs()) / 1.2);
                        ctx.draw(&CLine {
                            x1: i as f64,
                            y1: w[i] as f64,
                            x2: (i + 1) as f64,
                            y2: w[i + 1] as f64,
                            color: c,
                        });
                    }
                });
            f.render_widget(canvas, inner);
        }
        VizMode::Bars => render_bars(
            f.buffer_mut(),
            inner,
            &viz.bars,
            lerp(acc, Color::Black, 0.35),
            bright,
        ),
        VizMode::Mirror => render_mirror(
            f.buffer_mut(),
            inner,
            &viz.bars,
            lerp(acc, Color::Black, 0.3),
            bright,
        ),
        VizMode::Spectrum => {
            let px = (inner.width as usize * 2).max(2);
            let samples: Vec<f32> = (0..px)
                .map(|x| {
                    let pos = x as f32 / (px - 1) as f32 * (BARS - 1) as f32;
                    let (i, fr) = (pos.floor() as usize, pos.fract());
                    let j = (i + 1).min(BARS - 1);
                    viz.bars[i] * (1.0 - fr) + viz.bars[j] * fr
                })
                .collect();
            let fill = lerp(acc, Color::Black, 0.6);
            let canvas = Canvas::default()
                .marker(Marker::Braille)
                .x_bounds([0.0, (px - 1) as f64])
                .y_bounds([0.0, 1.0])
                .paint(|ctx| {
                    for (x, v) in samples.iter().enumerate() {
                        ctx.draw(&CLine {
                            x1: x as f64,
                            y1: 0.0,
                            x2: x as f64,
                            y2: *v as f64,
                            color: fill,
                        });
                    }
                    for x in 0..px - 1 {
                        ctx.draw(&CLine {
                            x1: x as f64,
                            y1: samples[x] as f64,
                            x2: (x + 1) as f64,
                            y2: samples[x + 1] as f64,
                            color: bright,
                        });
                    }
                });
            f.render_widget(canvas, inner);
        }
    }
}

fn render_bars(buf: &mut Buffer, area: Rect, bars: &[f32], lo: Color, hi: Color) {
    const PART: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    let n = (area.width as usize + 1) / 2;
    for i in 0..n {
        let x = area.x + (i * 2) as u16;
        if x >= area.x + area.width {
            break;
        }
        let v = bars[i * bars.len() / n].clamp(0.0, 1.0);
        let eighths = (v * area.height as f32 * 8.0).round() as usize;
        let (full, rem) = (eighths / 8, eighths % 8);
        let color = lerp(lo, hi, v);
        for r in 0..full.min(area.height as usize) {
            buf.get_mut(x, area.y + area.height - 1 - r as u16)
                .set_symbol("█")
                .set_fg(color);
        }
        if rem > 0 && full < area.height as usize {
            buf.get_mut(x, area.y + area.height - 1 - full as u16)
                .set_symbol(PART[rem - 1])
                .set_fg(color);
        }
    }
}

fn render_mirror(buf: &mut Buffer, area: Rect, bars: &[f32], lo: Color, hi: Color) {
    let n = (area.width as usize + 1) / 2;
    let half = area.height / 2;
    let mid = area.y + half;
    for i in 0..n {
        let x = area.x + (i * 2) as u16;
        if x >= area.x + area.width {
            break;
        }
        let v = bars[i * bars.len() / n].clamp(0.0, 1.0);
        let e = ((v * half as f32).round() as u16).max(if v > 0.03 { 1 } else { 0 });
        let color = lerp(lo, hi, v);
        for r in 0..e.min(half) {
            buf.get_mut(x, mid - 1 - r).set_symbol("█").set_fg(color);
            if mid + r < area.y + area.height {
                buf.get_mut(x, mid + r).set_symbol("█").set_fg(color);
            }
        }
    }
}

// ---------- now playing ----------

fn render_cover(buf: &mut Buffer, area: Rect, seed: Option<u64>, dim: Color) {
    let (w, hpx) = (area.width as usize, area.height as usize * 2);
    let Some(seed) = seed else {
        for y in 0..area.height {
            for x in 0..area.width {
                let c = if (x / 2 + y) % 2 == 0 {
                    Color::Rgb(28, 38, 48)
                } else {
                    Color::Rgb(34, 46, 58)
                };
                buf.get_mut(area.x + x, area.y + y)
                    .set_symbol(" ")
                    .set_bg(c);
            }
        }
        buf.get_mut(area.x + area.width / 2, area.y + area.height / 2)
            .set_symbol("♪")
            .set_fg(dim);
        return;
    };
    let hue = (hash64(seed) % 1000) as f32 / 1000.0;
    let c1 = hsv(hue, 0.55, 0.95);
    let c2 = hsv(
        (hue + 0.18 + (hash64(seed ^ 7) % 100) as f32 / 500.0) % 1.0,
        0.7,
        0.35,
    );
    let grid: Vec<f32> = (0..25)
        .map(|i| (hash64(seed.wrapping_add(i as u64 * 31)) % 1000) as f32 / 1000.0)
        .collect();
    let px = |x: usize, y: usize| -> Color {
        let (qx, qy) = ((x / 2) * 2, y); // chunky 2x1-cell pixels
        let u = qx as f32 / (w.max(2) - 1) as f32 * 4.0;
        let v = qy as f32 / (hpx.max(2) - 1) as f32 * 4.0;
        let (i, j) = (u.floor().min(3.0) as usize, v.floor().min(3.0) as usize);
        let (fu, fv) = (u - i as f32, v - j as f32);
        let g = |a: usize, b: usize| grid[b * 5 + a];
        let t = g(i, j) * (1.0 - fu) * (1.0 - fv)
            + g(i + 1, j) * fu * (1.0 - fv)
            + g(i, j + 1) * (1.0 - fu) * fv
            + g(i + 1, j + 1) * fu * fv;
        let n = (hash64(seed ^ ((qx as u64) << 16) ^ qy as u64) % 100) as f32 / 100.0 * 0.12;
        lerp(c1, c2, t + n)
    };
    for cy in 0..area.height as usize {
        for cx in 0..w {
            buf.get_mut(area.x + cx as u16, area.y + cy as u16)
                .set_symbol("▀")
                .set_fg(px(cx, cy * 2))
                .set_bg(px(cx, cy * 2 + 1));
        }
    }
}

fn draw_now_playing(f: &mut Frame, area: Rect, app: &App, acc: Color) {
    let origin = match app.current.as_ref().map(|t| &t.source) {
        Some(Source::Local(_)) => "local",
        Some(Source::Remote { .. }) => "online",
        None => "idle",
    };
    let title = Line::from(vec![
        Span::styled(
            " Now Playing ",
            Style::default().fg(acc).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{origin} "),
            Style::default().fg(rgb(app.theme.dim)),
        ),
    ]);
    let block = panel(title, false, acc, rgb(app.theme.outline));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width < 24 || inner.height < 5 {
        return;
    }

    // Square cover in whole cells, corrected for the real cell aspect ratio (cells are ~1:2).
    let (cw, ch) = crate::kittyviz::cell_px();
    let (cw, ch) = (cw.max(1), ch.max(1));
    // Tall card: square cover on top, track info and controls underneath.
    let side_rows = inner.height.saturating_sub(7).min((inner.width as u32 * cw / ch) as u16);
    let stacked = inner.height >= 16 && inner.width >= 30 && side_rows >= 6;
    let (cover_box, text_area, fill) = if stacked {
        let cover_cols = ((side_rows as u32 * ch) / cw) as u16;
        let cover_box = Rect {
            x: inner.x + (inner.width - cover_cols) / 2,
            y: inner.y,
            width: cover_cols,
            height: side_rows,
        };
        let text_area = Rect {
            x: inner.x,
            y: inner.y + side_rows + 1,
            width: inner.width,
            height: inner.height - side_rows - 1,
        };
        (cover_box, text_area, Rect { x: inner.x, y: inner.y, width: inner.width, height: side_rows })
    } else {
        // Short card: cover in a left column, text on the right.
        let max_cols = (inner.width / 3).max(4);
        let rows_for_width = (max_cols as u32 * cw / ch) as u16;
        let (cover_cols, cover_rows) = if rows_for_width <= inner.height {
            (max_cols, rows_for_width)
        } else {
            (((inner.height as u32 * ch) / cw) as u16, inner.height)
        };
        let cols = Layout::horizontal([
            Constraint::Length(cover_cols),
            Constraint::Length(2),
            Constraint::Min(10),
        ])
        .split(inner);
        let cover_box = Rect {
            x: cols[0].x,
            y: inner.y + (inner.height - cover_rows) / 2,
            width: cover_cols,
            height: cover_rows,
        };
        (cover_box, cols[2], cols[0])
    };
    let seed = app.current.as_ref().map(|t| {
        t.key().bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
        })
    });
    app.hits.borrow_mut().cover = cover_box;
    if app.cover_graphics() && app.cover.is_some() {
        // The real cover is drawn as a kitty image right after this frame; keep the cells blank.
        for y in fill.y..fill.y + fill.height {
            for x in fill.x..fill.x + fill.width {
                f.buffer_mut()
                    .get_mut(x, y)
                    .set_symbol(" ")
                    .set_bg(rgb(app.theme.surface));
            }
        }
    } else {
        render_cover(f.buffer_mut(), cover_box, seed, rgb(app.theme.dim));
    }

    let r = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(text_area);

    let w = text_area.width as usize;
    let (title, artist) = match &app.current {
        Some(t) => (
            t.title.clone(),
            if t.artist.is_empty() {
                "unknown artist".to_string()
            } else {
                t.artist.clone()
            },
        ),
        None => ("nothing playing".into(), "press Enter on a track".into()),
    };
    let glyph = if app.current.is_none() {
        "  "
    } else if app.player.is_paused() {
        "⏸ "
    } else {
        "▶ "
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(glyph, Style::default().fg(acc)),
            Span::styled(
                fit(&title, w.saturating_sub(2)),
                Style::default()
                    .fg(rgb(app.theme.on_surface))
                    .add_modifier(Modifier::BOLD),
            ),
        ])),
        r[0],
    );
    f.render_widget(
        Paragraph::new(Span::styled(
            fit(&artist, w),
            Style::default().fg(rgb(app.theme.dim)),
        )),
        r[1],
    );

    // progress line with knob
    let pos = app.player.position().as_secs();
    let dur = app
        .player
        .duration
        .map(|d| d.as_secs())
        .or(app.current.as_ref().and_then(|t| t.duration));
    let ratio = dur
        .filter(|d| *d > 0)
        .map(|d| (pos as f64 / d as f64).clamp(0.0, 1.0))
        .unwrap_or(0.0);
    let filled = ((w.saturating_sub(1)) as f64 * ratio).round() as usize;
    let bar = Line::from(vec![
        Span::styled("━".repeat(filled), Style::default().fg(acc)),
        Span::styled("●", Style::default().fg(rgb(app.theme.on_surface))),
        Span::styled(
            "─".repeat(w.saturating_sub(filled + 1)),
            Style::default().fg(rgb(app.theme.outline)),
        ),
    ]);
    f.render_widget(Paragraph::new(bar), r[3]);
    app.hits.borrow_mut().progress = r[3];

    // times
    let tcols = Layout::horizontal([Constraint::Min(0), Constraint::Length(9)]).split(r[4]);
    f.render_widget(
        Paragraph::new(Span::styled(
            format!(
                "{} / {}",
                fmt_ms(pos),
                dur.map(fmt_ms).unwrap_or_else(|| "--:--".into())
            ),
            Style::default().fg(rgb(app.theme.secondary)),
        )),
        tcols[0],
    );
    if let Some(d) = dur {
        f.render_widget(
            Paragraph::new(Span::styled(
                format!("-{}", fmt_ms(d.saturating_sub(pos))),
                Style::default().fg(rgb(app.theme.dim)),
            ))
            .alignment(Alignment::Right),
            tcols[1],
        );
    }

    // controls + volume
    let ccols = Layout::horizontal([Constraint::Min(0), Constraint::Length(22)]).split(r[5]);
    let on = |b: bool| Style::default().fg(if b { acc } else { rgb(app.theme.outline) });
    let play_glyph = if app.player.is_playing() {
        "▮▮"
    } else {
        "▶ "
    };
    let repeat = match app.queue.repeat {
        crate::queue::Repeat::Off => "⟳",
        crate::queue::Repeat::All => "⟳",
        crate::queue::Repeat::One => "⟳¹",
    };
    let mut spans = vec![
        Span::styled("|◀  ", Style::default().fg(rgb(app.theme.on_surface))),
        Span::styled(
            play_glyph,
            Style::default().fg(acc).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ▶|   ", Style::default().fg(rgb(app.theme.on_surface))),
        Span::styled("⇄ ", on(app.queue.shuffle)),
        Span::styled(repeat, on(app.queue.repeat != crate::queue::Repeat::Off)),
    ];
    if !app.player.info.is_empty() {
        spans.push(Span::styled(
            format!("   {}", app.player.info),
            Style::default().fg(rgb(app.theme.dim)),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), ccols[0]);

    let vol = (app.player.volume / 1.5).clamp(0.0, 1.0);
    let vfilled = (10.0 * vol).round() as usize;
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("vol ", Style::default().fg(rgb(app.theme.dim))),
            Span::styled("━".repeat(vfilled), Style::default().fg(acc)),
            Span::styled(
                "─".repeat(10 - vfilled),
                Style::default().fg(rgb(app.theme.outline)),
            ),
            Span::styled(
                format!(" {:>3}%", (app.player.volume * 100.0).round()),
                Style::default().fg(rgb(app.theme.dim)),
            ),
        ]))
        .alignment(Alignment::Right),
        ccols[1],
    );
}

// ---------- footer / help ----------

fn draw_footer(f: &mut Frame, area: Rect, app: &App, acc: Color) {
    if let Mode::Input(kind) = app.mode {
        let label = match kind {
            InputKind::Search => format!(" search {}: ", app.provider.name()),
            InputKind::PlaylistName => " save queue as playlist: ".to_string(),
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(label, Style::default().fg(Color::Black).bg(acc)),
                Span::raw(format!(" {}█", app.input)),
            ])),
            area,
        );
        return;
    }
    let hints: &[(&str, &str)] = match app.tab {
        Tab::Library => &[
            ("enter", "play"),
            ("a", "queue"),
            ("/", "search"),
            ("j/k", "move"),
            ("s", "shuffle"),
            ("v", "viz"),
            ("?", "help"),
        ],
        Tab::Search => &[
            ("enter", "play"),
            ("a", "queue"),
            ("D", "save"),
            ("/", "search"),
            ("P", "source"),
            ("v", "viz"),
            ("?", "help"),
        ],
        Tab::Queue => &[
            ("enter", "jump"),
            ("d", "remove"),
            ("w", "save playlist"),
            ("s", "shuffle"),
            ("r", "repeat"),
            ("?", "help"),
        ],
        Tab::Downloads => &[
            ("D", "download from search"),
            ("1-6", "tabs"),
            ("?", "help"),
        ],
        Tab::Playlists => &[
            ("enter", "load"),
            ("d", "delete"),
            ("w", "save queue"),
            ("?", "help"),
        ],
        Tab::Settings => &[
            ("enter", "change"),
            ("v", "viz"),
            ("P", "source"),
            ("?", "help"),
        ],
    };
    let mut spans = vec![Span::raw(" ")];
    for (k, d) in hints {
        spans.push(Span::styled(
            *k,
            Style::default().fg(acc).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {d}   "),
            Style::default().fg(rgb(app.theme.dim)),
        ));
    }
    let status = fit(&app.status, (area.width / 2) as usize);
    let sw = status.chars().count() as u16 + 1;
    let cols = Layout::horizontal([Constraint::Min(0), Constraint::Length(sw)]).split(area);
    f.render_widget(Paragraph::new(Line::from(spans)), cols[0]);
    f.render_widget(
        Paragraph::new(Span::styled(
            status,
            Style::default().fg(rgb(app.theme.dim)),
        ))
        .alignment(Alignment::Right),
        cols[1],
    );
}

fn draw_help(f: &mut Frame, area: Rect, acc: Color, theme: &crate::theme::Theme) {
    let (w, h) = (64.min(area.width), 23.min(area.height));
    let r = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    let rows = [
        ("space", "pause / resume"),
        ("n / p", "next / previous (p restarts after 3s)"),
        ("← →", "seek -5s / +5s"),
        ("+ -", "volume"),
        ("s / r", "shuffle / repeat (off → all → one)"),
        ("v", "switch visualiser: wave · bars · mirror · spectrum"),
        ("[ / ]", "wave stroke thinner / thicker"),
        (
            "1-6, tab",
            "Library · Queue · Search · Downloads · Playlists · Settings",
        ),
        ("j k g G", "move selection"),
        ("enter", "play (list becomes the queue) / jump / load"),
        ("a", "add to queue"),
        ("d", "remove from queue / delete playlist"),
        ("/", "search online"),
        ("P", "switch YouTube / SoundCloud"),
        ("D", "download selected result to music folder"),
        ("w", "save queue as a playlist"),
        ("T", "switch layout: resonance · minimal · focus"),
        ("q", "quit"),
    ];
    let mut lines: Vec<Line> = rows
        .iter()
        .map(|(k, d)| {
            Line::from(vec![
                Span::styled(
                    format!(" {k:<9}"),
                    Style::default().fg(acc).add_modifier(Modifier::BOLD),
                ),
                Span::styled(*d, Style::default().fg(rgb(theme.secondary))),
            ])
        })
        .collect();
    lines.push(Line::from(""));
    lines.push(Line::styled(
        " press any key to close",
        Style::default().fg(rgb(theme.dim)),
    ));
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines).block(
            panel(
                Line::styled(
                    " Help ",
                    Style::default().fg(acc).add_modifier(Modifier::BOLD),
                ),
                true,
                acc,
                rgb(theme.outline),
            )
            .style(Style::default().bg(Color::Rgb(14, 20, 28))),
        ),
        r,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn all_visualisers_and_cover_render_without_panic() {
        let mut viz = Visualizer::new();
        for i in 0..BARS {
            viz.bars[i] = (i as f32 / BARS as f32 * 6.0).sin().abs();
        }
        for i in 0..WAVE {
            viz.wave[i] = (i as f32 / 10.0).sin() * 0.8;
            viz.ghost[i] = (i as f32 / 10.0 + 0.4).sin() * 0.8;
        }
        for (w, h) in [(100, 20), (7, 3), (60, 6)] {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            for mode in [
                VizMode::Wave,
                VizMode::Bars,
                VizMode::Mirror,
                VizMode::Spectrum,
            ] {
                term.draw(|f| {
                    viz_body(
                        f,
                        f.size(),
                        mode,
                        &viz,
                        Color::Rgb(127, 214, 190),
                        Color::White,
                    )
                })
                .unwrap();
            }
            term.draw(|f| {
                let r = f.size();
                render_cover(f.buffer_mut(), r, Some(42), Color::Gray);
                render_cover(f.buffer_mut(), r, None, Color::Gray);
            })
            .unwrap();
        }
    }
}

/// Two-row now-playing strip for the minimal preset.
fn draw_now_compact(f: &mut Frame, area: Rect, app: &App, acc: Color) {
    let title = Line::styled(" now playing ", Style::default().fg(acc).add_modifier(Modifier::BOLD));
    let block = panel(title, false, acc, rgb(app.theme.outline));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width < 10 || inner.height < 2 {
        return;
    }
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(inner);

    let (title, artist) = match &app.current {
        Some(t) => (t.title.clone(), t.artist.clone()),
        None => ("nothing playing".to_string(), String::new()),
    };
    let glyph = if app.current.is_none() {
        "  "
    } else if app.player.is_paused() {
        "⏸ "
    } else {
        "▶ "
    };
    let pos = app.player.position().as_secs();
    let dur = app
        .player
        .duration
        .map(|d| d.as_secs())
        .or(app.current.as_ref().and_then(|t| t.duration));
    let time = format!(
        "{} / {}",
        fmt_ms(pos),
        dur.map(fmt_ms).unwrap_or_else(|| "--:--".into())
    );
    let time_w = time.chars().count() as u16 + 1;
    let head_cols = Layout::horizontal([Constraint::Min(0), Constraint::Length(time_w)]).split(rows[0]);
    let tw = head_cols[0].width as usize;
    let head = if artist.is_empty() {
        format!("{glyph}{title}")
    } else {
        format!("{glyph}{title} · {artist}")
    };
    f.render_widget(
        Paragraph::new(Span::styled(
            fit(&head, tw),
            Style::default()
                .fg(rgb(app.theme.on_surface))
                .add_modifier(Modifier::BOLD),
        )),
        head_cols[0],
    );
    f.render_widget(
        Paragraph::new(Span::styled(time, Style::default().fg(rgb(app.theme.dim))))
            .alignment(Alignment::Right),
        head_cols[1],
    );

    let w = rows[1].width as usize;
    let ratio = dur
        .filter(|d| *d > 0)
        .map(|d| (pos as f64 / d as f64).clamp(0.0, 1.0))
        .unwrap_or(0.0);
    let filled = ((w.saturating_sub(1)) as f64 * ratio).round() as usize;
    let bar = Line::from(vec![
        Span::styled("━".repeat(filled), Style::default().fg(acc)),
        Span::styled("●", Style::default().fg(rgb(app.theme.on_surface))),
        Span::styled(
            "─".repeat(w.saturating_sub(filled + 1)),
            Style::default().fg(rgb(app.theme.outline)),
        ),
    ]);
    f.render_widget(Paragraph::new(bar), rows[1]);
    app.hits.borrow_mut().progress = rows[1];
}

/// Synced lyrics panel: the current line is highlighted and centred, neighbours fade out.
fn draw_lyrics(f: &mut Frame, area: Rect, app: &App, acc: Color) {
    let title = Line::styled(" lyrics ", Style::default().fg(acc).add_modifier(Modifier::BOLD));
    let block = panel(title, false, acc, rgb(app.theme.outline));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height < 1 || inner.width < 4 {
        return;
    }
    let dim = rgb(app.theme.dim);
    let text_c = rgb(app.theme.on_surface);
    if app.lyrics.is_empty() {
        let msg = if app.lyrics_loading {
            "loading lyrics…"
        } else {
            "no lyrics for this track"
        };
        f.render_widget(
            Paragraph::new(Span::styled(msg, Style::default().fg(dim))).alignment(Alignment::Center),
            inner,
        );
        return;
    }

    let h = inner.height as usize;
    let w = inner.width as usize;
    let pos = app.player.position().as_secs_f64();
    let synced = crate::lyrics::is_synced(&app.lyrics);
    let cur = if synced {
        crate::lyrics::current_index(&app.lyrics, pos)
    } else {
        None
    };
    let center = (h / 2) as i64;

    let mut rows: Vec<Line> = Vec::with_capacity(h);
    for r in 0..h {
        let (idx, style) = match cur {
            Some(c) => {
                let dist = (r as i64 - center).abs();
                let style = match dist {
                    0 => Style::default().fg(acc).add_modifier(Modifier::BOLD),
                    1 => Style::default().fg(text_c),
                    _ => Style::default().fg(dim),
                };
                (c as i64 + r as i64 - center, style)
            }
            // unsynced: plain text from the top
            None => (r as i64, Style::default().fg(text_c)),
        };
        if idx < 0 || idx as usize >= app.lyrics.len() {
            rows.push(Line::from(""));
            continue;
        }
        let text = fit(&app.lyrics[idx as usize].1, w);
        rows.push(Line::from(Span::styled(text, style)));
    }
    f.render_widget(Paragraph::new(rows).alignment(Alignment::Center), inner);
}
