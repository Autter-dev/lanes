//! The home screen `lane` opens after onboarding: your lanes, what to do next, the commands
//! (with what's built and what's coming), the settings, and keys to change them.

use crate::theme::Theme;
use lane_core::lane_env;
use lane_core::state::Lane;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Layout, Margin};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

pub struct Input {
    pub version: &'static str,
    pub telemetry: bool,
    pub telemetry_forced_off: Option<&'static str>,
    pub theme: Theme,
    /// The repo `lane` was opened in, if any.
    pub repo: Option<HomeRepo>,
    /// This repo's lanes, or every repo's when `lane` was opened outside one.
    pub lanes: Vec<Lane>,
}

pub struct HomeRepo {
    /// The repo's folder name.
    pub name: String,
    /// Whether it has a lane.toml yet.
    pub set_up: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Quit {
        telemetry: bool,
    },
    /// Run the onboarding again.
    Setup {
        telemetry: bool,
    },
}

pub struct Home {
    input: Input,
    telemetry: bool,
    notice: Option<&'static str>,
}

const COMMANDS: [(&str, &str, bool); 6] = [
    (
        "lane setup",
        "run the welcome and machine checks again",
        true,
    ),
    ("lane init", "find your stack and write lane.toml", true),
    (
        "lane new <name>",
        "make a lane: an instant, ready-to-run copy",
        true,
    ),
    ("lane list", "your lanes with their ports and folders", true),
    (
        "lane exec <name> -- <cmd>",
        "run a command inside a lane",
        true,
    ),
    (
        "lane promote <name>",
        "turn a lane into one clean pull request",
        false,
    ),
];

impl Home {
    pub fn new(input: Input) -> Home {
        let telemetry = input.telemetry && input.telemetry_forced_off.is_none();
        Home {
            input,
            telemetry,
            notice: None,
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Option<Outcome> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Some(Outcome::Quit {
                telemetry: self.telemetry,
            });
        }
        self.notice = None;
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                return Some(Outcome::Quit {
                    telemetry: self.telemetry,
                });
            }
            KeyCode::Char('s') => {
                return Some(Outcome::Setup {
                    telemetry: self.telemetry,
                });
            }
            KeyCode::Char('t') => {
                if self.input.telemetry_forced_off.is_some() {
                    self.notice =
                        Some("The install count stays off while your environment turns it off.");
                } else {
                    self.telemetry = !self.telemetry;
                    self.notice = Some(if self.telemetry {
                        "Install count on. Saved when you leave."
                    } else {
                        "Install count off. Saved when you leave."
                    });
                }
            }
            _ => {}
        }
        None
    }

    pub fn render(&self, frame: &mut Frame) {
        let th = &self.input.theme;
        let area = frame.area();
        if area.width < crate::onboarding::MIN_SIZE.0 || area.height < crate::onboarding::MIN_SIZE.1
        {
            crate::too_small(frame, th, crate::onboarding::MIN_SIZE);
            return;
        }
        let inner = area.inner(Margin {
            horizontal: 2,
            vertical: 1,
        });
        let [header, _, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(inner);

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Lanes", th.strong()),
                Span::styled(".", th.accent_strong()),
                Span::styled("   by Autter ", th.muted()),
                Span::styled("=(o.o)=", th.otter()),
            ])),
            header,
        );
        frame.render_widget(
            Paragraph::new(Line::styled(
                format!("{} · not signed in", self.input.version),
                th.muted(),
            ))
            .alignment(Alignment::Right),
            header,
        );

        let lanes = &self.input.lanes;
        let mut lines = if body.height >= 20 && lanes.is_empty() {
            let mut big = crate::widgets::wordmark(th, f64::INFINITY, 0.6);
            big.push(Line::default());
            big
        } else {
            Vec::new()
        };
        if lanes.is_empty() {
            lines.extend([
                Line::styled("No lanes yet.", th.strong()),
                Line::styled(
                    "Your lanes will show here, each with its agent, URL, changed files and tests.",
                    th.muted(),
                ),
            ]);
        } else {
            lines.push(Line::styled(
                match &self.input.repo {
                    Some(r) => format!("Lanes in {}", r.name),
                    None => "Your lanes".to_string(),
                },
                th.strong(),
            ));
            // Everything below the lanes takes 16 rows; the lanes get what's left.
            let room = (body.height as usize).saturating_sub(16).max(1);
            let shown = if lanes.len() > room {
                room - 1
            } else {
                lanes.len()
            };
            let name_w = lanes
                .iter()
                .map(|l| l.name.len())
                .max()
                .unwrap_or(4)
                .min(24);
            let urls: Vec<String> = lanes[..shown].iter().map(lane_env::url).collect();
            let url_w = urls.iter().map(String::len).max().unwrap_or(0);
            for (l, url) in lanes[..shown].iter().zip(&urls) {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {:<name_w$}  ", l.name), th.accent()),
                    Span::styled(format!("{url:<url_w$}"), th.text()),
                    Span::styled(format!("  {}", l.state()), th.muted()),
                ]));
            }
            if shown < lanes.len() {
                lines.push(Line::styled(
                    format!("  and {} more: lane list", lanes.len() - shown),
                    th.muted(),
                ));
            }
        }
        lines.push(Line::default());
        let next = match &self.input.repo {
            Some(r) if !r.set_up => Some(("This repo isn't set up yet: ", "lane init")),
            Some(_) if lanes.is_empty() => Some(("Make your first lane: ", "lane new fix-auth")),
            Some(_) => Some(("Run the app in one: ", "lane exec <name> -- <dev command>")),
            None => Some(("Go to a repo you work on: ", "cd your-project")),
        };
        if let Some((what, cmd)) = next {
            lines.push(Line::from(vec![
                Span::styled(what, th.text()),
                Span::styled(cmd, th.accent()),
            ]));
            lines.push(Line::default());
        }
        lines.push(Line::styled("Commands", th.strong()));
        for (cmd, what, built) in COMMANDS {
            let mut spans = vec![
                Span::styled(
                    format!("  {cmd:<28}"),
                    if built { th.accent() } else { th.text() },
                ),
                Span::styled(what, th.text()),
            ];
            if !built {
                spans.push(Span::styled("   coming soon", th.muted()));
            }
            lines.push(Line::from(spans));
        }
        lines.push(Line::default());
        lines.push(Line::styled("Settings", th.strong()));
        let tele = match (self.input.telemetry_forced_off, self.telemetry) {
            (Some(why), _) => format!("off ({why})"),
            (None, true) => "on".into(),
            (None, false) => "off".into(),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<28}", "Install count"), th.text()),
            Span::styled(tele, th.text()),
        ]));
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<28}", "Signed in"), th.text()),
            Span::styled("no", th.text()),
            Span::styled("   sign-in opens with the gateway", th.muted()),
        ]));
        frame.render_widget(Paragraph::new(lines), body);

        let keys = match self.notice {
            Some(n) => Line::styled(n, th.accent()),
            None => Line::from(vec![
                Span::styled("s", th.key()),
                Span::styled(" setup again  ·  ", th.muted()),
                Span::styled("t", th.key()),
                Span::styled(" switch install count  ·  ", th.muted()),
                Span::styled("q", th.key()),
                Span::styled(" quit", th.muted()),
            ]),
        };
        frame.render_widget(Paragraph::new(keys), footer);
    }
}
