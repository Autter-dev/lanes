//! Colours for the terminal app. The terminal's own background and text colour are kept, so the
//! app reads in light and dark terminals; only accents are set. `NO_COLOR` drops colour entirely
//! (every state still has a symbol); 256-colour terminals get close indexed matches.

use lane_core::machine::Status;
use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub color: bool,
    accent: Color,
    warn: Color,
    fail: Color,
    muted: Color,
    otter: Color,
    water: Color,
    hat: Color,
}

impl Theme {
    pub fn new(no_color: bool, truecolor: bool) -> Theme {
        if truecolor {
            Theme {
                color: !no_color,
                accent: Color::Rgb(15, 185, 143),
                warn: Color::Rgb(224, 169, 58),
                fail: Color::Rgb(230, 92, 72),
                muted: Color::Rgb(128, 128, 120),
                otter: Color::Rgb(181, 128, 84),
                water: Color::Rgb(86, 156, 214),
                hat: Color::Rgb(78, 112, 196),
            }
        } else {
            Theme {
                color: !no_color,
                accent: Color::Indexed(36),
                warn: Color::Indexed(178),
                fail: Color::Indexed(167),
                muted: Color::DarkGray,
                otter: Color::Indexed(137),
                water: Color::Indexed(68),
                hat: Color::Indexed(61),
            }
        }
    }

    fn fg(&self, c: Color) -> Style {
        if self.color {
            Style::new().fg(c)
        } else {
            Style::new()
        }
    }

    pub fn text(&self) -> Style {
        Style::new()
    }
    pub fn strong(&self) -> Style {
        Style::new().add_modifier(Modifier::BOLD)
    }
    pub fn accent(&self) -> Style {
        self.fg(self.accent)
    }
    pub fn accent_strong(&self) -> Style {
        self.fg(self.accent).add_modifier(Modifier::BOLD)
    }
    pub fn muted(&self) -> Style {
        if self.color {
            self.fg(self.muted)
        } else {
            Style::new().add_modifier(Modifier::DIM)
        }
    }
    pub fn warn(&self) -> Style {
        self.fg(self.warn)
    }
    pub fn fail(&self) -> Style {
        self.fg(self.fail)
    }
    pub fn key(&self) -> Style {
        Style::new().add_modifier(Modifier::BOLD)
    }
    pub fn disabled(&self) -> Style {
        self.muted().add_modifier(Modifier::DIM)
    }

    /// Autter's otter, its water and its captain's hat.
    pub fn otter(&self) -> Style {
        self.fg(self.otter).add_modifier(Modifier::BOLD)
    }
    pub fn water(&self) -> Style {
        self.fg(self.water)
    }
    pub fn hat(&self) -> Style {
        self.fg(self.hat).add_modifier(Modifier::BOLD)
    }

    pub fn status(&self, s: Status) -> Style {
        match s {
            Status::Ok => self.accent(),
            Status::Info => self.muted(),
            Status::Warn => self.warn(),
            Status::Fail => self.fail(),
        }
    }
}
