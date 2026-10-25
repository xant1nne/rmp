use crate::player::SampleBuf;
use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::{f32::consts::PI, sync::Arc};

const N: usize = 1024;
pub const BARS: usize = 64;
pub const WAVE: usize = 512;
/// Samples shown by the oscilloscope (~23 ms at 44.1 kHz: a few visible cycles).
const WINDOW: usize = 1024;
/// How far to look for a rising zero crossing that locks the waveform in phase.
const TRIGGER: usize = 1024;

#[derive(Clone, Copy, PartialEq)]
pub enum VizMode {
    Wave,
    Bars,
    Mirror,
    Spectrum,
}

impl VizMode {
    pub fn name(self) -> &'static str {
        match self {
            VizMode::Wave => "wave",
            VizMode::Bars => "bars",
            VizMode::Mirror => "mirror",
            VizMode::Spectrum => "spectrum",
        }
    }
    pub fn next(self) -> Self {
        match self {
            VizMode::Wave => VizMode::Bars,
            VizMode::Bars => VizMode::Mirror,
            VizMode::Mirror => VizMode::Spectrum,
            VizMode::Spectrum => VizMode::Wave,
        }
    }
    pub fn from_name(s: &str) -> Self {
        match s {
            "bars" => VizMode::Bars,
            "mirror" => VizMode::Mirror,
            "spectrum" => VizMode::Spectrum,
            _ => VizMode::Wave,
        }
    }
}

pub struct Visualizer {
    fft: Arc<dyn Fft<f32>>,
    win: Vec<f32>,
    /// 0.0..=1.0 per bar, log-spaced frequency bands
    pub bars: Vec<f32>,
    /// -1.0..=1.0 phase-locked oscilloscope line and the previous frame
    pub wave: Vec<f32>,
    pub ghost: Vec<f32>,
    /// Peak follower for automatic gain, so quiet and loud tracks look similar.
    peak: f32,
    gain: f32,
}

impl Visualizer {
    pub fn new() -> Self {
        let fft = FftPlanner::<f32>::new().plan_fft_forward(N);
        let win = (0..N)
            .map(|i| 0.5 * (1.0 - (2.0 * PI * i as f32 / (N as f32 - 1.0)).cos()))
            .collect();
        Self {
            fft,
            win,
            bars: vec![0.0; BARS],
            wave: vec![0.0; WAVE],
            ghost: vec![0.0; WAVE],
            peak: 0.05,
            gain: 1.0,
        }
    }

    pub fn update(&mut self, samples: &SampleBuf, playing: bool) {
        let all: Option<Vec<f32>> = if playing {
            samples.try_lock().ok().and_then(|b| {
                if b.len() >= WINDOW {
                    Some(b.iter().copied().collect())
                } else {
                    None
                }
            })
        } else {
            None
        };

        let Some(all) = all else {
            for b in &mut self.bars {
                *b *= 0.85;
            }
            self.ghost.clone_from(&self.wave);
            for w in &mut self.wave {
                *w *= 0.85;
            }
            return;
        };

        self.ghost.clone_from(&self.wave);

        // Oscilloscope with a rising zero-crossing trigger: without it the window slides
        // freely through the audio and the line visibly shakes.
        let len = all.len();
        let latest = len - WINDOW;
        // Prefer the newest crossing, so what is drawn is what was just heard.
        let search_start = latest.saturating_sub(TRIGGER);
        let mut start = latest;
        for i in (search_start..latest).rev() {
            if all[i] <= 0.0 && all[i + 1] > 0.0 {
                start = i;
                break;
            }
        }
        let window = &all[start..start + WINDOW];

        // Peak-following AGC: instant attack, slow release, so the line keeps a usable height.
        // Below the noise floor the line fades out instead of amplifying hiss.
        let peak = window.iter().fold(0.0_f32, |m, v| m.max(v.abs()));
        if peak < 0.002 {
            self.peak = (self.peak * 0.9).max(0.003);
            for w in &mut self.wave {
                *w *= 0.8;
            }
        } else {
            self.peak = (self.peak * 0.93).max(peak).max(0.003);
            let target_gain = (0.9 / self.peak).clamp(0.5, 24.0);
            self.gain = self.gain * 0.75 + target_gain * 0.25;

            let step = WINDOW as f32 / WAVE as f32;
            for i in 0..WAVE {
                let idx = (i as f32 * step) as usize;
                let sample = window[idx.min(WINDOW - 1)] * self.gain;
                let target = sample.clamp(-1.0, 1.0);
                self.wave[i] = self.wave[i] * 0.3 + target * 0.7;
            }
        }

        // spectrum on the newest N samples
        let mut buf: Vec<Complex<f32>> = all[len - N..]
            .iter()
            .zip(&self.win)
            .map(|(s, w)| Complex::new(s * w, 0.0))
            .collect();
        self.fft.process(&mut buf);
        let half = N / 2;
        for b in 0..BARS {
            let lo = ((half as f32).powf(b as f32 / BARS as f32) as usize).max(1);
            let hi = ((half as f32).powf((b + 1) as f32 / BARS as f32) as usize)
                .max(lo + 1)
                .min(half);
            let mag = buf[lo..hi].iter().map(|c| c.norm()).fold(0.0_f32, f32::max);
            let db = 20.0 * (mag / (N as f32 / 4.0) + 1e-6).log10();
            let v = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
            self.bars[b] = v.max(self.bars[b] * 0.8); // fast attack, smooth fall
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    fn feed(buf: &SampleBuf, phase: &mut f32, n: usize) {
        let mut b = buf.lock().unwrap();
        for _ in 0..n {
            // 440 Hz at 44.1 kHz
            *phase += 2.0 * PI * 440.0 / 44100.0;
            b.push_back(phase.sin() * 0.4);
        }
        while b.len() > 4096 {
            b.pop_front();
        }
    }

    #[test]
    fn wave_follows_the_signal_instead_of_averaging_it_away() {
        let buf: SampleBuf = Arc::new(Mutex::new(VecDeque::new()));
        let mut viz = Visualizer::new();
        let mut phase = 0.0f32;
        feed(&buf, &mut phase, 4096);
        for _ in 0..20 {
            viz.update(&buf, true);
        }
        let peak = viz.wave.iter().fold(0.0_f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.5, "wave should use most of the height, got {peak}");

        let crossings = viz
            .wave
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        // 440 Hz over ~23 ms => roughly 10 rising crossings.
        assert!(
            (6..=16).contains(&crossings),
            "unexpected crossing count: {crossings}"
        );
    }

    #[test]
    fn window_is_phase_locked_between_frames() {
        let buf: SampleBuf = Arc::new(Mutex::new(VecDeque::new()));
        let mut viz = Visualizer::new();
        let mut phase = 0.0f32;
        feed(&buf, &mut phase, 4096);
        for _ in 0..10 {
            viz.update(&buf, true);
        }
        let first = viz.wave.clone();
        feed(&buf, &mut phase, 256); // same as one audio callback
        viz.update(&buf, true);
        let diff: f32 = first
            .iter()
            .zip(&viz.wave)
            .map(|(a, b)| (a - b).abs())
            .sum::<f32>()
            / WAVE as f32;
        assert!(
            diff < 0.15,
            "waveform jumps between frames: mean diff {diff}"
        );
    }

    #[test]
    fn silence_decays_instead_of_amplifying_noise() {
        let buf: SampleBuf = Arc::new(Mutex::new(VecDeque::new()));
        let mut viz = Visualizer::new();
        {
            let mut b = buf.lock().unwrap();
            b.extend((0..4096).map(|i| (i as f32).sin() * 1e-5));
        }
        for _ in 0..10 {
            viz.update(&buf, true);
        }
        let peak = viz.wave.iter().fold(0.0_f32, |m, v| m.max(v.abs()));
        assert!(peak < 0.05, "hiss should stay near the axis, got {peak}");
    }
}
