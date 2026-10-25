//! Screen presets: where the list, visualiser, now-playing card, lyrics and footer go.
use ratatui::layout::{Constraint, Layout, Rect};

/// `auto` switches to the compact preset below these terminal sizes.
pub const AUTO_MIN_W: u16 = 100;
pub const AUTO_MIN_H: u16 = 30;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Preset {
    /// follow the window size: resonance on large windows, minimal on small ones
    Auto,
    /// tabs on top, list left, visualiser + now playing right
    Resonance,
    /// one list, a thin now-playing strip, no visualiser
    Minimal,
    /// big visualiser on top, list and a large now-playing card below
    Focus,
    /// cover and track info on the left, synced lyrics on the right, queue below
    Studio,
}

impl Preset {
    pub fn from_name(s: &str) -> Self {
        match s {
            "resonance" => Preset::Resonance,
            "minimal" => Preset::Minimal,
            "focus" => Preset::Focus,
            "studio" => Preset::Studio,
            _ => Preset::Auto,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Preset::Auto => "auto",
            Preset::Resonance => "resonance",
            Preset::Minimal => "minimal",
            Preset::Focus => "focus",
            Preset::Studio => "studio",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Preset::Auto => Preset::Resonance,
            Preset::Resonance => Preset::Minimal,
            Preset::Minimal => Preset::Focus,
            Preset::Focus => Preset::Studio,
            Preset::Studio => Preset::Auto,
        }
    }

    /// The concrete preset that is drawn for a given terminal area.
    pub fn effective(self, area: Rect) -> Self {
        match self {
            Preset::Auto => {
                if area.width < AUTO_MIN_W || area.height < AUTO_MIN_H {
                    Preset::Minimal
                } else {
                    Preset::Resonance
                }
            }
            other => other,
        }
    }
}

pub struct Regions {
    pub top: Rect,
    pub list: Rect,
    pub viz: Option<Rect>,
    pub now: Rect,
    pub lyrics: Option<Rect>,
    pub footer: Rect,
}

pub fn regions(preset: Preset, area: Rect) -> Regions {
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(8),
        Constraint::Length(1),
    ])
    .split(area);
    let (top, main, footer) = (rows[0], rows[1], rows[2]);

    match preset.effective(area) {
        Preset::Resonance | Preset::Auto => {
            let cols = Layout::horizontal([Constraint::Percentage(38), Constraint::Percentage(62)])
                .split(main);
            let right =
                Layout::vertical([Constraint::Min(6), Constraint::Length(10)]).split(cols[1]);
            Regions {
                top,
                list: cols[0],
                viz: Some(right[0]),
                now: right[1],
                lyrics: None,
                footer,
            }
        }
        Preset::Minimal => {
            let v = Layout::vertical([Constraint::Min(4), Constraint::Length(4)]).split(main);
            Regions {
                top,
                list: v[0],
                viz: None,
                now: v[1],
                lyrics: None,
                footer,
            }
        }
        Preset::Focus => {
            let v = Layout::vertical([Constraint::Min(8), Constraint::Length(12)]).split(main);
            let h = Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
                .split(v[1]);
            Regions {
                top,
                list: h[0],
                viz: Some(v[0]),
                now: h[1],
                lyrics: None,
                footer,
            }
        }
        Preset::Studio => {
            let v = Layout::vertical([Constraint::Min(8), Constraint::Length(9)]).split(main);
            let h = Layout::horizontal([Constraint::Percentage(42), Constraint::Min(10)])
                .split(v[0]);
            Regions {
                top,
                list: v[1],
                viz: None,
                now: h[0],
                lyrics: Some(h[1]),
                footer,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_picks_minimal_on_small_windows() {
        let small = Rect::new(0, 0, 80, 24);
        let big = Rect::new(0, 0, 180, 50);
        assert_eq!(Preset::Auto.effective(small), Preset::Minimal);
        assert_eq!(Preset::Auto.effective(big), Preset::Resonance);
        assert_eq!(Preset::Studio.effective(small), Preset::Studio);
    }

    #[test]
    fn cycle_visits_every_preset_once() {
        let mut p = Preset::Auto;
        let mut seen = vec![];
        for _ in 0..5 {
            seen.push(p.name());
            p = p.next();
        }
        assert_eq!(p, Preset::Auto);
        assert_eq!(seen.len(), 5);
    }
}
