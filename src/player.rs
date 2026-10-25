use anyhow::{Context, Result};
use rodio::{source::SeekError, Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use std::{
    collections::VecDeque,
    fs::File,
    io::BufReader,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

pub type SampleBuf = Arc<Mutex<VecDeque<f32>>>;

pub struct Player {
    _stream: OutputStream,
    handle: OutputStreamHandle,
    sink: Sink,
    pub volume: f32,
    /// Latest mono samples, tapped from the audio thread for the visualizer.
    pub samples: SampleBuf,
    pub duration: Option<Duration>,
    /// e.g. "mp3 44.1k 2ch 182kbps"
    pub info: String,
    active: bool,
}

impl Player {
    pub fn new(volume: f32) -> Result<Self> {
        let (stream, handle) = OutputStream::try_default().context("no audio output device")?;
        let sink = Sink::try_new(&handle)?;
        sink.set_volume(volume);
        Ok(Self {
            _stream: stream,
            handle,
            sink,
            volume,
            samples: Arc::new(Mutex::new(VecDeque::new())),
            duration: None,
            info: String::new(),
            active: false,
        })
    }

    pub fn play(&mut self, path: &Path) -> Result<()> {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        let dec = Decoder::new(BufReader::new(file)).context("unsupported or broken audio file")?;
        self.sink.stop();
        self.sink = Sink::try_new(&self.handle)?;
        self.sink.set_volume(self.volume);
        self.duration = dec.total_duration();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("?")
            .to_lowercase();
        let kbps = self
            .duration
            .filter(|d| d.as_secs() > 0)
            .map(|d| format!(" {}kbps", size * 8 / d.as_secs() / 1000))
            .unwrap_or_default();
        self.info = format!(
            "{ext} {:.1}k {}ch{kbps}",
            dec.sample_rate() as f32 / 1000.0,
            dec.channels()
        );
        if let Ok(mut b) = self.samples.lock() {
            b.clear();
        }
        self.sink
            .append(Tap::new(dec.convert_samples::<f32>(), self.samples.clone()));
        self.active = true;
        Ok(())
    }

    pub fn toggle_pause(&mut self) {
        if self.sink.is_paused() {
            self.sink.play();
        } else {
            self.sink.pause();
        }
    }

    pub fn is_paused(&self) -> bool {
        self.sink.is_paused()
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Stop playback and drop the current source (keeps the audio device open).
    pub fn stop(&mut self) {
        self.sink.stop();
        self.active = false;
        self.duration = None;
        self.info.clear();
        if let Ok(mut b) = self.samples.lock() {
            b.clear();
        }
    }

    pub fn is_playing(&self) -> bool {
        self.active && !self.sink.is_paused() && !self.sink.empty()
    }

    /// True exactly once when the current track has run out.
    pub fn poll_finished(&mut self) -> bool {
        if self.active && self.sink.empty() {
            self.active = false;
            return true;
        }
        false
    }

    pub fn position(&self) -> Duration {
        self.sink.get_pos()
    }

    pub fn seek(&mut self, delta_secs: i64) {
        if !self.active {
            return;
        }
        let cur = self.position().as_secs() as i64;
        let mut target = (cur + delta_secs).max(0) as u64;
        if let Some(d) = self.duration {
            target = target.min(d.as_secs().saturating_sub(1));
        }
        let _ = self.sink.try_seek(Duration::from_secs(target));
    }

    pub fn seek_to(&mut self, secs: u64) {
        if !self.active {
            return;
        }
        let mut t = secs;
        if let Some(d) = self.duration {
            t = t.min(d.as_secs().saturating_sub(1));
        }
        let _ = self.sink.try_seek(Duration::from_secs(t));
    }

    pub fn change_volume(&mut self, delta: f32) {
        self.volume = (self.volume + delta).clamp(0.0, 1.5);
        self.sink.set_volume(self.volume);
    }
}

/// Pass-through source that copies the first channel into a shared buffer.
struct Tap<S: Source<Item = f32>> {
    inner: S,
    buf: SampleBuf,
    batch: Vec<f32>,
    ch: usize,
    i: usize,
}

impl<S: Source<Item = f32>> Tap<S> {
    fn new(inner: S, buf: SampleBuf) -> Self {
        let ch = inner.channels().max(1) as usize;
        Self {
            inner,
            buf,
            batch: Vec::with_capacity(256),
            ch,
            i: 0,
        }
    }
}

impl<S: Source<Item = f32>> Iterator for Tap<S> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        if self.i == 0 {
            self.batch.push(s);
            if self.batch.len() >= 256 {
                // never block the audio thread
                if let Ok(mut b) = self.buf.try_lock() {
                    b.extend(self.batch.drain(..));
                    while b.len() > 4096 {
                        b.pop_front();
                    }
                } else {
                    self.batch.clear();
                }
            }
        }
        self.i = (self.i + 1) % self.ch;
        Some(s)
    }
}

impl<S: Source<Item = f32>> Source for Tap<S> {
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }
    fn channels(&self) -> u16 {
        self.inner.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.i = 0;
        self.batch.clear();
        self.inner.try_seek(pos)
    }
}
