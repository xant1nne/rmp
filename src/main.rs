mod app;
mod config;
mod covers;
mod discord;
mod kittyviz;
mod layouts;
mod lyrics;
mod library;
mod matugen;
mod player;
mod playlist;
mod queue;
mod theme;
mod track;
mod ui;
mod visualizer;
mod ytdlp;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::{io::stdout, panic, time::Duration};

fn restore() {
    let _ = kittyviz::clear_all(&mut stdout());
    let _ = disable_raw_mode();
    let _ = execute!(stdout(), DisableMouseCapture, LeaveAlternateScreen);
}

fn help() -> &'static str {
    "\
rmp — TUI music player

USAGE:
    rmp [OPTION]

OPTIONS:
    -h, --help            print this help and exit
    -V, --version         print the version and exit
        --print-config    print the path and contents of config.json
        --rpc-test
                          connect to Discord, show a test activity for 8s, exit
        --dump-library [DIR]
                          print the scanned library (tags, duration, cover) and exit
        --embed-cover <AUDIO>
                          normalise the thumbnail next to AUDIO to <stem>.png and
                          embed it into the audio tags (downloads do this automatically)
    -m, --setup-matugen <image>
                          install the matugen template, wire it into
                          ~/.config/matugen/config.toml and generate
                          ~/.config/rmp/colors.json from <image>
        --preview-wave [SCALE]
                          render the kitty wave preview PNGs into the temp dir
        --rpc-test     connect to Discord, show a test activity for 8s, exit

KEYS:
    space pause  n/p next/prev  ←/→ seek  +/- volume  s/r shuffle/repeat
    v viz  [ ] wave thickness  P source  / search  a queue  d remove
    D download  w playlist  1-6/tab tabs  ? help  q quit
"
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("-h" | "--help") => {
            print!("{}", help());
            return Ok(());
        }
        Some("-V" | "--version") => {
            println!("rmp {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some("--print-config") => {
            let cfg = config::load(); // also normalises/creates the file
            println!("{}", config::path().display());
            println!("{}", serde_json::to_string_pretty(&cfg).unwrap_or_default());
            return Ok(());
        }
        Some("-m" | "--setup-matugen") => {
            let Some(image) = args.get(1) else {
                eprintln!("usage: rmp --setup-matugen <wallpaper image>");
                std::process::exit(2);
            };
            let colors = matugen::setup(std::path::Path::new(image))?;
            println!(
                "matugen template installed; palette written to {}",
                colors.display()
            );
            return Ok(());
        }
        Some("--embed-cover") => {
            let Some(audio) = args.get(1).map(std::path::PathBuf::from) else {
                eprintln!("usage: rmp --embed-cover <audio file>");
                std::process::exit(2);
            };
            match covers::attach_downloaded(&audio) {
                Some(png) => {
                    println!(
                        "cover written to {} and embedded into the tags",
                        png.display()
                    )
                }
                None => {
                    eprintln!(
                        "no thumbnail found next to {} (expected <stem>.jpg/png/webp)",
                        audio.display()
                    );
                    std::process::exit(1);
                }
            }
            return Ok(());
        }
        Some("--dump-library") => {
            let dir = args
                .get(1)
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| config::load().music_dir);
            let tracks = library::scan(&dir);
            println!("{} tracks in {}", tracks.len(), dir.display());
            for t in &tracks {
                let cover = covers::embedded(t).is_some() || covers::folder_cover(t).is_some();
                println!(
                    "{:>4}s  cover:{:<5} thumb:{:<5}  {:<20}  album:{:<15}  {}",
                    t.duration.unwrap_or(0),
                    cover,
                    t.thumb.is_some(),
                    t.artist,
                    t.album.clone().unwrap_or_default(),
                    t.title
                );
            }
            return Ok(());
        }
        Some("--rpc-test") => {
            let cfg = config::load();
            let mut rpc = discord::Rpc::new(&cfg);
            let track = track::Track {
                title: "Test track".into(),
                artist: "Test artist".into(),
                duration: Some(210),
                album: Some("Test album".into()),
                thumb: Some("https://i.ytimg.com/vi/TfkBoEez7Ig/hqdefault.jpg".into()),
                source: track::Source::Local(std::path::PathBuf::from("/test")),
            };
            rpc.update(&track, std::time::Duration::from_secs(42), false);
            if rpc.connected() {
                match rpc.last_error() {
                    Some(e) => println!("connected, but Discord rejected the activity: {e}"),
                    None => println!(
                        "connected to Discord (app {}) — the card should say \"Test track\" for 8s",
                        cfg.discord_app_id
                    ),
                }
                std::thread::sleep(std::time::Duration::from_secs(8));
            } else {
                eprintln!(
                    "could not connect: {}",
                    rpc.last_error()
                        .unwrap_or("Discord is not running / IPC socket unavailable")
                );
            }
            rpc.clear();
            return Ok(());
        }
        Some("--preview-wave") => {
            let scale = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1.0);
            return preview_wave(scale);
        }
        Some(_) if args[0].starts_with('-') => {
            eprintln!("rmp: unknown option `{}`\n\n{}", args[0], help());
            std::process::exit(2);
        }
        _ => {}
    }

    let cfg = config::load();
    let crash_log = cfg.cache_dir.join("crash.log");
    let mut app = app::App::new(cfg)?; // errors (e.g. no audio device) print normally

    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    let hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        restore();
        // Keep the panic where it can be read after the alternate screen is gone.
        if let Some(dir) = crash_log.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let msg = format!(
            "{} rmp {}\n{}\nthread: {:?}\n\n{}\n\n",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            env!("CARGO_PKG_VERSION"),
            info,
            std::thread::current().name(),
            std::backtrace::Backtrace::force_capture()
        );
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&crash_log)
        {
            let _ = f.write_all(msg.as_bytes());
        }
        hook(info);
    }));

    let mut term = Terminal::new(CrosstermBackend::new(stdout()))?;
    let res = run(&mut term, &mut app);
    restore();
    app.shutdown();
    res
}

/// Render the wave at several simulated cell heights so the stroke weight can be
/// judged without launching the TUI (handy on HiDPI screens).
fn preview_wave(scale: f32) -> Result<()> {
    use image::{Rgba, RgbaImage};

    let wave: Vec<f32> = (0..512)
        .map(|i| {
            let t = i as f32;
            ((t / 17.0).sin() * 0.55 + (t / 6.1).sin() * 0.22).clamp(-1.0, 1.0)
        })
        .collect();
    let ghost: Vec<f32> = wave.iter().map(|v| v * 0.85).collect();

    let (w, h) = (1000u32, 220u32);
    let dir = std::env::temp_dir();
    for cell_h in [16u32, 24, 32, 44] {
        let style = kittyviz::Style::for_cell(cell_h, scale);
        let rgba = kittyviz::render_wave_styled(w, h, &wave, &ghost, [127, 214, 190], style);
        let mut img = RgbaImage::from_pixel(w, h, Rgba([14, 20, 28, 255]));
        for (i, px) in rgba.chunks_exact(4).enumerate() {
            let a = px[3] as f32 / 255.0;
            if a <= 0.0 {
                continue;
            }
            let base = img.get_pixel(i as u32 % w, i as u32 / w).0;
            let over = |b: u8, f: u8| (f as f32 * a + b as f32 * (1.0 - a)) as u8;
            img.put_pixel(
                i as u32 % w,
                i as u32 / w,
                Rgba([
                    over(base[0], px[0]),
                    over(base[1], px[1]),
                    over(base[2], px[2]),
                    255,
                ]),
            );
        }
        let path = dir.join(format!("rmp-wave-cell{}px.png", cell_h));
        img.save(&path)?;
        println!(
            "{}  (half_width {:.1}px, glow {:.1}px)",
            path.display(),
            style.half_width,
            style.glow
        );
    }

    // Same rendering path as the player, fed with a simulated 110 Hz + 440 Hz tone.
    let buf: crate::player::SampleBuf =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));
    let mut viz = crate::visualizer::Visualizer::new();
    {
        let mut b = buf.lock().unwrap();
        for i in 0..4096 {
            let t = i as f32 / 44100.0;
            b.push_back(
                (std::f32::consts::TAU * 110.0 * t).sin() * 0.45
                    + (std::f32::consts::TAU * 440.0 * t).sin() * 0.2,
            );
        }
    }
    for _ in 0..20 {
        viz.update(&buf, true);
    }
    let style = kittyviz::Style::for_cell(32, scale);
    let rgba = kittyviz::render_wave_styled(w, h, &viz.wave, &viz.ghost, [127, 214, 190], style);
    let mut img = RgbaImage::from_pixel(w, h, Rgba([14, 20, 28, 255]));
    for (i, px) in rgba.chunks_exact(4).enumerate() {
        let a = px[3] as f32 / 255.0;
        let base = img.get_pixel(i as u32 % w, i as u32 / w).0;
        let over = |b: u8, f: u8| (f as f32 * a + b as f32 * (1.0 - a)) as u8;
        img.put_pixel(
            i as u32 % w,
            i as u32 / w,
            Rgba([
                over(base[0], px[0]),
                over(base[1], px[1]),
                over(base[2], px[2]),
                255,
            ]),
        );
    }
    let live = dir.join("rmp-wave-live.png");
    img.save(&live)?;
    println!("{}  (simulated player signal)", live.display());
    Ok(())
}

fn run(term: &mut Terminal<CrosstermBackend<std::io::Stdout>>, app: &mut app::App) -> Result<()> {
    while !app.quit {
        term.draw(|f| ui::draw(f, app))?;
        app.draw_graphics(&mut stdout());
        app.draw_cover(&mut stdout());
        if event::poll(Duration::from_millis(33))? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => app.on_key(k),
                Event::Mouse(m) => app.on_mouse(m),
                Event::Resize(..) => app.force_redraw(),
                _ => {}
            }
        }
        app.tick();
    }
    Ok(())
}
