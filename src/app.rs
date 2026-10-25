use crate::kittyviz;
use crate::layouts::Preset;
use crate::{
    config::Config,
    covers,
    discord::Rpc,
    library,
    player::Player,
    playlist,
    queue::Queue,
    track::{Source, Track},
    visualizer::{Visualizer, VizMode},
    ytdlp::{self, Provider},
};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::{
    cell::RefCell,
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::mpsc::{channel, Receiver, Sender},
    thread,
    time::{Duration, Instant, SystemTime},
};

#[derive(Clone, Copy, PartialEq)]
pub enum Tab {
    Library,
    Queue,
    Search,
    Downloads,
    Playlists,
    Settings,
}

impl Tab {
    pub const ALL: [Tab; 6] = [
        Tab::Library,
        Tab::Queue,
        Tab::Search,
        Tab::Downloads,
        Tab::Playlists,
        Tab::Settings,
    ];
    pub fn idx(self) -> usize {
        self as usize
    }
    pub fn title(self) -> &'static str {
        match self {
            Tab::Library => "Library",
            Tab::Queue => "Queue",
            Tab::Search => "Search",
            Tab::Downloads => "Downloads",
            Tab::Playlists => "Playlists",
            Tab::Settings => "Settings",
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum InputKind {
    Search,
    PlaylistName,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Normal,
    Input(InputKind),
    Help,
}

pub enum Dl {
    Active,
    Done,
    Failed(String),
}

pub struct Download {
    pub label: String,
    pub state: Dl,
}

/// Messages from background threads (yt-dlp work).
pub enum Msg {
    Results(Vec<Track>),
    Ready(Track, PathBuf),
    DlDone(usize),
    DlFail(usize, String),
    /// A cached/streamed track could not be fetched; `id` is the remote id.
    FetchFail(String, String),
    /// Cover art for the track with this key finished downloading.
    CoverReady(String, PathBuf),
    /// Lyrics for the track with this key.
    Lyrics(String, Vec<(f64, String)>),
    Error(String),
}

pub struct App {
    pub cfg: Config,
    pub tab: Tab,
    pub mode: Mode,
    pub input: String,
    pub library: Vec<Track>,
    pub results: Vec<Track>,
    pub queue: Queue,
    pub player: Player,
    pub viz: Visualizer,
    pub viz_mode: VizMode,
    pub preset: Preset,
    pub lyrics: Vec<(f64, String)>,
    pub lyrics_loading: bool,
    pub theme: crate::theme::Theme,
    pub graphics: bool,
    /// Kitty image id currently showing the wave (0 = none). Two ids are used
    /// alternately so the new frame is drawn before the old one is deleted.
    wave_id: u32,
    /// Signature of the last kitty wave frame, to skip identical redraws.
    last_graphics: Option<u64>,
    frame_buf: Vec<u8>,
    pub hits: RefCell<Hits>,
    pub last_click: Option<(Tab, usize, Instant)>,
    /// Cover art PNG for the current track (kitty graphics).
    pub cover: Option<PathBuf>,
    cover_key: Option<String>,
    cover_shown: bool,
    cover_sig: Option<(PathBuf, u16, u16, u16, u16)>,
    colors_mtime: Option<SystemTime>,
    last_check: Instant,
    pub provider: Provider,
    pub downloads: Vec<Download>,
    pub playlists: Vec<String>,
    pub sel: [usize; 6],
    pub status: String,
    pub current: Option<Track>,
    pub quit: bool,
    rpc: Rpc,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    pending: Option<String>,
    /// Config needs to be written back to disk (debounced in `tick`).
    dirty: bool,
    last_save: Instant,
    /// Last time we refreshed Rich Presence; a periodic refresh also revives the
    /// activity after Discord restarts.
    last_rpc: Instant,
}

impl App {
    pub fn new(cfg: Config) -> Result<Self> {
        let (tx, rx) = channel();
        let library = library::scan(&cfg.music_dir);
        let status = format!("{} tracks in {}", library.len(), cfg.music_dir.display());
        let provider = Provider::from_name(&cfg.provider);
        let colors_mtime = std::fs::metadata(crate::matugen::colors_path())
            .and_then(|m| m.modified())
            .ok();
        Ok(Self {
            player: Player::new(cfg.volume)?,
            rpc: Rpc::new(&cfg),
            viz_mode: VizMode::from_name(&cfg.viz),
            preset: Preset::from_name(&cfg.layout),
            lyrics: Vec::new(),
            lyrics_loading: false,
            theme: crate::theme::Theme::load(&cfg),
            graphics: kittyviz::supported(&cfg.graphics),
            wave_id: 0,
            last_graphics: None,
            frame_buf: Vec::new(),
            hits: RefCell::new(Hits::default()),
            last_click: None,
            cover: None,
            cover_key: None,
            cover_shown: false,
            cover_sig: None,
            colors_mtime,
            last_check: Instant::now() - Duration::from_secs(10),
            cfg,
            tab: Tab::Library,
            mode: Mode::Normal,
            input: String::new(),
            library,
            results: vec![],
            queue: Queue::new(),
            viz: Visualizer::new(),
            provider,
            downloads: vec![],
            playlists: playlist::list(),
            sel: [0; 6],
            status,
            current: None,
            quit: false,
            tx,
            rx,
            pending: None,
            dirty: false,
            last_save: Instant::now(),
            last_rpc: Instant::now(),
        })
    }

    pub fn shutdown(&mut self) {
        self.rpc.clear();
        self.persist();
    }

    /// Copy volatile state into the config and write it, if there is anything to write.
    pub fn persist(&mut self) {
        if !self.dirty {
            return;
        }
        self.cfg.volume = self.player.volume;
        self.cfg.viz = self.viz_mode.name().to_string();
        self.cfg.provider = self.provider.key().to_string();
        // `graphics` is auto-detected at startup, so never overwrite the user's "auto"/"off".
        self.dirty = false;
        self.last_save = Instant::now();
        if let Err(e) = crate::config::save(&self.cfg) {
            self.status = format!("✗ config: {e}");
        }
    }

    fn touch_config(&mut self) {
        self.dirty = true;
    }

    pub fn rpc_connected(&self) -> bool {
        self.rpc.connected()
    }

    /// Force the kitty image to be redrawn (terminal resize, config change).
    pub fn force_redraw(&mut self) {
        self.cover_shown = false;
        self.cover_sig = None;
        self.last_graphics = None;
    }

    // ---------- lists ----------

    /// Track lists (Library / Search / Queue); other tabs return nothing.
    pub fn items(&self, tab: Tab) -> Vec<&Track> {
        match tab {
            Tab::Library => self.library.iter().collect(),
            Tab::Search => self.results.iter().collect(),
            Tab::Queue => self.queue.ordered(),
            _ => vec![],
        }
    }

    pub fn settings_rows(&self) -> Vec<(String, String)> {
        vec![
            (
                "Source (enter to switch)".into(),
                self.provider.name().into(),
            ),
            ("Visualiser (enter / v)".into(), self.viz_mode.name().into()),
            ("Layout (enter / T)".into(), self.preset.name().into()),
            (
                "Theme".into(),
                format!("{} · {}", self.cfg.theme, self.cfg.wave_scale),
            ),
            (
                "Music folder".into(),
                self.cfg.music_dir.display().to_string(),
            ),
            (
                "Cache folder".into(),
                self.cfg.cache_dir.display().to_string(),
            ),
            ("yt-dlp".into(), self.cfg.ytdlp.clone()),
            (
                "Discord RPC".into(),
                if self.cfg.discord_app_id.is_empty() {
                    "off (set discord_app_id)".into()
                } else if self.rpc.connected() {
                    format!("connected · {}", self.cfg.discord_app_id)
                } else if let Some(e) = self.rpc.last_error() {
                    format!("not connected · {e}")
                } else {
                    "waiting for Discord".into()
                },
            ),
        ]
    }

    fn len(&self, tab: Tab) -> usize {
        match tab {
            Tab::Library => self.library.len(),
            Tab::Search => self.results.len(),
            Tab::Queue => self.queue.len(),
            Tab::Downloads => self.downloads.len(),
            Tab::Playlists => self.playlists.len(),
            Tab::Settings => self.settings_rows().len(),
        }
    }

    fn selected_track(&self) -> Option<Track> {
        self.items(self.tab)
            .get(self.sel[self.tab.idx()])
            .map(|t| (*t).clone())
    }

    fn move_sel(&mut self, delta: i64) {
        let len = self.len(self.tab) as i64;
        let i = self.tab.idx();
        self.sel[i] = if len == 0 {
            0
        } else {
            (self.sel[i] as i64 + delta).clamp(0, len - 1) as usize
        };
    }

    // ---------- playback ----------

    fn play_track(&mut self, t: Track) {
        match t.source.clone() {
            Source::Local(p) => self.start(t, p),
            Source::Remote { url, id } => {
                let stem = ytdlp::safe(&id);
                let cached = self.cfg.cache_dir.join(format!("{stem}.mp3"));
                if cached.exists() {
                    self.start(t, cached);
                    return;
                }
                self.pending = Some(id.clone());
                self.status = format!("⬇ buffering: {}", t.label());
                let (tx, bin, dir) = (
                    self.tx.clone(),
                    self.cfg.ytdlp.clone(),
                    self.cfg.cache_dir.clone(),
                );
                let thumb = t.thumb.clone();
                thread::spawn(move || {
                    let _ = match ytdlp::download(&bin, &url, &dir, &stem, thumb.as_deref()) {
                        Ok(p) => tx.send(Msg::Ready(t, p)),
                        Err(e) => tx.send(Msg::FetchFail(id, e.to_string())),
                    };
                });
            }
        }
    }

    fn start(&mut self, t: Track, path: PathBuf) {
        match self.player.play(&path) {
            Ok(()) => {
                self.status = format!("▶ {}", t.label());
                self.current = Some(t.clone());
                self.lyrics.clear();
                self.lyrics_loading = true;
                {
                    let (tx, cache, t2) = (self.tx.clone(), self.cfg.cache_dir.clone(), t.clone());
                    thread::spawn(move || {
                        let lines = crate::lyrics::load(&cache, &t2);
                        let _ = tx.send(Msg::Lyrics(t2.key(), lines));
                    });
                }
                self.load_cover(&t, &path);
                self.rpc_update();
            }
            Err(e) => self.status = format!("✗ {e}"),
        }
    }

    /// Fetch or extract the cover for `t` (local: immediately, remote: in the background).
    fn load_cover(&mut self, t: &Track, audio: &Path) {
        let key = t.key();
        let dir = covers::dir(&self.cfg.cache_dir);
        self.cover_key = Some(key.clone());
        self.cover_shown = false;
        self.cover_sig = None;
        self.cover = covers::cached(&self.cfg.cache_dir, t);
        if self.cover.is_some() {
            return;
        }
        // A downloaded track already has its `<stem>.png` next to it (and the
        // picture embedded in the tags), so do not hit the network again.
        if let Some(side) = covers::sidecar(audio) {
            if let Ok(bytes) = std::fs::read(side) {
                self.cover = covers::store(&dir, &key, &bytes).ok();
            }
        }
        if self.cover.is_some() {
            return;
        }
        match &t.source {
            Source::Local(_) => self.cover = covers::local(&self.cfg.cache_dir, t),
            Source::Remote { .. } => {
                let (tx, bin, t2) = (self.tx.clone(), self.cfg.ytdlp.clone(), t.clone());
                thread::spawn(move || {
                    if let Ok(p) = covers::fetch_remote(&bin, &t2, &dir) {
                        let _ = tx.send(Msg::CoverReady(key, p));
                    }
                });
            }
        }
    }

    fn advance(&mut self, auto: bool) {
        match self.queue.next(auto).cloned() {
            Some(t) => self.play_track(t),
            None if auto => {
                self.current = None;
                self.rpc.clear();
            }
            None => {}
        }
    }

    fn rpc_update(&mut self) {
        if let Some(t) = &self.current {
            self.rpc
                .update(t, self.player.position(), self.player.is_paused());
        }
    }

    fn stop_playback(&mut self) {
        self.player.stop();
        self.current = None;
        self.pending = None;
        self.rpc.clear();
        self.status = "■ stopped".into();
    }

    pub fn tick(&mut self) {
        self.check_colors();
        if self.dirty && self.last_save.elapsed() >= Duration::from_secs(1) {
            self.persist();
        }
        // Keep the presence alive: this re-sends after Discord restarts and
        // refreshes the state without waiting for the next key press.
        if self.current.is_some() && self.last_rpc.elapsed() >= Duration::from_secs(15) {
            self.last_rpc = Instant::now();
            self.rpc_update();
        }
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Lyrics(key, lines) => {
                    if self.current.as_ref().map(|t| t.key()) == Some(key) {
                        self.lyrics = lines;
                        self.lyrics_loading = false;
                    }
                }
                Msg::Results(r) => {
                    self.status = format!("{} results", r.len());
                    self.results = r;
                    self.sel[Tab::Search.idx()] = 0;
                    self.tab = Tab::Search;
                }
                Msg::Ready(t, p) => {
                    if self.pending.as_deref() == Some(t.key().as_str()) {
                        self.pending = None;
                        self.start(t, p);
                    }
                }
                Msg::DlDone(i) => {
                    if let Some(d) = self.downloads.get_mut(i) {
                        d.state = Dl::Done;
                    }
                    self.library = library::scan(&self.cfg.music_dir);
                    self.status = "✓ saved to library".into();
                }
                Msg::DlFail(i, e) => {
                    if let Some(d) = self.downloads.get_mut(i) {
                        d.state = Dl::Failed(e.clone());
                    }
                    self.status = format!("✗ {e}");
                }
                Msg::Error(e) => self.status = format!("✗ {e}"),
                Msg::FetchFail(id, e) => {
                    if self.pending.as_deref() == Some(id.as_str()) {
                        self.pending = None;
                    }
                    self.status = format!("✗ {e}");
                }
                Msg::CoverReady(key, path) => {
                    if self.cover_key.as_deref() == Some(key.as_str()) {
                        self.cover = Some(path);
                        self.cover_shown = false;
                    }
                }
            }
        }
        if self.player.poll_finished() {
            self.advance(true);
        }
        self.viz
            .update(&self.player.samples, self.player.is_playing());
    }

    // ---------- input ----------

    pub fn on_key(&mut self, k: KeyEvent) {
        match self.mode {
            Mode::Input(kind) => self.on_key_input(kind, k),
            Mode::Help => self.mode = Mode::Normal,
            Mode::Normal => self.on_key_normal(k),
        }
    }

    fn on_key_input(&mut self, kind: InputKind, k: KeyEvent) {
        match k.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(c) => self.input.push(c),
            KeyCode::Enter => {
                self.mode = Mode::Normal;
                let text = self.input.trim().to_string();
                if text.is_empty() {
                    return;
                }
                match kind {
                    InputKind::Search => self.search(text),
                    InputKind::PlaylistName => self.save_playlist(&text),
                }
            }
            _ => {}
        }
    }

    fn search(&mut self, q: String) {
        self.status = format!("searching {}…", self.provider.name());
        let (tx, bin, p) = (self.tx.clone(), self.cfg.ytdlp.clone(), self.provider);
        thread::spawn(move || {
            let _ = match ytdlp::search(&bin, p, &q) {
                Ok(r) => tx.send(Msg::Results(r)),
                Err(e) => tx.send(Msg::Error(e.to_string())),
            };
        });
    }

    fn save_playlist(&mut self, name: &str) {
        if self.queue.len() == 0 {
            self.status = "queue is empty".into();
            return;
        }
        let name = ytdlp::safe(name);
        self.status = match playlist::save(&name, &self.queue.to_vec()) {
            Ok(()) => {
                self.playlists = playlist::list();
                format!("✓ playlist \"{name}\" saved")
            }
            Err(e) => format!("✗ {e}"),
        };
    }

    fn on_key_normal(&mut self, k: KeyEvent) {
        use KeyCode::{BackTab, Char, Down, Enter, Left, Right, Up};
        match (k.code, k.modifiers) {
            (Char('q'), _) | (Char('c'), KeyModifiers::CONTROL) => self.quit = true,
            (Char('?'), _) => self.mode = Mode::Help,
            (Char(' '), _) => {
                self.player.toggle_pause();
                self.rpc_update();
            }
            (Char('n'), _) => self.advance(false),
            (Char('p'), _) => {
                if self.player.position() > Duration::from_secs(3) {
                    if let Some(t) = self.current.clone() {
                        self.play_track(t);
                    }
                } else if let Some(t) = self.queue.prev().cloned() {
                    self.play_track(t);
                }
            }
            (Left, _) => {
                self.player.seek(-5);
                self.rpc_update();
            }
            (Right, _) => {
                self.player.seek(5);
                self.rpc_update();
            }
            (Char('+'), _) | (Char('='), _) => {
                self.player.change_volume(0.05);
                self.touch_config();
            }
            (Char('-'), _) => {
                self.player.change_volume(-0.05);
                self.touch_config();
            }
            (Char('s'), _) => self.queue.toggle_shuffle(),
            (Char('r'), _) => self.queue.cycle_repeat(),
            (Char('v'), _) => {
                self.viz_mode = self.viz_mode.next();
                self.touch_config();
            }
            (Char('['), _) | (Char(']'), _) => {
                let d = if k.code == Char(']') { 0.1 } else { -0.1 };
                self.cfg.wave_scale = (self.cfg.wave_scale + d).clamp(0.4, 2.5);
                self.last_graphics = None;
                self.touch_config();
                self.status = format!("wave scale ×{:.1}", self.cfg.wave_scale);
            }
            (Char('T'), _) => {
                self.preset = self.preset.next();
                self.cfg.layout = self.preset.name().into();
                self.touch_config();
                self.status = format!("layout: {}", self.preset.name());
            }
            (Char('P'), _) => {
                self.provider = self.provider.toggle();
                self.touch_config();
            }
            (KeyCode::Tab, _) => self.tab = Tab::ALL[(self.tab.idx() + 1) % 6],
            (BackTab, _) => self.tab = Tab::ALL[(self.tab.idx() + 5) % 6],
            (Char(c @ '1'..='6'), _) => self.tab = Tab::ALL[c as usize - '1' as usize],
            (Char('j'), _) | (Down, _) => self.move_sel(1),
            (Char('k'), _) | (Up, _) => self.move_sel(-1),
            (Char('g'), _) => self.move_sel(-1_000_000),
            (Char('G'), _) => self.move_sel(1_000_000),
            (Char('/'), _) => {
                self.mode = Mode::Input(InputKind::Search);
                self.input.clear();
                self.tab = Tab::Search;
            }
            (Char('w'), _) => {
                self.mode = Mode::Input(InputKind::PlaylistName);
                self.input.clear();
            }
            (Enter, _) => self.activate(),
            (Char('a'), _) => self.enqueue(),
            (Char('d'), _) => self.delete_selected(),
            (Char('D'), _) => self.save_selected(),
            _ => {}
        }
    }

    fn delete_selected(&mut self) {
        match self.tab {
            Tab::Queue => {
                let removed = self.queue.remove(self.sel[Tab::Queue.idx()]);
                self.move_sel(0);
                if removed == crate::queue::Removed::Current {
                    if self.player.is_active() {
                        match self.queue.current().cloned() {
                            Some(t) => self.play_track(t),
                            None => self.stop_playback(),
                        }
                    } else {
                        self.current = self.queue.current().cloned();
                        self.rpc.clear();
                    }
                }
            }
            Tab::Playlists => {
                if let Some(name) = self.playlists.get(self.sel[Tab::Playlists.idx()]).cloned() {
                    let _ = playlist::delete(&name);
                    self.playlists = playlist::list();
                    self.status = format!("playlist \"{name}\" deleted");
                    self.move_sel(0);
                }
            }
            _ => {}
        }
    }

    fn activate(&mut self) {
        let sel = self.sel[self.tab.idx()];
        match self.tab {
            Tab::Library | Tab::Search => {
                let list: Vec<Track> = self.items(self.tab).into_iter().cloned().collect();
                if list.is_empty() {
                    return;
                }
                if let Some(t) = self.queue.set(list, sel).cloned() {
                    self.play_track(t);
                }
            }
            Tab::Queue => {
                if let Some(t) = self.queue.jump(sel).cloned() {
                    self.play_track(t);
                }
            }
            Tab::Playlists => {
                let Some(name) = self.playlists.get(sel).cloned() else {
                    return;
                };
                match playlist::load(&name) {
                    Ok(list) if !list.is_empty() => {
                        if let Some(t) = self.queue.set(list, 0).cloned() {
                            self.play_track(t);
                        }
                    }
                    Ok(_) => self.status = "playlist is empty".into(),
                    Err(e) => self.status = format!("✗ {e}"),
                }
            }
            Tab::Settings => match sel {
                0 => {
                    self.provider = self.provider.toggle();
                    self.touch_config();
                }
                1 => {
                    self.viz_mode = self.viz_mode.next();
                    self.touch_config();
                }
                2 => {
                    self.preset = self.preset.next();
                    self.cfg.layout = self.preset.name().into();
                    self.touch_config();
                }
                _ => {}
            },
            Tab::Downloads => {}
        }
    }

    fn enqueue(&mut self) {
        if !matches!(self.tab, Tab::Library | Tab::Search) {
            return;
        }
        if let Some(t) = self.selected_track() {
            let pos = self.queue.push(t.clone());
            self.status = format!("+ queued: {}", t.label());
            if self.current.is_none() {
                if let Some(t) = self.queue.jump(pos).cloned() {
                    self.play_track(t);
                }
            }
        }
    }

    /// Download the selected remote track into the music folder.
    fn save_selected(&mut self) {
        let Some(t) = self.selected_track() else {
            return;
        };
        let Source::Remote { url, .. } = t.source.clone() else {
            self.status = "already local".into();
            return;
        };
        self.status = format!("⬇ downloading: {}", t.label());
        self.downloads.push(Download {
            label: t.label(),
            state: Dl::Active,
        });
        let idx = self.downloads.len() - 1;
        let stem = ytdlp::safe(&t.label().replace(" — ", " - "));
        let (tx, bin, dir) = (
            self.tx.clone(),
            self.cfg.ytdlp.clone(),
            self.cfg.music_dir.clone(),
        );
        let thumb = t.thumb.clone();
        thread::spawn(move || {
            let _ = match ytdlp::download(&bin, &url, &dir, &stem, thumb.as_deref()) {
                Ok(_) => tx.send(Msg::DlDone(idx)),
                Err(e) => tx.send(Msg::DlFail(idx, e.to_string())),
            };
        });
    }
}

/// Clickable regions captured during the last draw.
#[derive(Clone, Default)]
pub struct Hits {
    pub tabs: Vec<(Rect, usize)>,
    pub list: Rect,
    pub list_offset: usize,
    pub progress: Rect,
    pub viz: Rect,
    pub cover: Rect,
}

impl App {
    /// Reloads the whole palette from ~/.config/rmp/colors.json when matugen rewrites it.
    pub fn check_colors(&mut self) {
        if self.last_check.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_check = Instant::now();
        let path = crate::matugen::colors_path();
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if mtime == self.colors_mtime {
            return;
        }
        self.colors_mtime = mtime;
        if mtime.is_some() {
            self.theme = crate::theme::Theme::load(&self.cfg);
            self.status = "palette updated from colors.json".into();
            self.last_graphics = None;
        }
    }

    pub fn on_mouse(&mut self, m: MouseEvent) {
        if self.mode != Mode::Normal {
            return;
        }
        let h = self.hits.borrow().clone();
        let (x, y) = (m.column, m.row);
        let inside = |r: Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        match m.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let d: i64 = if m.kind == MouseEventKind::ScrollUp {
                    -1
                } else {
                    1
                };
                if inside(h.list) {
                    self.move_sel(d);
                } else {
                    self.player.change_volume(-0.05 * d as f32);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some((_, i)) = h.tabs.iter().find(|(r, _)| inside(*r)) {
                    self.tab = Tab::ALL[*i];
                    return;
                }
                if inside(h.progress) && h.progress.width > 1 {
                    let ratio = (x - h.progress.x) as f64 / (h.progress.width - 1) as f64;
                    let dur = self
                        .player
                        .duration
                        .map(|d| d.as_secs())
                        .or(self.current.as_ref().and_then(|t| t.duration));
                    if let Some(d) = dur {
                        self.player
                            .seek_to((ratio.clamp(0.0, 1.0) * d as f64) as u64);
                        self.rpc_update();
                    }
                    return;
                }
                if inside(h.list) && y > h.list.y && y < h.list.y + h.list.height - 1 {
                    let row = (y - h.list.y - 1) as usize + h.list_offset;
                    let tab = self.tab;
                    if row < self.len(tab) {
                        let double = matches!(
                            self.last_click,
                            Some((t, r, when)) if t == tab && r == row && when.elapsed() < Duration::from_millis(450)
                        );
                        self.sel[tab.idx()] = row;
                        self.last_click = Some((tab, row, Instant::now()));
                        if double {
                            self.activate();
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

impl App {
    /// True when the wave should be drawn as a kitty image instead of cells.
    pub fn wave_graphics(&self) -> bool {
        self.graphics && self.viz_mode == VizMode::Wave && self.mode != Mode::Help
    }

    /// Called after each frame: draws or removes the kitty wave image.
    pub fn draw_graphics(&mut self, out: &mut impl std::io::Write) {
        let rect = self.hits.borrow().viz;
        if !self.wave_graphics() || rect.width < 3 || rect.height < 2 {
            if self.wave_id != 0 {
                let _ = kittyviz::clear_id(out, self.wave_id);
                self.wave_id = 0;
            }
            self.last_graphics = None;
            return;
        }

        let (cw, ch) = kittyviz::cell_px();
        // The line is smooth, so kitty can upscale; keeping the bitmap modest
        // avoids writing tens of megabytes to the frame file 30 times a second.
        let w = (rect.width as u32 * cw).clamp(16, 1600);
        let h = (rect.height as u32 * ch).clamp(16, 900);

        let mut hasher = DefaultHasher::new();
        w.hash(&mut hasher);
        h.hash(&mut hasher);
        self.theme.accent.hash(&mut hasher);
        self.cfg.wave_scale.to_bits().hash(&mut hasher);
        for s in self.viz.wave.iter().chain(self.viz.ghost.iter()) {
            s.to_bits().hash(&mut hasher);
        }
        let sig = hasher.finish();
        if self.wave_id != 0 && self.last_graphics == Some(sig) {
            return; // nothing moved (paused): keep the current image
        }

        // Use the real pixel height of one cell after clamping, not the nominal one.
        let cell_h = (h / rect.height.max(1) as u32).max(1);
        let style = kittyviz::Style::for_cell(cell_h, self.cfg.wave_scale);
        kittyviz::render_wave_into(
            &mut self.frame_buf,
            w,
            h,
            &self.viz.wave,
            &self.viz.ghost,
            self.theme.accent,
            style,
        );
        let id = if self.wave_id == 1 { 3 } else { 1 };
        if kittyviz::show(
            out,
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            w,
            h,
            &self.frame_buf,
            id,
        )
        .is_ok()
        {
            let old = self.wave_id;
            self.wave_id = id;
            self.last_graphics = Some(sig);
            if old != 0 && old != id {
                let _ = kittyviz::clear_id(out, old);
            }
        }
    }

    /// True when the Now Playing cover is drawn as a kitty image instead of cells.
    pub fn cover_graphics(&self) -> bool {
        self.graphics && self.mode != Mode::Help
    }

    /// Place or remove the cover image (kitty id 2).
    pub fn draw_cover(&mut self, out: &mut impl std::io::Write) {
        let rect = self.hits.borrow().cover;
        let visible =
            self.cover_graphics() && self.cover.is_some() && rect.width >= 2 && rect.height >= 2;
        if !visible {
            if self.cover_shown {
                let _ = kittyviz::clear_id(out, 2);
                self.cover_shown = false;
            }
            self.cover_sig = None;
            return;
        }
        let path = self.cover.clone().expect("checked above");
        let sig = (path.clone(), rect.x, rect.y, rect.width, rect.height);
        if self.cover_shown && self.cover_sig.as_ref() == Some(&sig) {
            return;
        }
        let (cw, ch) = kittyviz::cell_px();
        let (iw, ih) = image::image_dimensions(&path).unwrap_or((1, 1));
        let (cols, rows) = kittyviz::fit_cells(iw, ih, rect.width, rect.height, cw, ch);
        let x = rect.x + (rect.width - cols) / 2;
        let y = rect.y + (rect.height - rows) / 2;
        if kittyviz::show_png(out, x, y, cols, rows, &path, 2).is_ok() {
            self.cover_shown = true;
            self.cover_sig = Some((path, rect.x, rect.y, rect.width, rect.height));
        }
    }
}
