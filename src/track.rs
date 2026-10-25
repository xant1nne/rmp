use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Source {
    Local(PathBuf),
    Remote { url: String, id: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Track {
    pub title: String,
    pub artist: String,
    pub duration: Option<u64>, // seconds
    pub source: Source,
    /// Album name from tags, when available.
    #[serde(default)]
    pub album: Option<String>,
    /// Thumbnail URL for remote tracks (filled by `yt-dlp --dump-json`).
    #[serde(default)]
    pub thumb: Option<String>,
}

impl Track {
    pub fn label(&self) -> String {
        if self.artist.is_empty() {
            self.title.clone()
        } else {
            format!("{} — {}", self.title, self.artist)
        }
    }

    /// Stable key used to match async download results to the request.
    pub fn key(&self) -> String {
        match &self.source {
            Source::Local(p) => p.to_string_lossy().into_owned(),
            Source::Remote { id, .. } => id.clone(),
        }
    }
}
