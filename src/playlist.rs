use crate::{config::config_dir, track::Track};
use anyhow::Result;
use std::{fs, path::PathBuf};

fn dir() -> PathBuf {
    config_dir().join("playlists")
}

pub fn save(name: &str, tracks: &[Track]) -> Result<()> {
    fs::create_dir_all(dir())?;
    fs::write(
        dir().join(format!("{name}.json")),
        serde_json::to_string_pretty(tracks)?,
    )?;
    Ok(())
}

pub fn load(name: &str) -> Result<Vec<Track>> {
    Ok(serde_json::from_str(&fs::read_to_string(
        dir().join(format!("{name}.json")),
    )?)?)
}

pub fn delete(name: &str) -> Result<()> {
    fs::remove_file(dir().join(format!("{name}.json")))?;
    Ok(())
}

pub fn list() -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir())
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            (p.extension().and_then(|x| x.to_str()) == Some("json"))
                .then(|| p.file_stem().and_then(|s| s.to_str()).map(str::to_owned))
                .flatten()
        })
        .collect();
    v.sort();
    v
}
