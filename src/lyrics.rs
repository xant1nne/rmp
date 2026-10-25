//! Synced lyrics: LRCLIB lookup through `curl`, an on-disk cache and LRC parsing.
//! Only timing and layout logic lives here; no lyric text is stored in the code.
use crate::track::Track;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// (seconds, text). Plain (unsynced) lyrics use 0.0 for every line.
pub type Lines = Vec<(f64, String)>;

fn hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    })
}

fn cache_path(cache_dir: &Path, track: &Track) -> PathBuf {
    let key = format!("{}|{}", track.artist, track.title);
    cache_dir.join("lyrics").join(format!("{:016x}.lrc", hash(&key)))
}

fn parse_time(tag: &str) -> Option<f64> {
    let (m, s) = tag.split_once(':')?;
    let m: f64 = m.trim().parse().ok()?;
    let s: f64 = s.trim().parse().ok()?;
    Some(m * 60.0 + s)
}

/// Parses `[mm:ss.xx]` timestamps. Metadata tags such as `[ar:...]` are skipped.
pub fn parse_lrc(text: &str) -> Lines {
    let mut out: Lines = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim();
        let mut times = Vec::new();
        while let Some(stripped) = rest.strip_prefix('[') {
            let Some(end) = stripped.find(']') else { break };
            match parse_time(&stripped[..end]) {
                Some(t) => times.push(t),
                None => break,
            }
            rest = stripped[end + 1..].trim_start();
        }
        for t in times {
            out.push((t, rest.to_string()));
        }
    }
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    out
}

pub fn to_lines(text: &str) -> Lines {
    let timed = parse_lrc(text);
    if !timed.is_empty() {
        return timed;
    }
    text.lines()
        .map(|l| (0.0, l.trim().to_string()))
        .filter(|(_, l)| !l.is_empty())
        .collect()
}

/// True when the lines carry real timestamps (so they can be highlighted).
pub fn is_synced(lines: &Lines) -> bool {
    lines.iter().any(|(t, _)| *t > 0.0)
}

/// Index of the line that should be highlighted at playback position `pos` (seconds).
pub fn current_index(lines: &Lines, pos: f64) -> Option<usize> {
    let idx = lines.partition_point(|(t, _)| *t <= pos);
    idx.checked_sub(1)
}

/// Strips "(Official video)" style suffixes and YouTube's " - Topic" artist suffix.
fn clean(s: &str) -> String {
    s.split(['(', '['])
        .next()
        .unwrap_or(s)
        .trim()
        .to_string()
}

/// Ok(Some) found, Ok(None) the service has no entry, Err(()) network/tool failure (not cached).
fn fetch(track: &Track) -> Result<Option<String>, ()> {
    let artist = track.artist.trim_end_matches(" - Topic").trim().to_string();
    if artist.is_empty() {
        return Ok(None);
    }
    let title = clean(&track.title);
    let title = if title.is_empty() { track.title.clone() } else { title };

    let mut cmd = Command::new("curl");
    cmd.args(["-s", "-m", "10", "-G", "https://lrclib.net/api/get"])
        .args(["--data-urlencode"])
        .arg(format!("track_name={title}"))
        .args(["--data-urlencode"])
        .arg(format!("artist_name={artist}"));
    if let Some(d) = track.duration.filter(|d| *d > 0) {
        cmd.args(["--data-urlencode"]).arg(format!("duration={d}"));
    }
    let out = cmd.output().map_err(|_| ())?;
    if !out.status.success() {
        return Err(());
    }
    let Ok(v) = serde_json::from_slice::<Value>(&out.stdout) else {
        return Ok(None);
    };
    let pick = |k: &str| v.get(k).and_then(Value::as_str).filter(|s| !s.trim().is_empty());
    Ok(pick("syncedLyrics")
        .or_else(|| pick("plainLyrics"))
        .map(str::to_owned))
}

/// Cached lyrics for `track`, fetching and storing them on a miss. Runs off the UI thread.
pub fn load(cache_dir: &Path, track: &Track) -> Lines {
    let path = cache_path(cache_dir, track);
    if let Ok(text) = fs::read_to_string(&path) {
        return to_lines(&text);
    }
    match fetch(track) {
        Ok(got) => {
            if let Some(dir) = path.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let text = got.unwrap_or_default();
            let _ = fs::write(&path, &text);
            to_lines(&text)
        }
        Err(()) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_timestamps_and_skips_metadata() {
        let text = "[ar:Someone]\n[00:01.50][00:03.00]line a\n[00:02.00]line b\n";
        let lines = parse_lrc(text);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].0, 1.5);
        assert_eq!(lines[1].1, "line b");
        assert_eq!(lines[2].0, 3.0);
    }

    #[test]
    fn current_line_follows_position() {
        let lines = vec![(1.0, "a".to_string()), (2.0, "b".to_string()), (5.0, "c".to_string())];
        assert_eq!(current_index(&lines, 0.5), None);
        assert_eq!(current_index(&lines, 2.5), Some(1));
        assert_eq!(current_index(&lines, 99.0), Some(2));
    }

    #[test]
    fn plain_text_is_unsynced() {
        let lines = to_lines("first\n\nsecond\n");
        assert!(!is_synced(&lines));
        assert_eq!(lines.len(), 2);
    }
}
