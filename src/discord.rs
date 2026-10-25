//! Discord Rich Presence over the local IPC socket.
//!
//! We speak the (tiny) Discord IPC protocol directly instead of going through a
//! wrapper crate for three reasons:
//! * the activity payload can carry `name`, which Discord shows as the activity
//!   title line — this is what makes the card say the track name instead of the
//!   application name;
//! * socket reads/writes get timeouts, so a hung Discord can never freeze the
//!   player (the previous client could block forever in the handshake read);
//! * reconnect handling is fully under our control.
use crate::{
    config::Config,
    track::{Source, Track},
};
use serde_json::{json, Value};
use std::{
    env,
    io::{ErrorKind, Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const ENV_KEYS: [&str; 4] = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"];
const APP_SUBPATHS: [&str; 4] = [
    "",
    "app/com.discordapp.Discord/",
    "snap.discord-canary/",
    "snap.discord/",
];
/// Discord only waits a fraction of a second; never block the UI thread longer.
const IO_TIMEOUT: Duration = Duration::from_millis(600);

pub struct Rpc {
    app_id: String,
    image: String,
    small: String,
    buttons: bool,
    client: Option<UnixStream>,
    /// Last activity sent, to avoid spamming Discord with identical payloads.
    last: Option<String>,
    last_try: Instant,
    last_error: Option<String>,
    nonce: u64,
}

/// Discord rejects strings shorter than 2 chars and longer than 128.
fn pad(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() < 2 {
        if s.is_empty() {
            "··".to_string()
        } else {
            format!("{s}·")
        }
    } else {
        s.chars().take(128).collect()
    }
}

fn socket_paths() -> Vec<PathBuf> {
    let base = ENV_KEYS
        .iter()
        .find_map(|k| env::var_os(k))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let mut paths = Vec::new();
    for sub in APP_SUBPATHS {
        for i in 0..10 {
            paths.push(base.join(sub).join(format!("discord-ipc-{i}")));
        }
    }
    paths
}

fn pack(opcode: u32, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + data.len());
    out.extend_from_slice(&opcode.to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    out
}

/// Read one full frame. Times out instead of hanging the caller.
fn read_frame(sock: &mut UnixStream) -> std::io::Result<Value> {
    let mut header = [0u8; 8];
    sock.read_exact(&mut header)?;
    let len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
    if len > 1 << 20 {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            "oversized IPC frame",
        ));
    }
    let mut body = vec![0u8; len];
    sock.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e.to_string()))
}

fn connect_socket() -> std::io::Result<UnixStream> {
    let mut last = None;
    for path in socket_paths() {
        match UnixStream::connect(&path) {
            Ok(s) => {
                s.set_read_timeout(Some(IO_TIMEOUT))?;
                s.set_write_timeout(Some(IO_TIMEOUT))?;
                return Ok(s);
            }
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| {
        std::io::Error::new(ErrorKind::NotFound, "Discord IPC socket not found")
    }))
}

impl Rpc {
    pub fn new(cfg: &Config) -> Self {
        Self {
            app_id: cfg.discord_app_id.clone(),
            image: cfg.discord_image_key.clone(),
            small: cfg.discord_small_key.clone(),
            buttons: cfg.discord_buttons,
            client: None,
            last: None,
            last_try: Instant::now() - Duration::from_secs(60),
            last_error: None,
            nonce: 1,
        }
    }

    pub fn connected(&self) -> bool {
        self.client.is_some()
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    fn send(&mut self, opcode: u32, data: &Value) -> std::io::Result<()> {
        let text = data.to_string();
        let sock = self.client.as_mut().ok_or_else(|| {
            std::io::Error::new(ErrorKind::NotConnected, "not connected to Discord")
        })?;
        sock.write_all(&pack(opcode, text.as_bytes()))?;
        sock.flush()
    }

    fn drop_client(&mut self) {
        if let Some(s) = self.client.take() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
        // Force an immediate reconnect attempt on the next update.
        self.last = None;
        self.last_try = Instant::now() - Duration::from_secs(60);
    }

    fn ensure(&mut self) -> bool {
        if self.app_id.is_empty() {
            return false;
        }
        if self.client.is_some() {
            return true;
        }
        // Retry at most every 10 seconds; Discord may simply not be running.
        if self.last_try.elapsed() < Duration::from_secs(10) {
            return false;
        }
        self.last_try = Instant::now();

        let mut sock = match connect_socket() {
            Ok(s) => s,
            Err(e) => {
                self.last_error = Some(e.to_string());
                return false;
            }
        };

        let hello = json!({ "v": 1, "client_id": self.app_id });
        if let Err(e) = sock.write_all(&pack(0, hello.to_string().as_bytes())) {
            self.last_error = Some(e.to_string());
            return false;
        }
        let _ = sock.flush();
        match read_frame(&mut sock) {
            Ok(v) => {
                if let Some(msg) = v.get("message").and_then(Value::as_str) {
                    // e.g. invalid client id
                    self.last_error = Some(msg.to_string());
                    return false;
                }
            }
            Err(e) => {
                self.last_error = Some(e.to_string());
                return false;
            }
        }

        self.client = Some(sock);
        self.last_error = None;
        // The socket is fresh: whatever we sent before is gone with the old
        // connection, so the next update must be transmitted again.
        self.last = None;
        true
    }

    pub fn update(&mut self, t: &Track, pos: Duration, paused: bool) {
        if !self.ensure() {
            return;
        }

        let title = pad(&t.title);
        let artist = if t.artist.is_empty() {
            "unknown artist".to_string()
        } else {
            t.artist.clone()
        };
        let mut state_text = match t.album.as_deref().filter(|a| !a.is_empty()) {
            Some(album) => format!("{artist} · {album}"),
            None => artist,
        };
        if paused {
            state_text.push_str(" · paused");
        }
        let state = pad(&state_text);

        // The de-duplication key only needs the track and a coarse position:
        // Discord renders the elapsed time from the timestamps itself.
        let key = format!(
            "{}|{}|{}|{}|{}",
            title,
            state,
            paused,
            pos.as_secs() / 5,
            t.duration.unwrap_or(0)
        );
        if self.last.as_deref() == Some(key.as_str()) {
            return;
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let mut activity = serde_json::Map::new();
        // `name` is the bold title line of the card. Discord accepts it over IPC
        // even though it is not exposed by the legacy Rust wrapper.
        activity.insert("name".into(), json!(title));
        activity.insert("type".into(), json!(2)); // Listening
        activity.insert("details".into(), json!(title));
        activity.insert("state".into(), json!(state));
        activity.insert("instance".into(), json!(false));

        if !paused {
            let start = now - pos.as_secs() as i64;
            let mut ts = serde_json::Map::new();
            ts.insert("start".into(), json!(start));
            if let Some(d) = t.duration {
                ts.insert("end".into(), json!(start + d as i64));
            }
            activity.insert("timestamps".into(), Value::Object(ts));
        }

        let mut assets = serde_json::Map::new();
        // Prefer the track's own artwork. Discord accepts an external https URL
        // for `large_image`, which is the only way to show a per-track cover
        // without uploading an asset to the application beforehand.
        let artwork = t
            .thumb
            .as_deref()
            .filter(|u| u.starts_with("https://") || u.starts_with("http://"))
            .map(str::to_owned)
            .or_else(|| (!self.image.is_empty()).then(|| self.image.clone()));
        if let Some(art) = artwork {
            assets.insert("large_image".into(), json!(art));
            assets.insert("large_text".into(), json!(title));
        }
        if paused && !self.small.is_empty() {
            assets.insert("small_image".into(), json!(self.small));
            assets.insert("small_text".into(), json!("paused"));
        }
        if !assets.is_empty() {
            activity.insert("assets".into(), Value::Object(assets));
        }

        if self.buttons {
            if let Source::Remote { url, .. } = &t.source {
                if url.starts_with("https://") {
                    activity.insert(
                        "buttons".into(),
                        json!([{ "label": "Open track", "url": url }]),
                    );
                }
            }
        }

        let nonce = self.nonce.to_string();
        self.nonce += 1;
        let payload = json!({
            "cmd": "SET_ACTIVITY",
            "args": { "pid": std::process::id(), "activity": activity },
            "nonce": nonce,
        });

        if let Err(e) = self.send(1, &payload) {
            self.last_error = Some(e.to_string());
            self.drop_client();
            return;
        }

        // Discord answers every command; an error means the activity was rejected.
        if let Some(sock) = self.client.as_mut() {
            match read_frame(sock) {
                Ok(v) => {
                    if let Some(msg) = v.get("message").and_then(Value::as_str) {
                        self.last_error = Some(msg.to_string());
                        self.drop_client();
                        return;
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
                    // No reply in time — keep the connection, the activity was
                    // probably still accepted. Do not block the player either way.
                }
                Err(e) => {
                    self.last_error = Some(e.to_string());
                    self.drop_client();
                    return;
                }
            }
        }

        self.last = Some(key);
        self.last_error = None;
    }

    pub fn clear(&mut self) {
        self.last = None;
        if self.client.is_none() {
            return;
        }
        let payload = json!({
            "cmd": "SET_ACTIVITY",
            "args": { "pid": std::process::id(), "activity": Value::Null },
            "nonce": format!("{}", self.nonce),
        });
        self.nonce += 1;
        let _ = self.send(1, &payload);
        if let Some(sock) = self.client.as_mut() {
            let _ = read_frame(sock);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::pad;

    #[test]
    fn pad_never_returns_below_two_chars() {
        assert_eq!(pad(""), "··");
        assert_eq!(pad("a"), "a·");
        assert_eq!(pad("ab"), "ab");
    }

    #[test]
    fn pad_truncates_long_text() {
        let long = "x".repeat(300);
        assert_eq!(pad(&long).chars().count(), 128);
    }

    #[test]
    fn frame_header_is_little_endian_opcode_plus_length() {
        let frame = super::pack(1, b"hi");
        assert_eq!(&frame[..4], &1u32.to_le_bytes());
        assert_eq!(&frame[4..8], &2u32.to_le_bytes());
        assert_eq!(&frame[8..], b"hi");
    }
}
