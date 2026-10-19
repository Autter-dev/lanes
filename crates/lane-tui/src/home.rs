//! The home screen `lane` opens after onboarding: every lane with its agent, status, URL,
//! changed files, tests and extra disk, kept live; what lanes saved; a status bar; and keys,
//! with a help overlay (`?`) and a command palette (`:` or Ctrl-K). With no lanes yet, it says
//! how to get the first one.

use crate::feed::{Row, Snapshot, Source};
use crate::theme::Theme;
use lane_core::insight;
use lane_core::lane_env;
use lane_core::machine::Check;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Clear, Paragraph, Row as TableRow, Table, TableState, Wrap};
use std::path::PathBuf;

pub struct Input {
    pub version: &'static str,
    pub telemetry: bool,
    pub telemetry_forced_off: Option<&'static str>,
    pub theme: Theme,
    /// The repo `lane` was opened in, if any.
    pub repo: Option<HomeRepo>,
    /// The lanes, as the watcher last saw them.
    pub snapshot: Snapshot,
    /// Agents installed here that aren't in lanes yet, when the setting is on.
    pub agents_unset: Vec<&'static str>,
}

pub struct HomeRepo {
    /// The repo's folder name.
    pub name: String,
    /// Whether it has a lane.toml yet.
    pub set_up: bool,
    pub root: PathBuf,
}

/// How the home screen ends.
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

/// What a key asks the caller to do: leave, or change lanes (which runs in the background).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Exit(Outcome),
    New(String),
    Remove(String),
}

/// What sits over the screen, if anything.
#[derive(Debug, Clone, PartialEq)]
enum Overlay {
    None,
    Help,
    Palette {
        query: String,
        selected: usize,
    },
    /// Typing a new lane's name.
    Name(String),
    /// Removing a lane: y or n.
    Confirm(String),
    Logs(Vec<String>),
    Doctor(Vec<Check>),
}

/// A command in the palette: its label, the key that does the same, and what it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    New,
    Remove,
    Open,
    Copy,
    Logs,
    Doctor,
    Setup,
    Telemetry,
    Help,
    Quit,
}

const COMMANDS: [(Command, &str, &str); 10] = [
    (Command::New, "n", "Make a new lane…"),
    (Command::Remove, "d", "Remove the selected lane…"),
    (
        Command::Open,
        "o",
        "Open the selected lane's app in the browser",
    ),
    (Command::Copy, "c", "Copy the selected lane's folder"),
    (Command::Logs, "l", "Show what Lanes did (lane logs)"),
    (
        Command::Doctor,
        "",
        "Check what could stop lanes working (lane doctor)",
    ),
    (
        Command::Setup,
        "s",
        "Run the welcome and machine checks again",
    ),
    (
        Command::Telemetry,
        "t",
        "Switch the install count on or off",
    ),
    (Command::Help, "?", "Show the keys"),
    (Command::Quit, "q", "Quit"),
];

/// Commands shown when there are no lanes yet.
const START: [(&str, &str); 5] = [
    ("lane init", "set up this repo, with a Y/n before each step"),
    (
        "lane new <name>",
        "make a lane: an instant, ready-to-run copy",
    ),
    ("claude -w <name>", "start Claude Code in a lane of its own"),
    ("lane doctor", "check what could stop lanes working"),
    ("lane logs", "see what Lanes did"),
];

pub struct Home {
    input: Input,
    telemetry: bool,
    selected: usize,
    overlay: Overlay,
    notice: Option<String>,
    busy: Option<String>,
}

impl Home {
    pub fn new(input: Input) -> Home {
        let telemetry = input.telemetry && input.telemetry_forced_off.is_none();
        Home {
            input,
            telemetry,
            selected: 0,
            overlay: Overlay::None,
            notice: None,
            busy: None,
        }
    }

    /// A new snapshot from the watcher. True when the screen needs drawing again.
    pub fn update(&mut self, snapshot: Snapshot) -> bool {
        if snapshot == self.input.snapshot {
            return false;
        }
        self.input.snapshot = snapshot;
        let n = self.input.snapshot.rows.len();
        if self.selected >= n {
            self.selected = n.saturating_sub(1);
        }
        true
    }

    /// Whether a lane is being made or removed in the background.
    pub fn busy(&self) -> bool {
        self.busy.is_some()
    }

    /// The lane being made or removed is done: say how it went.
    pub fn done(&mut self, message: impl Into<String>) {
        self.busy = None;
        self.notice = Some(message.into());
    }

    fn selected_row(&self) -> Option<&Row> {
        self.input.snapshot.rows.get(self.selected)
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Option<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            return Some(self.exit_quit());
        }
        match std::mem::replace(&mut self.overlay, Overlay::None) {
            Overlay::None => {}
            Overlay::Help | Overlay::Logs(_) | Overlay::Doctor(_) => return None,
            Overlay::Palette {
                mut query,
                mut selected,
            } => {
                let matches = palette_matches(&query);
                match key.code {
                    KeyCode::Esc => {}
                    KeyCode::Enter => {
                        if let Some((cmd, _, _)) = matches.get(selected) {
                            return self.run(*cmd);
                        }
                    }
                    KeyCode::Up => {
                        selected = selected.saturating_sub(1);
                        self.overlay = Overlay::Palette { query, selected };
                    }
                    KeyCode::Down => {
                        selected = (selected + 1).min(matches.len().saturating_sub(1));
                        self.overlay = Overlay::Palette { query, selected };
                    }
                    KeyCode::Backspace => {
                        query.pop();
                        self.overlay = Overlay::Palette { query, selected: 0 };
                    }
                    KeyCode::Char(c) if !ctrl => {
                        query.push(c);
                        self.overlay = Overlay::Palette { query, selected: 0 };
                    }
                    _ => self.overlay = Overlay::Palette { query, selected },
                }
                return None;
            }
            Overlay::Name(mut name) => {
                match key.code {
                    KeyCode::Esc => {}
                    KeyCode::Enter => {
                        let name = name.trim_matches('-').to_string();
                        if lane_core::state::check_name(&name).is_ok() {
                            self.busy = Some(format!("Making {name}"));
                            return Some(Action::New(name));
                        }
                        self.notice = Some(
                            "Use lowercase letters, digits and hyphens, like fix-auth.".into(),
                        );
                        self.overlay = Overlay::Name(name);
                    }
                    KeyCode::Backspace => {
                        name.pop();
                        self.overlay = Overlay::Name(name);
                    }
                    KeyCode::Char(c) if !ctrl && name.len() < 40 => {
                        let c = c.to_ascii_lowercase();
                        if c.is_ascii_alphanumeric() {
                            name.push(c);
                        } else if (c == '-' || c == ' ' || c == '_') && !name.ends_with('-') {
                            name.push('-');
                        }
                        self.overlay = Overlay::Name(name);
                    }
                    _ => self.overlay = Overlay::Name(name),
                }
                return None;
            }
            Overlay::Confirm(name) => {
                if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                    self.busy = Some(format!("Removing {name}"));
                    return Some(Action::Remove(name));
                }
                return None;
            }
        }
        self.notice = None;
        if ctrl {
            return match key.code {
                KeyCode::Char('k') => self.run_palette(),
                _ => None,
            };
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let n = self.input.snapshot.rows.len();
                self.selected = (self.selected + 1).min(n.saturating_sub(1));
                None
            }
            KeyCode::Char(':') => self.run_palette(),
            KeyCode::Char('?') => self.run(Command::Help),
            KeyCode::Char('n') => self.run(Command::New),
            KeyCode::Char('d') | KeyCode::Delete => self.run(Command::Remove),
            KeyCode::Char('o') => self.run(Command::Open),
            KeyCode::Char('c') => self.run(Command::Copy),
            KeyCode::Char('l') => self.run(Command::Logs),
            KeyCode::Char('s') => self.run(Command::Setup),
            KeyCode::Char('t') => self.run(Command::Telemetry),
            KeyCode::Char('q') | KeyCode::Esc => self.run(Command::Quit),
            _ => None,
        }
    }

    fn run_palette(&mut self) -> Option<Action> {
        self.overlay = Overlay::Palette {
            query: String::new(),
            selected: 0,
        };
        None
    }

    fn exit_quit(&self) -> Action {
        Action::Exit(Outcome::Quit {
            telemetry: self.telemetry,
        })
    }

    fn run(&mut self, cmd: Command) -> Option<Action> {
        match cmd {
            Command::Quit if self.busy.is_some() => {
                self.notice = Some(
                    "Lanes is still at it. Quit once it's done, or Ctrl-C to leave now.".into(),
                );
            }
            Command::Quit => return Some(self.exit_quit()),
            Command::Setup => {
                return Some(Action::Exit(Outcome::Setup {
                    telemetry: self.telemetry,
                }));
            }
            Command::Help => self.overlay = Overlay::Help,
            Command::New => {
                if self.busy.is_some() {
                    self.notice = Some("Wait for the lane being made or removed to finish.".into());
                } else if self.input.repo.is_none() {
                    self.notice = Some("Open Lanes in a repo to make a lane there.".into());
                } else {
                    self.overlay = Overlay::Name(String::new());
                }
            }
            Command::Remove => match self.selected_row().map(|r| r.lane.name.clone()) {
                Some(_) if self.busy.is_some() => {
                    self.notice = Some("Wait for the lane being made or removed to finish.".into())
                }
                Some(name) => self.overlay = Overlay::Confirm(name),
                None => self.notice = Some("No lane to remove.".into()),
            },
            Command::Open => match self.selected_row() {
                Some(r) => {
                    let url = lane_env::url(&r.lane);
                    self.notice = Some(match crate::actions::open_url(&url) {
                        true => format!("Opened {url}"),
                        false => format!("Open {url} in your browser"),
                    });
                }
                None => self.notice = Some("No lane to open.".into()),
            },
            Command::Copy => match self.selected_row() {
                Some(r) => {
                    let path = r.lane.path.display().to_string();
                    self.notice = Some(match crate::actions::copy(&path) {
                        true => format!("Copied {path}"),
                        false => format!("Couldn't reach the clipboard. The folder is {path}"),
                    });
                }
                None => self.notice = Some("No lane to copy.".into()),
            },
            Command::Logs => {
                let lines = lane_core::log::read(40, None)
                    .unwrap_or_default()
                    .iter()
                    .map(|e| {
                        let mark = match e.level {
                            lane_core::log::Level::Info => " ",
                            lane_core::log::Level::Warn => "!",
                            lane_core::log::Level::Error => "✗",
                        };
                        let lane = e
                            .lane
                            .as_deref()
                            .map(|l| format!("{l}: "))
                            .unwrap_or_default();
                        format!(
                            "{mark} {}  {}  {lane}{}",
                            e.at.replace('T', " ").trim_end_matches('Z'),
                            e.event,
                            e.message
                        )
                    })
                    .collect();
                self.overlay = Overlay::Logs(lines);
            }
            Command::Doctor => {
                let dir = self
                    .input
                    .repo
                    .as_ref()
                    .map(|r| r.root.clone())
                    .unwrap_or_else(|| PathBuf::from("."));
                self.overlay = Overlay::Doctor(lane_core::doctor::run(&dir));
            }
            Command::Telemetry => {
                if self.input.telemetry_forced_off.is_some() {
                    self.notice = Some(
                        "The install count stays off while your environment turns it off.".into(),
                    );
                } else {
                    self.telemetry = !self.telemetry;
                    self.notice = Some(
                        match self.telemetry {
                            true => "Install count on. Saved when you leave.",
                            false => "Install count off. Saved when you leave.",
                        }
                        .into(),
                    );
                }
            }
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
        let [header, _, body, status, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
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
            Paragraph::new(Line::styled(self.input.version, th.muted()))
                .alignment(Alignment::Right),
            header,
        );

        if self.input.snapshot.rows.is_empty() {
            self.render_empty(frame, body);
        } else {
            self.render_lanes(frame, body);
        }
        frame.render_widget(Paragraph::new(self.status_line(status.width)), status);
        frame.render_widget(Paragraph::new(self.keys_line()), footer);
        self.render_overlay(frame, body);
    }

    /// Signed-in user, daemon health, CPU and running apps, disk saved.
    fn status_line(&self, width: u16) -> Line<'static> {
        let th = &self.input.theme;
        let s = &self.input.snapshot;
        let daemon = match (s.source, width >= 110) {
            (Source::Daemon, _) => "daemon running",
            (Source::StateFile, true) => "daemon not running yet: live from Lanes' state",
            (Source::StateFile, false) => "no daemon yet",
        };
        let running = s.running();
        let mut parts = vec![
            "not signed in".to_string(),
            daemon.to_string(),
            format!("{} cores", s.cores),
            format!(
                "{running} app{} running",
                if running == 1 { "" } else { "s" }
            ),
        ];
        if s.savings.measured > 0 {
            parts.push(format!("{} saved", insight::bytes(s.savings.saved)));
        }
        Line::styled(parts.join("  ·  "), th.muted())
    }

    fn keys_line(&self) -> Line<'static> {
        let th = &self.input.theme;
        if let Some(n) = &self.notice {
            return Line::styled(n.clone(), th.accent());
        }
        if let Some(b) = &self.busy {
            return Line::styled(format!("{b}…"), th.accent());
        }
        let keys: &[(&str, &str)] = match self.overlay {
            Overlay::Palette { .. } => &[
                ("type", "to filter"),
                ("↑↓", "choose"),
                ("Enter", "run"),
                ("Esc", "close"),
            ],
            Overlay::Name(_) => &[
                ("type", "the lane's name"),
                ("Enter", "make it"),
                ("Esc", "cancel"),
            ],
            Overlay::Confirm(_) => &[("y", "remove it"), ("n", "keep it")],
            Overlay::Help | Overlay::Logs(_) | Overlay::Doctor(_) => &[("any key", "close")],
            Overlay::None if self.input.snapshot.rows.is_empty() => &[
                ("?", "help"),
                (":", "commands"),
                ("s", "setup again"),
                ("t", "install count"),
                ("q", "quit"),
            ],
            Overlay::None => &[
                ("?", "help"),
                (":", "commands"),
                ("↑↓", "select"),
                ("o", "open"),
                ("n", "new"),
                ("d", "remove"),
                ("q", "quit"),
            ],
        };
        let mut spans = Vec::new();
        for (i, (k, what)) in keys.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled("  ·  ", th.muted()));
            }
            spans.push(Span::styled(k.to_string(), th.key()));
            spans.push(Span::styled(format!(" {what}"), th.muted()));
        }
        Line::from(spans)
    }

    fn render_empty(&self, frame: &mut Frame, body: Rect) {
        let th = &self.input.theme;
        let mut lines = if body.height >= 20 {
            let mut big = crate::widgets::wordmark(th, f64::INFINITY, 0.6);
            big.push(Line::default());
            big
        } else {
            Vec::new()
        };
        lines.extend([
            Line::styled("No lanes yet.", th.strong()),
            Line::styled(
                "Your lanes will show here, each with its agent, URL, changed files and tests.",
                th.muted(),
            ),
            Line::default(),
        ]);
        let next = match &self.input.repo {
            Some(r) if !r.set_up => ("This repo isn't set up yet: ", "lane init"),
            Some(_) => ("Make your first lane: ", "lane new fix-auth"),
            None => ("Go to a repo you work on: ", "cd your-project"),
        };
        lines.push(Line::from(vec![
            Span::styled(next.0, th.text()),
            Span::styled(next.1, th.accent()),
        ]));
        if let Some(line) = self.agents_line() {
            lines.push(line);
        }
        lines.push(Line::default());
        lines.push(Line::styled("Commands", th.strong()));
        for (cmd, what) in START {
            lines.push(Line::from(vec![
                Span::styled(format!("  {cmd:<22}"), th.accent()),
                Span::styled(what, th.text()),
            ]));
        }
        lines.push(Line::default());
        lines.push(Line::styled("Settings", th.strong()));
        let tele = match (self.input.telemetry_forced_off, self.telemetry) {
            (Some(why), _) => format!("off ({why})"),
            (None, true) => "on".into(),
            (None, false) => "off".into(),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<22}", "Install count"), th.text()),
            Span::styled(tele, th.text()),
        ]));
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<22}", "Signed in"), th.text()),
            Span::styled("no", th.text()),
            Span::styled("   sign-in opens with the gateway", th.muted()),
        ]));
        frame.render_widget(Paragraph::new(lines), body);
    }

    fn agents_line(&self) -> Option<Line<'static>> {
        let th = &self.input.theme;
        (!self.input.agents_unset.is_empty()).then(|| {
            Line::from(vec![
                Span::styled(
                    format!(
                        "{}: not in lanes yet. Run ",
                        self.input.agents_unset.join(", ")
                    ),
                    th.text(),
                ),
                Span::styled("lane setup", th.accent()),
                Span::styled(" to put them there.", th.text()),
            ])
        })
    }

    fn render_lanes(&self, frame: &mut Frame, body: Rect) {
        let th = &self.input.theme;
        let s = &self.input.snapshot;
        // The table as tall as its lanes, what they saved right under it, then the agents hint.
        // With more lanes than fit, the table scrolls to keep the selected one in view.
        let savings = self.savings_lines(body.width);
        let [title, table_area, _, savings_area, _, hint] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Max(s.rows.len() as u16 + 1),
            Constraint::Length(1),
            Constraint::Length(savings.len() as u16),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .flex(ratatui::layout::Flex::Start)
        .areas(body);

        let heading = match &self.input.repo {
            Some(r) => format!("Lanes in {}", r.name),
            None => "Your lanes".into(),
        };
        frame.render_widget(Paragraph::new(Line::styled(heading, th.strong())), title);
        frame.render_widget(
            Paragraph::new(Line::styled("updated live", th.muted())).alignment(Alignment::Right),
            title,
        );

        let wide = body.width >= 104;
        let mut header = vec!["LANE", "STATUS", "URL", "CHANGED", "EXTRA DISK"];
        if wide {
            header.insert(1, "AGENT");
            header.insert(5, "TESTS");
        }
        let rows: Vec<TableRow> = s
            .rows
            .iter()
            .map(|r| {
                let (status, style) = status(r, th);
                let url = lane_env::url(&r.lane)
                    .trim_start_matches("http://")
                    .to_string();
                let changed = match r.details.changed {
                    0 => "—".to_string(),
                    1 => "1 file".to_string(),
                    n => format!("{n} files"),
                };
                let extra = match &r.stats {
                    Some(st) if st.method == "clone" => insight::bytes(insight::lane_disk(
                        st,
                        r.details.changed_bytes,
                        insight::PER_FILE_BYTES,
                    )),
                    Some(st) => format!("{} (copy)", insight::bytes(st.bytes)),
                    None => "—".into(),
                };
                let mut cells = vec![
                    Cell::from(r.lane.name.clone()).style(th.accent()),
                    Cell::from(status).style(style),
                    Cell::from(url),
                    Cell::from(changed),
                    Cell::from(extra),
                ];
                if wide {
                    cells.insert(1, Cell::from(agent_name(r.lane.agent.as_deref())));
                    let tests = match &r.details.tests {
                        Some(t) if t.passed => Cell::from("✓ passed").style(th.accent()),
                        Some(_) => Cell::from("✗ failed").style(th.fail()),
                        None => Cell::from("—"),
                    };
                    cells.insert(5, tests);
                }
                TableRow::new(cells)
            })
            .collect();
        let widths: Vec<Constraint> = if wide {
            vec![
                Constraint::Length(16),
                Constraint::Length(12),
                Constraint::Length(9),
                Constraint::Fill(1),
                Constraint::Length(9),
                Constraint::Length(9),
                Constraint::Length(14),
            ]
        } else {
            vec![
                Constraint::Length(16),
                Constraint::Length(9),
                Constraint::Fill(1),
                Constraint::Length(9),
                Constraint::Length(12),
            ]
        };
        let table = Table::new(rows, widths)
            .header(TableRow::new(header).style(th.muted()))
            .column_spacing(2)
            .row_highlight_style(Style::new().add_modifier(Modifier::BOLD))
            .highlight_symbol("▸ ");
        let mut state = TableState::default().with_selected(Some(self.selected));
        frame.render_stateful_widget(table, table_area, &mut state);

        frame.render_widget(Paragraph::new(savings), savings_area);
        if let Some(line) = self.agents_line() {
            frame.render_widget(Paragraph::new(line), hint);
        }
    }

    /// What lanes saved. Disk is two bars on one scale: what a worktree plus its own install
    /// would take for each lane, against what the lanes take. Then the installs skipped, and the
    /// ports each lane has to itself.
    fn savings_lines(&self, width: u16) -> Vec<Line<'static>> {
        let th = &self.input.theme;
        let s = &self.input.snapshot.savings;
        let mut lines = vec![];
        if s.measured == 0 {
            lines.push(Line::styled("Saved by lanes", th.strong()));
            lines.push(Line::styled(
                "  Make a lane with this version, and what it saves shows here.",
                th.muted(),
            ));
            return lines;
        }
        let mut headline = vec![
            Span::styled("Saved by lanes   ", th.strong()),
            Span::styled(
                format!("{} of disk", insight::bytes(s.saved)),
                th.accent_strong(),
            ),
        ];
        if s.ms_saved >= 1000 {
            headline.push(Span::styled("  ·  ", th.muted()));
            headline.push(Span::styled(
                format!("{} of waiting", insight::duration(s.ms_saved)),
                th.accent_strong(),
            ));
        }
        lines.push(Line::from(headline));

        let notes = width >= 100;
        let bar_w = (width as usize)
            .saturating_sub(if notes { 80 } else { 30 })
            .clamp(8, 28);
        let bar = |label: &str, part: u64, note: &str, style: Style| -> Line<'static> {
            let filled = match s.as_worktrees {
                0 => 0,
                w => ((part as f64 / w as f64) * bar_w as f64).round() as usize,
            }
            .min(bar_w);
            // Anything above nothing gets a sliver, so a small number still shows.
            let (cells, sliver) = match (filled, part) {
                (0, 0) => (0, ""),
                (0, _) => (0, "▏"),
                (n, _) => (n, ""),
            };
            let mut spans = vec![
                Span::styled(format!("  {label:<14}"), th.text()),
                Span::styled("█".repeat(cells), style),
                Span::styled(sliver, style),
                Span::styled(
                    " ".repeat(bar_w - cells - sliver.chars().count()),
                    th.text(),
                ),
                Span::styled(format!("  {:>7}", insight::bytes(part)), th.strong()),
            ];
            if notes {
                spans.push(Span::styled(format!("   {note}"), th.muted()));
            }
            Line::from(spans)
        };
        lines.push(bar(
            "As worktrees",
            s.as_worktrees,
            "a checkout plus its own install for each lane",
            th.muted(),
        ));
        lines.push(bar(
            "As lanes",
            s.as_lanes,
            match s.copies {
                0 => "what the lanes take: the files they changed, and a little for every file",
                _ => "what lanes changed, a little for every file, and the lanes copied in full",
            },
            th.accent(),
        ));
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<14}", "Installs"), th.text()),
            Span::styled(
                match (s.installs_skipped, s.ms_saved) {
                    (0, _) => "none skipped yet".to_string(),
                    (n, 0) => format!(
                        "{n} of {} lanes didn't install again, but making them took as long",
                        s.measured
                    ),
                    (n, ms) => format!(
                        "{n} of {} lanes didn't install again: at least {} saved",
                        s.measured,
                        insight::duration(ms)
                    ),
                },
                th.text(),
            ),
        ]));
        let rows = &self.input.snapshot.rows;
        let ports: u32 = rows.iter().map(|r| r.lane.ports.count as u32).sum();
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<14}", "Ports"), th.text()),
            Span::styled(
                format!(
                    "{} lane{} with {ports} ports of their own, so no two apps clash",
                    rows.len(),
                    if rows.len() == 1 { "" } else { "s" },
                ),
                th.text(),
            ),
        ]));
        // Lanes that have gone still count: what every lane saved since the first.
        let t = &self.input.snapshot.totals;
        if t.removed > 0 {
            let since = t
                .since
                .map(|s| lane_core::log::utc(s)[..10].to_string())
                .unwrap_or_default();
            let mut parts = vec![format!("{} lanes since {since}", t.lanes)];
            if t.disk_measured > 0 {
                parts.push(format!("{} not written", insight::bytes(t.not_written)));
            }
            if t.ms_saved >= 1000 {
                parts.push(format!(
                    "at least {} of waiting saved",
                    insight::duration(t.ms_saved)
                ));
            }
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<14}", "In total"), th.text()),
                Span::styled(parts.join(", "), th.text()),
            ]));
        }
        if s.copies > 0 {
            lines.push(Line::styled(
                format!(
                    "  {} lane{} copied in full: this disk can't clone files. Run lane doctor to see why.",
                    s.copies,
                    if s.copies == 1 { " is" } else { "s are" }
                ),
                th.warn(),
            ));
        }
        lines
    }

    fn render_overlay(&self, frame: &mut Frame, body: Rect) {
        let th = &self.input.theme;
        let (title, lines): (String, Vec<Line>) = match &self.overlay {
            Overlay::None => return,
            Overlay::Help => (
                " Keys ".into(),
                [
                    ("↑ ↓  j k", "select a lane"),
                    ("o", "open the lane's app in the browser"),
                    ("c", "copy the lane's folder"),
                    ("n", "make a new lane"),
                    ("d", "remove the selected lane (asks first)"),
                    ("l", "show what Lanes did"),
                    (":  Ctrl-K", "commands"),
                    ("s", "run the welcome again"),
                    ("t", "switch the install count"),
                    ("?", "show or hide this help"),
                    ("q  Esc", "quit"),
                ]
                .iter()
                .map(|(k, what)| {
                    Line::from(vec![
                        Span::styled(format!("  {k:<12}"), th.key()),
                        Span::styled(*what, th.text()),
                    ])
                })
                .collect(),
            ),
            Overlay::Palette { query, selected } => {
                let mut lines = vec![
                    Line::from(vec![
                        Span::styled("  : ", th.key()),
                        Span::styled(query.clone(), th.strong()),
                        Span::styled("▏", th.accent()),
                    ]),
                    Line::default(),
                ];
                let matches = palette_matches(query);
                if matches.is_empty() {
                    lines.push(Line::styled("  Nothing matches.", th.muted()));
                }
                for (i, (_, key, label)) in matches.iter().enumerate() {
                    let style = if i == *selected {
                        th.accent_strong()
                    } else {
                        th.text()
                    };
                    lines.push(Line::from(vec![
                        Span::styled(if i == *selected { "▸ " } else { "  " }, th.accent()),
                        Span::styled(format!("{label:<52}"), style),
                        Span::styled(key.to_string(), th.muted()),
                    ]));
                }
                (" Commands ".into(), lines)
            }
            Overlay::Name(name) => (
                " New lane ".into(),
                vec![
                    Line::styled(
                        "  Name it after the task: lowercase letters, digits and hyphens.",
                        th.muted(),
                    ),
                    Line::default(),
                    Line::from(vec![
                        Span::styled("  lane new ", th.muted()),
                        Span::styled(name.clone(), th.strong()),
                        Span::styled("▏", th.accent()),
                    ]),
                ],
            ),
            Overlay::Confirm(name) => (
                " Remove a lane ".into(),
                vec![
                    Line::from(vec![
                        Span::styled("  Remove ", th.text()),
                        Span::styled(name.clone(), th.strong()),
                        Span::styled(" and its folder, changes and all? ", th.text()),
                        Span::styled("y/n", th.key()),
                    ]),
                    Line::styled("  Its ports are freed. This can't be undone.", th.muted()),
                ],
            ),
            Overlay::Logs(entries) => {
                let mut lines: Vec<Line> = match entries.is_empty() {
                    true => vec![Line::styled("  Nothing logged yet.", th.muted())],
                    false => entries
                        .iter()
                        .map(|e| Line::styled(format!(" {e}"), th.text()))
                        .collect(),
                };
                let room = body.height.saturating_sub(4) as usize;
                if lines.len() > room {
                    lines.drain(..lines.len() - room);
                }
                (" What Lanes did (lane logs) ".into(), lines)
            }
            Overlay::Doctor(checks) => (
                " lane doctor ".into(),
                checks
                    .iter()
                    .flat_map(|c| {
                        let mut v = vec![Line::from(vec![
                            Span::styled(format!(" {} ", c.status.symbol()), th.status(c.status)),
                            Span::styled(format!("{:<16}", c.label), th.strong()),
                            Span::styled(c.summary.clone(), th.text()),
                        ])];
                        if let Some(h) = c
                            .hint
                            .as_ref()
                            .filter(|_| c.status != lane_core::machine::Status::Ok)
                        {
                            v.push(Line::styled(format!("{:<20}{h}", ""), th.muted()));
                        }
                        v
                    })
                    .collect(),
            ),
        };
        // As wide as what it shows, within the screen.
        let widest = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
        let w = (widest + 4)
            .max(title.chars().count() as u16 + 6)
            .max(44)
            .min(body.width.saturating_sub(4));
        let h = (lines.len() as u16 + 2).min(body.height);
        let area = Rect {
            x: body.x + (body.width - w) / 2,
            y: body.y + (body.height - h) / 3,
            width: w,
            height: h,
        };
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .block(Block::bordered().title(Span::styled(title, th.strong()))),
            area,
        );
    }
}

/// A lane's status, and how to show it.
fn status(r: &Row, th: &Theme) -> (String, Style) {
    if r.gone {
        return ("gone".into(), th.warn());
    }
    if r.details.running {
        return ("running".into(), th.accent_strong());
    }
    (r.lane.state().to_string(), th.text())
}

fn agent_name(agent: Option<&str>) -> String {
    match agent {
        Some("claude") => "Claude Code".into(),
        Some("codex") => "Codex".into(),
        Some("cursor") => "Cursor".into(),
        Some("gemini") => "Gemini CLI".into(),
        Some("copilot") => "GitHub Copilot".into(),
        Some(other) => other.to_string(),
        None => "—".into(),
    }
}

/// The palette's commands that match what's been typed, in order.
fn palette_matches(query: &str) -> Vec<(Command, &'static str, &'static str)> {
    let q = query.to_ascii_lowercase();
    COMMANDS
        .iter()
        .copied()
        .filter(|(_, key, label)| {
            q.is_empty() || label.to_ascii_lowercase().contains(&q) || *key == q
        })
        .collect()
}
