use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

/// Discord application used for Rich Presence by default.
pub const DEFAULT_DISCORD_APP_ID: &str = "1561845271334486016";

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub music_dir: PathBuf,
    pub cache_dir: PathBuf,
    /// Application ID from https://discord.com/developers/applications (empty = RPC off)
    pub discord_app_id: String,
    /// Name of the large image asset uploaded to the Discord application
    pub discord_image_key: String,
    /// Optional small badge asset (e.g. "pause"); empty = no small image
    pub discord_small_key: String,
    /// Show an "Open track" button on remote tracks
    pub discord_buttons: bool,
    pub ytdlp: String,
    pub volume: f32,
    /// UI accent color (RGB) when the matugen palette is not used
    pub accent: [u8; 3],
    /// start visualizer: wave | bars | mirror | spectrum
    pub viz: String,
    /// "auto" uses kitty graphics when running in kitty, "off" disables it
    pub graphics: String,
    /// Wave line thickness multiplier (0.5 = thin, 1.0 = normal, 1.8 = thick)
    pub wave_scale: f32,
    /// "matugen" follows ~/.config/rmp/colors.json, "builtin" always uses `accent`
    pub theme: String,
    /// last search source: youtube | soundcloud
    pub provider: String,
    /// screen preset: resonance | minimal | focus
    pub layout: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            music_dir: dirs::audio_dir().unwrap_or_else(|| PathBuf::from("~/Music")),
            cache_dir: dirs::cache_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join("rmp"),
            discord_app_id: DEFAULT_DISCORD_APP_ID.into(),
            discord_image_key: "logo".into(),
            discord_small_key: String::new(),
            discord_buttons: true,
            ytdlp: "yt-dlp".into(),
            volume: 0.7,
            accent: [127, 214, 190],
            viz: "wave".into(),
            graphics: "auto".into(),
            wave_scale: 1.0,
            theme: "matugen".into(),
            provider: "youtube".into(),
            layout: "auto".into(),
        }
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rmp")
}

pub fn path() -> PathBuf {
    config_dir().join("config.json")
}

/// Write the config atomically (tmp file + rename), creating the directory if needed.
pub fn save(cfg: &Config) -> std::io::Result<()> {
    let dir = config_dir();
    fs::create_dir_all(&dir)?;
    let target = dir.join("config.json");
    let tmp = dir.join("config.json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(cfg).unwrap_or_default())?;
    fs::rename(tmp, target)
}

pub fn load() -> Config {
    let file = path();
    if let Ok(s) = fs::read_to_string(&file) {
        if let Ok(cfg) = serde_json::from_str::<Config>(&s) {
            // Rewrite when new fields appeared, so the file always documents every option.
            if serde_json::to_string_pretty(&cfg)
                .map(|fresh| fresh != s)
                .unwrap_or(false)
            {
                let _ = save(&cfg);
            }
            return cfg;
        }
    }
    let cfg = Config::default();
    let _ = save(&cfg);
    cfg
}
