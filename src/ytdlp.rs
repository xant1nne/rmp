//! YouTube / SoundCloud through the external `yt-dlp` binary.
use crate::track::{Source, Track};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Copy, PartialEq)]
pub enum Provider {
    YouTube,
    SoundCloud,
}

impl Provider {
    pub fn toggle(self) -> Self {
        match self {
            Provider::YouTube => Provider::SoundCloud,
            Provider::SoundCloud => Provider::YouTube,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Provider::YouTube => "YouTube",
            Provider::SoundCloud => "SoundCloud",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Provider::YouTube => "youtube",
            Provider::SoundCloud => "soundcloud",
        }
    }
    pub fn from_name(s: &str) -> Self {
        if s.eq_ignore_ascii_case("soundcloud") || s.eq_ignore_ascii_case("sc") {
            Provider::SoundCloud
        } else {
            Provider::YouTube
        }
    }
    fn prefix(self) -> &'static str {
        match self {
            Provider::YouTube => "ytsearch15:",
            Provider::SoundCloud => "scsearch15:",
        }
    }
}

pub fn safe(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn search(bin: &str, provider: Provider, query: &str) -> Result<Vec<Track>> {
    let out = Command::new(bin)
        .args(["--dump-json", "--flat-playlist", "--no-warnings"])
        .arg(format!("{}{}", provider.prefix(), query))
        .output()
        .with_context(|| format!("cannot run `{bin}` (is yt-dlp installed?)"))?;
    if !out.status.success() {
        bail!(
            "yt-dlp: {}",
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .last()
                .unwrap_or("failed")
        );
    }
    let mut tracks = vec![];
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
        let id = v
            .get("id")
            .map(|x| x.to_string().trim_matches('"').to_owned())
            .unwrap_or_default();
        let url = s("webpage_url").or_else(|| s("url")).unwrap_or_default();
        if url.is_empty() {
            continue;
        }
        let thumb = s("thumbnail").or_else(|| {
            v.get("thumbnails")
                .and_then(Value::as_array)
                .and_then(|a| a.last())
                .and_then(|t| t.get("url"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
        tracks.push(Track {
            title: s("title").unwrap_or_else(|| "?".into()),
            artist: s("uploader").or_else(|| s("channel")).unwrap_or_default(),
            duration: v.get("duration").and_then(Value::as_f64).map(|d| d as u64),
            album: None,
            thumb,
            source: Source::Remote { url, id },
        });
    }
    Ok(tracks)
}

/// Download audio as mp3 into `dir/<stem>.mp3` (needs ffmpeg for the conversion).
/// The artwork is downloaded too, normalised to a 512 px `<stem>.png` sidecar
/// and embedded into the audio tags so every file keeps its own cover.
pub fn download(
    bin: &str,
    url: &str,
    dir: &Path,
    stem: &str,
    thumb: Option<&str>,
) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let template = dir.join(format!("{stem}.%(ext)s"));
    let out = Command::new(bin)
        .args([
            "-x",
            "--audio-format",
            "mp3",
            "--write-thumbnail",
            "--convert-thumbnails",
            "jpg",
            "--no-playlist",
            "--no-warnings",
            "-o",
        ])
        .arg(&template)
        .arg(url)
        .output()
        .with_context(|| format!("cannot run `{bin}`"))?;
    if !out.status.success() {
        bail!(
            "yt-dlp: {}",
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .last()
                .unwrap_or("download failed")
        );
    }
    let audio = dir.join(format!("{stem}.mp3"));
    if !audio.is_file() {
        bail!("yt-dlp finished but {} is missing", audio.display());
    }
    // Remember where the artwork came from: a local file then still has a
    // public URL that Discord can use as its per-track large image.
    if let Some(thumb) = thumb.filter(|u| u.starts_with("https://")) {
        let _ = std::fs::write(dir.join(format!("{stem}.url")), thumb);
    }
    // Best effort: a missing thumbnail must not invalidate a good download.
    let _ = crate::covers::attach_downloaded(&audio);
    Ok(audio)
}
