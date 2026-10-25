//! Cover art: extract embedded pictures, find folder covers and fetch remote
//! thumbnails. Everything is normalised to a small PNG that kitty can display.
use crate::track::{Source, Track};
use anyhow::{bail, Context, Result};
use lofty::file::{AudioFile, TaggedFileExt};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub fn dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join("covers")
}

fn hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    })
}

pub fn path_for(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{:016x}.png", hash(key)))
}

/// Embedded picture from the file's tags.
pub fn embedded(track: &Track) -> Option<Vec<u8>> {
    let Source::Local(path) = &track.source else {
        return None;
    };
    let file = lofty::read_from_path(path).ok()?;
    let tag = file.primary_tag().or_else(|| file.first_tag())?;
    tag.pictures().first().map(|p| p.data().to_vec())
}

/// "cover.jpg" / "folder.png" next to the track, used when there is no embedded art.
pub fn folder_cover(track: &Track) -> Option<Vec<u8>> {
    let Source::Local(path) = &track.source else {
        return None;
    };
    let dir = path.parent()?;
    // Per-track artwork saved next to the audio wins over a folder-wide cover.
    if let Some(own) = sidecar(path) {
        return fs::read(own).ok();
    }
    const NAMES: [&str; 4] = ["cover", "folder", "front", "album"];
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        let stem = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        let ext = p
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        if NAMES.contains(&stem.as_str()) && ["jpg", "jpeg", "png", "webp"].contains(&ext.as_str())
        {
            return fs::read(&p).ok();
        }
    }
    None
}

/// Decode any supported image, downscale and store as PNG. Returns the cached path.
pub fn store(dir: &Path, key: &str, bytes: &[u8]) -> Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let png = thumb_png(bytes)?;
    let path = path_for(dir, key);
    fs::write(&path, png)?;
    Ok(path)
}

/// Decode with safety limits and return a 512 px PNG.
pub fn thumb_png(bytes: &[u8]) -> Result<Vec<u8>> {
    // Embedded art is attacker-controlled file data; refuse absurd payloads and
    // cap the decoder so a 40 000 px picture cannot OOM the player.
    if bytes.len() > 32 * 1024 * 1024 {
        bail!("cover image is too large ({} bytes)", bytes.len());
    }
    let format = image::guess_format(bytes).context("unsupported cover image format")?;
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(128 * 1024 * 1024);
    limits.max_image_width = Some(12000);
    limits.max_image_height = Some(12000);
    reader.limits(limits);
    let img = reader.decode().context("cannot decode cover image")?;
    let thumb = img.thumbnail(512, 512);
    let mut out = std::io::Cursor::new(Vec::new());
    thumb.write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

/// `<track stem>.png/jpg/...` next to an audio file — one cover per track.
pub fn sidecar(audio: &Path) -> Option<PathBuf> {
    let stem = audio.file_stem()?.to_str()?;
    let parent = audio.parent()?;
    ["png", "jpg", "jpeg", "webp"]
        .into_iter()
        .map(|ext| parent.join(format!("{stem}.{ext}")))
        .find(|p| p.is_file())
}

/// Embed a PNG cover into the audio file's tags. Best effort: some containers
/// do not support pictures, and a failed embed must not fail the download.
pub fn embed_cover(audio: &Path, png: &[u8]) -> Result<()> {
    let mut file = lofty::read_from_path(audio)?;
    let picture = lofty::picture::Picture::unchecked(png.to_vec())
        .pic_type(lofty::picture::PictureType::CoverFront)
        .mime_type(lofty::picture::MimeType::Png)
        .description("Cover")
        .build();

    let kind = file.file_type().primary_tag_type();
    let has_primary = file.primary_tag().is_some();
    let tag = if has_primary {
        file.primary_tag_mut()
    } else {
        file.first_tag_mut()
    };
    if let Some(tag) = tag {
        if tag.pictures().is_empty() {
            tag.push_picture(picture);
        } else {
            tag.set_picture(0, picture);
        }
    } else {
        let mut tag = lofty::tag::Tag::new(kind);
        tag.push_picture(picture);
        file.insert_tag(tag);
    }
    file.save_to_path(audio, lofty::config::WriteOptions::default())?;
    Ok(())
}

/// After yt-dlp downloaded `<stem>.mp3` and `<stem>.jpg`: normalise the artwork
/// to a 512 px PNG sidecar, embed it into the audio tags and drop the original
/// (possibly huge/webp) thumbnail. Returns the sidecar path.
pub fn attach_downloaded(audio: &Path) -> Option<PathBuf> {
    let stem = audio.file_stem()?.to_str()?;
    let parent = audio.parent()?;
    let source = fs::read_dir(parent)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_stem().and_then(|s| s.to_str()) == Some(stem)
                && p.extension()
                    .and_then(|s| s.to_str())
                    .map(|e| ["jpg", "jpeg", "png", "webp"].contains(&e.to_lowercase().as_str()))
                    .unwrap_or(false)
        })?;

    let png = thumb_png(&fs::read(&source).ok()?).ok()?;
    let sidecar = parent.join(format!("{stem}.png"));
    let tmp = parent.join(format!(".{stem}.png.tmp"));
    fs::write(&tmp, &png).ok()?;
    fs::rename(&tmp, &sidecar).ok()?;
    if source != sidecar {
        let _ = fs::remove_file(&source);
    }
    let _ = embed_cover(audio, &png);
    Some(sidecar)
}

/// Download the video thumbnail with yt-dlp and convert it to PNG.
pub fn fetch_remote(bin: &str, track: &Track, dir: &Path) -> Result<PathBuf> {
    let Source::Remote { url, id } = &track.source else {
        bail!("not a remote track")
    };
    fs::create_dir_all(dir)?;
    let stem = format!("thumb-{}", crate::ytdlp::safe(id));
    let out = Command::new(bin)
        .args([
            "--skip-download",
            "--write-thumbnail",
            "--no-playlist",
            "--no-warnings",
            "-o",
        ])
        .arg(dir.join(format!("{stem}.%(ext)s")))
        .arg(url)
        .output()
        .with_context(|| format!("cannot run `{bin}`"))?;
    if !out.status.success() {
        bail!(
            "yt-dlp: {}",
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .last()
                .unwrap_or("thumbnail failed")
        );
    }
    let file = fs::read_dir(dir)?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.file_stem().and_then(|s| s.to_str()) == Some(stem.as_str()))
        .context("yt-dlp did not write a thumbnail")?;
    let bytes = fs::read(&file)?;
    let _ = fs::remove_file(&file);
    store(dir, &track.key(), &bytes)
}

/// Cached cover for `track`, if one was already produced.
pub fn cached(cache_dir: &Path, track: &Track) -> Option<PathBuf> {
    let p = path_for(&dir(cache_dir), &track.key());
    p.exists().then_some(p)
}

/// Synchronous local cover extraction (tags first, then folder art).
pub fn local(cache_dir: &Path, track: &Track) -> Option<PathBuf> {
    if !matches!(track.source, Source::Local(_)) {
        return None;
    }
    let bytes = embedded(track).or_else(|| folder_cover(track))?;
    store(&dir(cache_dir), &track.key(), &bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_paths_are_stable_and_distinct() {
        let d = Path::new("/tmp/covers");
        assert_eq!(path_for(d, "abc"), path_for(d, "abc"));
        assert_ne!(path_for(d, "abc"), path_for(d, "abd"));
    }

    #[test]
    fn sidecar_is_per_track_not_folder_wide() {
        let out = std::env::temp_dir().join(format!("rmp-sidecar-test-{}", std::process::id()));
        fs::create_dir_all(&out).unwrap();
        let audio = out.join("Artist - Song.mp3");
        fs::write(&audio, b"not really audio").unwrap();
        let art = out.join("Artist - Song.png");
        fs::write(&art, b"png").unwrap();
        fs::write(out.join("cover.png"), b"folder").unwrap();
        assert_eq!(sidecar(&audio), Some(art));
        let _ = fs::remove_dir_all(&out);
    }

    #[test]
    fn stores_a_tiny_png() {
        let out = std::env::temp_dir().join("rmp-cover-test");
        fs::create_dir_all(&out).unwrap();
        let src = out.join("src.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]))
            .save(&src)
            .unwrap();
        let bytes = fs::read(&src).unwrap();
        let path = store(&out, "unit-test", &bytes).expect("store png");
        assert!(path.exists());
        let _ = fs::remove_dir_all(&out);
    }
}
