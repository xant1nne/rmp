//! End-to-end matugen integration: install the template, wire up
//! `~/.config/matugen/config.toml` and generate `~/.config/rmp/colors.json`.
use anyhow::{bail, Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use crate::config::config_dir;

pub const TEMPLATE: &str = include_str!("../extras/matugen/rmp-colors.json");

pub fn matugen_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("matugen")
}

pub fn matugen_config() -> PathBuf {
    matugen_dir().join("config.toml")
}

pub fn template_path() -> PathBuf {
    matugen_dir().join("templates").join("rmp-colors.json")
}

pub fn colors_path() -> PathBuf {
    config_dir().join("colors.json")
}

fn template_block() -> String {
    format!(
        "[templates.rmp]\ninput_path = '{}'\noutput_path = '{}'\n",
        template_path().display(),
        colors_path().display()
    )
}

/// Make `~/.config/matugen/config.toml` usable by matugen 3.x without touching
/// unrelated user settings.
fn ensure_matugen_config() -> Result<()> {
    let dir = matugen_dir();
    fs::create_dir_all(dir.join("templates"))?;
    let path = matugen_config();
    let mut text = fs::read_to_string(&path).unwrap_or_default();

    // matugen 3 refuses to start when the file exists but has no `[config]` table.
    let has_config = text.lines().any(|l| l.trim() == "[config]");
    if !has_config {
        text = format!("[config]\nversion_check = false\n\n{text}");
    }

    if text.contains("[templates.rmp]") {
        // Rewrite the block so absolute paths track the current HOME.
        let start = text.find("[templates.rmp]").unwrap();
        let tail = &text[start + 1..];
        let end = tail
            .find("\n[")
            .map(|i| start + 1 + i)
            .unwrap_or(text.len());
        text.replace_range(start..end, "");
        text = text.trim_end().to_string() + "\n\n";
    }
    if !text.ends_with('\n') && !text.is_empty() {
        text.push('\n');
    }
    text.push_str(&template_block());

    // Validate by letting matugen parse it in a dry run later; write atomically.
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, &text)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// Install everything and run matugen once on `image`.
pub fn setup(image: &Path) -> Result<PathBuf> {
    if !image.is_file() {
        bail!("image not found: {}", image.display());
    }
    fs::create_dir_all(matugen_dir().join("templates"))?;
    fs::write(template_path(), TEMPLATE)?;
    ensure_matugen_config()?;

    let out = Command::new("matugen")
        .arg("image")
        .arg(image)
        .args(["-m", "dark", "-q"])
        .output()
        .context("cannot run `matugen` (is it installed?)")?;
    if !out.status.success() {
        bail!(
            "matugen failed: {}",
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .last()
                .unwrap_or("unknown error")
        );
    }

    let colors = colors_path();
    verify(&colors)?;
    Ok(colors)
}

/// Check that the generated palette has at least the whole set of keys we use.
pub fn verify(path: &Path) -> Result<()> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("matugen did not write {}", path.display()))?;
    let v: serde_json::Value =
        serde_json::from_str(&text).context("colors.json is not valid JSON")?;
    let missing: Vec<&str> = ["accent", "surface", "background", "outline", "on_surface"]
        .into_iter()
        .filter(|k| v.get(*k).and_then(|x| x.as_str()).is_none())
        .collect();
    if !missing.is_empty() {
        bail!("colors.json is missing keys: {}", missing.join(", "));
    }
    Ok(())
}
