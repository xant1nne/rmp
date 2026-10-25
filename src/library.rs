use crate::track::{Source, Track};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::prelude::Accessor;
use std::path::Path;
use walkdir::WalkDir;

const EXT: [&str; 8] = ["mp3", "flac", "ogg", "wav", "m4a", "aac", "opus", "mp4"];

/// Metadata read from a file's tags (falls back to nothing).
#[derive(Default)]
pub struct Tags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration: Option<u64>,
}

pub fn read_tags(path: &Path) -> Tags {
    let Ok(file) = lofty::read_from_path(path) else {
        return Tags::default();
    };
    let tag = file.primary_tag().or_else(|| file.first_tag());
    let mut out = Tags::default();
    if let Some(tag) = tag {
        let clean = |s: Option<std::borrow::Cow<'_, str>>| {
            s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
        };
        out.title = clean(tag.title());
        out.artist = clean(tag.artist());
        out.album = clean(tag.album());
    }
    let secs = file.properties().duration().as_secs();
    if secs > 0 {
        out.duration = Some(secs);
    }
    out
}

pub fn scan(dir: &Path) -> Vec<Track> {
    let mut out: Vec<Track> = WalkDir::new(dir)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| EXT.contains(&x.to_lowercase().as_str()))
                .unwrap_or(false)
        })
        .map(|e| {
            let path = e.path();
            let tags = read_tags(path);
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
            // Without tags, understand "Artist - Title" file names.
            let (file_artist, file_title) = match stem.split_once(" - ") {
                Some((a, t)) => (a.trim().to_string(), t.trim().to_string()),
                None => (String::new(), stem.to_string()),
            };
            // A download stores its public artwork URL in `<stem>.url`, so even a
            // local file can show the real cover in Discord Rich Presence.
            let thumb = std::fs::read_to_string(path.with_extension("url"))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|u| u.starts_with("https://"));
            Track {
                title: tags.title.unwrap_or(file_title),
                artist: tags.artist.unwrap_or(file_artist),
                duration: tags.duration,
                album: tags.album,
                thumb,
                source: Source::Local(path.to_path_buf()),
            }
        })
        .collect();
    out.sort_by_key(|t| t.label().to_lowercase());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tags_are_empty() {
        let t = read_tags(Path::new("/nonexistent/file.mp3"));
        assert!(t.title.is_none());
        assert!(t.duration.is_none());
    }
}
