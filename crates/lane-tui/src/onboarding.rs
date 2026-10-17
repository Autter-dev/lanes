//! The first-run onboarding: six steps along one lane. Welcome, your machine, how a lane works,
//! privacy, sign-in (optional, disabled until the gateway opens) and ready.

use crate::theme::Theme;
use crate::widgets;
use crate::wrap::wrap;
use lane_core::machine::{self, CHECK_IDS, Check, Status};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::Instant;

const STEPS: [&str; 6] = [
    "Welcome",
    "Your machine",
    "How a lane works",
    "Privacy",
    "Sign in",
    "Ready",
];
const MACHINE: usize = 1;
const EXPLAINER: usize = 2;
const PRIVACY: usize = 3;
const SIGN_IN: usize = 4;
const READY: usize = 5;

/// How long the step marker takes to glide to the next stop.
const GLIDE_SECS: f64 = 0.35;

/// The smallest terminal the onboarding lays out in.
pub const MIN_SIZE: (u16, u16) = (60, 18);

pub struct Input {
    /// The folder the checks look at (where `lane` was run).
    pub dir: PathBuf,
    pub telemetry: bool,
    /// Why the install count is off no matter what (an environment opt-out).
    pub telemetry_forced_off: Option<&'static str>,
    pub animate: bool,
    pub theme: Theme,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Finished the last step: save these choices.
    Finished { telemetry: bool },
    /// Left early. Nothing is saved, so onboarding shows again next time.
    Quit,
}

struct Explained {
    title: &'static str,
    text: &'static str,
    cmd: &'static str,
}

const EXPLAINED: [Explained; 4] = [
    Explained {
        title: "Your repo",
        text: "Keep your Git repo, your GitHub and your agents as they are. Nothing to migrate.",
        cmd: "lane init",
    },
    Explained {
        title: "A lane per agent",
        text: "Each agent gets an instant, ready-to-run copy with its own URL, ports and database. When an agent makes a Git worktree, it becomes a lane on its own.",
        cmd: "lane new fix-auth",
    },
    Explained {
        title: "A warning, not a merge conflict",
        text: "The moment two agents open the same function, you see it on both lanes while they're still working, not when you merge.",
        cmd: "lane list",
    },
    Explained {
        title: "One clean PR",
        text: "Keep the best lane and turn it into one pull request, with who (and which agent) wrote what. The rest stay out of your branch.",
        cmd: "lane promote fix-auth",
    },
];

pub struct Onboarding {
    step: usize,
    explainer_step: usize,
    telemetry: bool,
    forced_off: Option<&'static str>,
    dir: PathBuf,
    checks: Vec<Option<Check>>,
    revealed: usize,
    last_reveal: f64,
    rx: Option<Receiver<(usize, Check)>>,
    help: bool,
    notice: Option<(&'static str, f64)>,
    started: Instant,
    /// The step the marker glides from, and when the move started.
    from_step: usize,
    moved_at: f64,
    animate: bool,
    theme: Theme,
}

impl Onboarding {
    /// Start the onboarding; the machine checks begin at once, in the background.
    pub fn new(input: Input) -> Onboarding {
        let (tx, rx) = mpsc::channel();
        for (i, id) in CHECK_IDS.iter().enumerate() {
            let tx = tx.clone();
            let dir = input.dir.clone();
            std::thread::spawn(move || {
                let _ = tx.send((i, machine::run_check(id, &dir)));
            });
        }
        let mut o = Onboarding::blank(input);
        o.rx = Some(rx);
        o
    }

    /// Start with checks already known (for tests and still previews).
    pub fn with_checks(input: Input, checks: Vec<Check>) -> Onboarding {
        let mut o = Onboarding::blank(input);
        o.revealed = checks.len();
        o.checks = checks.into_iter().map(Some).collect();
        o
    }

    fn blank(input: Input) -> Onboarding {
        let forced_off = input.telemetry_forced_off;
        Onboarding {
            step: 0,
            explainer_step: 0,
            telemetry: input.telemetry && forced_off.is_none(),
            forced_off,
            dir: input.dir,
            checks: vec![None; CHECK_IDS.len()],
            revealed: 0,
            last_reveal: 0.0,
            rx: None,
            help: false,
            notice: None,
            started: Instant::now(),
            from_step: 0,
            moved_at: f64::NEG_INFINITY,
            animate: input.animate,
            theme: input.theme,
        }
    }

    /// Jump to a step (tests and previews).
    pub fn go_to(&mut self, step: usize, explainer_step: usize) {
        self.step = step.min(READY);
        self.explainer_step = explainer_step.min(3);
    }

    fn elapsed(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    /// The time drawings use: real time, or a still frame when animation is off.
    fn t(&self) -> f64 {
        if self.animate {
            self.elapsed()
        } else {
            f64::INFINITY
        }
    }

    fn checks_done(&self) -> bool {
        self.revealed == self.checks.len()
    }

    /// Take in finished checks and reveal them one by one. True if anything changed.
    pub fn tick(&mut self) -> bool {
        let mut changed = false;
        if let Some(rx) = &self.rx {
            while let Ok((i, check)) = rx.try_recv() {
                self.checks[i] = Some(check);
                changed = true;
            }
        }
        let now = self.elapsed();
        let gap = if self.animate { 0.14 } else { 0.0 };
        while self.revealed < self.checks.len()
            && self.checks[self.revealed].is_some()
            && now - self.last_reveal >= gap
        {
            self.revealed += 1;
            self.last_reveal = now;
            changed = true;
            if self.animate {
                break;
            }
        }
        if let Some((_, at)) = self.notice
            && now - at > 3.0
        {
            self.notice = None;
            changed = true;
        }
        changed
    }

    /// True while something on screen moves, so the loop keeps drawing.
    pub fn animating(&self) -> bool {
        let moving = match self.step {
            0 => true,
            MACHINE => !self.checks_done(),
            EXPLAINER => self.explainer_step > 0,
            PRIVACY => self.telemetry,
            SIGN_IN | READY => true,
            _ => false,
        };
        let gliding = self.elapsed() - self.moved_at < GLIDE_SECS + 0.05;
        (self.animate && (moving || gliding)) || (self.step == MACHINE && !self.checks_done())
    }

    pub fn telemetry(&self) -> bool {
        self.telemetry
    }

    fn next(&mut self) -> Option<Outcome> {
        if self.step == EXPLAINER && self.explainer_step < 3 {
            self.explainer_step += 1;
        } else if self.step == READY {
            return Some(Outcome::Finished {
                telemetry: self.telemetry,
            });
        } else {
            self.move_to(self.step + 1);
        }
        None
    }

    fn move_to(&mut self, step: usize) {
        self.from_step = self.step;
        self.moved_at = self.elapsed();
        self.step = step;
    }

    fn glide(&self) -> Option<(usize, f64)> {
        self.animate.then(|| {
            (
                self.from_step,
                (self.elapsed() - self.moved_at) / GLIDE_SECS,
            )
        })
    }

    fn back(&mut self) {
        if self.step == EXPLAINER && self.explainer_step > 0 {
            self.explainer_step -= 1;
        } else if self.step > 0 {
            self.move_to(self.step - 1);
            if self.step == EXPLAINER {
                self.explainer_step = 3;
            }
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Option<Outcome> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Some(Outcome::Quit);
        }
        if self.help {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') | KeyCode::Enter
            ) {
                self.help = false;
            }
            return None;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Some(Outcome::Quit),
            KeyCode::Char('?') => self.help = true,
            KeyCode::Enter | KeyCode::Right | KeyCode::Tab | KeyCode::Char('l') => {
                return self.next();
            }
            KeyCode::Left | KeyCode::BackTab | KeyCode::Backspace | KeyCode::Char('h') => {
                self.back()
            }
            KeyCode::Char(' ') if self.step == PRIVACY => {
                if self.forced_off.is_some() {
                    self.notice = Some((
                        "It stays off while that setting is in your environment.",
                        self.elapsed(),
                    ));
                } else {
                    self.telemetry = !self.telemetry;
                }
            }
            KeyCode::Up | KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('k')
                if self.step == SIGN_IN =>
            {
                self.notice = Some((
                    "Sign-in isn't available yet. It opens with the gateway.",
                    self.elapsed(),
                ));
            }
            _ => {}
        }
        None
    }

    // ---------- drawing ----------

    pub fn render(&self, frame: &mut Frame) {
        let area = frame.area();
        if area.width < MIN_SIZE.0 || area.height < MIN_SIZE.1 {
            crate::too_small(frame, &self.theme, MIN_SIZE);
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
        self.render_header(frame, header);
        let lines = match self.step {
            0 => self.welcome(body),
            MACHINE => self.machine(body.width),
            EXPLAINER => self.explainer(body),
            PRIVACY => self.privacy(body.width),
            SIGN_IN => self.sign_in(body.width),
            _ => self.ready(body.width),
        };
        frame.render_widget(Paragraph::new(lines), body);
        if self.step == SIGN_IN && body.width >= 92 {
            let art = Rect {
                x: body.x + body.width - 12,
                y: body.y,
                width: 12,
                height: 4,
            };
            frame.render_widget(Paragraph::new(widgets::otter(&self.theme, self.t())), art);
        }
        self.render_footer(frame, footer);
        if self.help {
            self.render_help(frame, area);
        }
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let th = &self.theme;
        let segment = if area.width >= 90 { 5 } else { 3 };
        let mut spans = vec![
            Span::styled("Lanes", th.strong()),
            Span::styled(".", th.accent_strong()),
            Span::raw("   "),
        ];
        spans.extend(widgets::step_lane(th, self.step, STEPS.len(), segment, self.glide()).spans);
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
        let right = Line::from(vec![
            Span::styled(
                format!("{} of {} · ", self.step + 1, STEPS.len()),
                th.muted(),
            ),
            Span::styled(STEPS[self.step], th.text()),
        ]);
        frame.render_widget(Paragraph::new(right).alignment(Alignment::Right), area);
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        let th = &self.theme;
        let mut keys: Vec<(&str, &str)> = match self.step {
            0 => vec![("Enter", "start")],
            EXPLAINER => vec![("→", "next"), ("←", "back")],
            PRIVACY => vec![("Space", "switch"), ("Enter", "next"), ("←", "back")],
            READY => vec![("Enter", "save and finish"), ("←", "back")],
            _ => vec![("Enter", "next"), ("←", "back")],
        };
        keys.extend([("?", "help"), ("q", "quit")]);
        let mut spans = Vec::new();
        for (i, (k, what)) in keys.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled("  ·  ", th.muted()));
            }
            spans.push(Span::styled(*k, th.key()));
            spans.push(Span::styled(format!(" {what}"), th.muted()));
        }
        let line = match self.notice {
            Some((msg, _)) => Line::from(vec![
                Span::styled("! ", th.warn()),
                Span::styled(msg, th.warn()),
            ]),
            None => Line::from(spans),
        };
        frame.render_widget(Paragraph::new(line), area);
    }

    fn render_help(&self, frame: &mut Frame, area: Rect) {
        let th = &self.theme;
        let w = 52.min(area.width - 4);
        let h = 13.min(area.height - 2);
        let popup = Rect {
            x: area.x + (area.width - w) / 2,
            y: area.y + (area.height - h) / 2,
            width: w,
            height: h,
        };
        let row = |k: &'static str, what: &'static str| {
            Line::from(vec![
                Span::styled(format!("  {k:<12}"), th.key()),
                Span::styled(what, th.text()),
            ])
        };
        let lines = vec![
            Line::default(),
            row("Enter  →", "next step"),
            row("←", "previous step"),
            row("Space", "switch the install count (Privacy)"),
            row("?", "show or hide this help"),
            row("q  Esc", "quit; nothing is saved"),
            row("Ctrl-C", "quit at once"),
            Line::default(),
            Line::styled("  Run `lane setup` any time to see this again.", th.muted()),
        ];
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Keys ")
                    .border_style(th.muted()),
            ),
            popup,
        );
    }

    fn para(&self, text: &str, width: u16, indent: usize) -> Vec<Line<'static>> {
        wrap(text, (width as usize).min(78), indent)
            .into_iter()
            .map(|l| Line::styled(l, self.theme.text()))
            .collect()
    }

    fn welcome(&self, body: Rect) -> Vec<Line<'static>> {
        let th = &self.theme;
        let width = body.width;
        // The big wordmark when there's room for it (80x24 and up); one bold line otherwise.
        let mut lines = if body.height >= 19 {
            let mut big = widgets::wordmark(th, self.t(), 0.6);
            big.push(Line::default());
            big
        } else {
            vec![Line::default()]
        };
        lines.extend([
            Line::styled("Every agent gets its own lane.", th.strong()),
            Line::default(),
        ]);
        lines.extend(widgets::hero_lanes(th, self.t(), width as usize));
        let track = (width as usize).saturating_sub(16 + 12 + 2).clamp(10, 64);
        let mut otter_lane = vec![Span::styled(format!("{:<16}", "   by Autter"), th.muted())];
        otter_lane.extend(widgets::swimming_otter(th, self.t(), track, widgets::Swim::Loop).spans);
        lines.push(Line::from(otter_lane));
        lines.push(Line::default());
        lines.extend(self.para(
            "Lanes gives each coding agent an instant copy of your project, with its own ports, URL and database, so they never get in each other's way. When one gets it right, its work becomes one clean pull request.",
            width,
            0,
        ));
        lines.push(Line::default());
        lines.extend(
            wrap(
                "About two minutes. Nothing is installed or sent while you look around.",
                (width as usize).min(78),
                0,
            )
            .into_iter()
            .map(|l| Line::styled(l, th.muted())),
        );
        lines
    }

    fn machine(&self, width: u16) -> Vec<Line<'static>> {
        let th = &self.theme;
        let label_w = 15;
        let mut lines = vec![
            Line::from(vec![
                Span::styled("Checking this machine", th.strong()),
                Span::styled(format!("  in {}", self.dir.display()), th.muted()),
            ]),
            Line::default(),
        ];
        for (i, id) in CHECK_IDS.iter().enumerate() {
            match (&self.checks[i], i < self.revealed) {
                (Some(c), true) => {
                    // The summary wraps under itself, so long results never run off the edge.
                    let mut parts =
                        wrap(&c.summary, (width as usize).min(96), label_w + 2).into_iter();
                    let first = parts.next().unwrap_or_default();
                    lines.push(Line::from(vec![
                        Span::styled(format!("{} ", c.status.symbol()), th.status(c.status)),
                        Span::styled(format!("{:<label_w$}", c.label), th.strong()),
                        Span::styled(first.trim_start().to_string(), th.text()),
                    ]));
                    lines.extend(parts.map(|l| Line::styled(l, th.text())));
                    if let Some(hint) = c.hint.as_deref().filter(|_| c.status != Status::Ok) {
                        for l in wrap(hint, (width as usize).min(96), label_w + 2) {
                            lines.push(Line::styled(l, th.muted()));
                        }
                    }
                }
                _ => {
                    let mut row = widgets::runner(th, self.t() + i as f64 * 0.37, 2);
                    row.push(Span::raw(" "));
                    row.push(Span::styled(
                        format!("{:<label_w$}", label_for(id)),
                        th.muted(),
                    ));
                    row.extend(widgets::runner(th, self.t() * 1.3 + i as f64, 8));
                    row.push(Span::styled(" checking", th.muted()));
                    lines.push(Line::from(row));
                }
            }
        }
        lines.push(Line::default());
        if self.checks_done() {
            lines.push(self.machine_summary());
        }
        lines
    }

    fn machine_summary(&self) -> Line<'static> {
        let th = &self.theme;
        let checks: Vec<&Check> = self.checks.iter().flatten().collect();
        let fails = checks.iter().filter(|c| c.status == Status::Fail).count();
        let warns = checks.iter().filter(|c| c.status == Status::Warn).count();
        match (fails, warns) {
            (0, 0) => Line::styled("Everything Lanes needs is here.", th.accent_strong()),
            (0, n) => Line::styled(
                format!(
                    "{n} thing{} worth knowing, and nothing stops you.",
                    if n == 1 { "" } else { "s" }
                ),
                th.warn(),
            ),
            (n, _) => Line::styled(
                format!(
                    "{n} thing{} to fix before Lanes can make lanes. You can still look around.",
                    if n == 1 { "" } else { "s" }
                ),
                th.fail(),
            ),
        }
    }

    fn explainer(&self, body: Rect) -> Vec<Line<'static>> {
        let th = &self.theme;
        let e = &EXPLAINED[self.explainer_step];
        let mut dots = Vec::new();
        for i in 0..4 {
            let (dot, style) = if i <= self.explainer_step {
                ("●", th.accent())
            } else {
                ("○", th.muted())
            };
            dots.push(Span::styled(dot, style));
            dots.push(Span::raw(" "));
        }
        dots.push(Span::styled(
            format!(" step {} of 4", self.explainer_step + 1),
            th.muted(),
        ));
        let mut lines = vec![Line::from(dots), Line::default()];
        lines.extend(widgets::explainer(
            th,
            self.explainer_step,
            self.t(),
            body.width,
        ));
        lines.push(Line::default());
        lines.push(Line::styled(e.title, th.strong()));
        lines.extend(self.para(e.text, body.width, 0));
        lines.push(Line::default());
        lines.push(Line::from(vec![
            Span::styled("$ ", th.muted()),
            Span::styled(e.cmd, th.accent()),
        ]));
        lines
    }

    fn privacy(&self, width: u16) -> Vec<Line<'static>> {
        let th = &self.theme;
        let item = |s: &'static str| {
            Line::from(vec![
                Span::styled("  · ", th.muted()),
                Span::styled(s, th.text()),
            ])
        };
        let mut lines = vec![
            Line::styled("The anonymous install count", th.strong()),
            Line::default(),
            Line::styled("When it's on, Lanes sends this, and only this:", th.text()),
            item("a random install id, not linked to you"),
            item("the Lanes version"),
            item("your OS and CPU type"),
            item("how you installed Lanes (Homebrew, npm, cargo…)"),
            Line::default(),
        ];
        lines.extend(self.para("It never sends code, file paths, repo names or your IP address. It's how we know whether anyone uses Lanes.", width, 0));
        lines.push(Line::default());
        let (mark, style, label) = match (self.forced_off, self.telemetry) {
            (Some(_), _) => ("○", th.muted(), "Off"),
            (None, true) => ("●", th.accent_strong(), "On"),
            (None, false) => ("○", th.text(), "Off"),
        };
        let mut row = vec![
            Span::styled(format!("[{mark}] "), style),
            Span::styled("Send the anonymous install count: ", th.text()),
            Span::styled(label, style),
        ];
        if let Some(why) = self.forced_off {
            row.push(Span::styled(
                format!(" · {why}, so it stays off"),
                th.muted(),
            ));
        } else {
            row.push(Span::styled("   Space to switch", th.muted()));
        }
        lines.push(Line::from(row));
        lines.push(widgets::privacy_flow(th, self.t(), self.telemetry));
        lines.push(Line::default());
        lines.push(Line::styled(
            "Change it any time with `lane setup`, or set DO_NOT_TRACK=1.",
            th.muted(),
        ));
        lines
    }

    fn sign_in(&self, width: u16) -> Vec<Line<'static>> {
        let th = &self.theme;
        let item = |s: &'static str| {
            Line::from(vec![
                Span::styled("  · ", th.muted()),
                Span::styled(s, th.text()),
            ])
        };
        let mut title = vec![Span::styled("Autter, for teams (optional)", th.strong())];
        if width < 92 {
            title.push(Span::styled("   =(o.o)=", th.otter()));
        }
        let mut lines = vec![Line::from(title), Line::default()];
        lines.extend(self.para(
            "Signing in connects the repos you choose to the Lanes gateway on Autter:",
            width,
            0,
        ));
        lines.extend([
            item("repo Q&A, and security checks while agents work"),
            item("a safe-to-merge verdict on every pull request"),
            item("cloud agents and CI that read your repo without cloning"),
            Line::default(),
        ]);
        lines.extend(self.para(
            "You never need Autter to use Lanes. Everything else works without an account.",
            width,
            0,
        ));
        lines.extend([
            Line::default(),
            Line::from(vec![
                Span::styled("› ", th.accent_strong()),
                Span::styled("Continue without signing in", th.accent_strong()),
            ]),
            Line::from(vec![
                Span::styled("  Sign in to Autter", th.disabled()),
                Span::styled("    available when the gateway opens", th.muted()),
            ]),
        ]);
        lines
    }

    fn ready(&self, width: u16) -> Vec<Line<'static>> {
        let th = &self.theme;
        let find = |id: &str| self.checks.iter().flatten().find(|c| c.id == id);
        let in_repo = find("repo").is_some_and(|c| c.status == Status::Ok);
        let swim = widgets::Swim::Arrive {
            from: self.moved_at.max(0.0),
            secs: 2.2,
        };
        let lane_w = (width as usize).saturating_sub(16).min(44);
        let mut top = vec![Span::styled("You're set.  ", th.accent_strong())];
        top.extend(widgets::swimming_otter(th, self.t(), lane_w, swim).spans);
        let arrived = !self.animate || self.elapsed() - self.moved_at >= 2.2;
        if arrived {
            top.push(Span::styled(" ✓", th.accent_strong()));
        }
        let mut lines = vec![Line::from(top), Line::default()];
        for id in ["copies", "agents"] {
            if let Some(c) = find(id) {
                lines.push(Line::from(vec![
                    Span::styled(format!("{} ", c.status.symbol()), th.status(c.status)),
                    Span::styled(format!("{}: ", c.label), th.strong()),
                    Span::styled(
                        fit(
                            &c.summary,
                            (width as usize).saturating_sub(c.label.chars().count() + 4),
                        ),
                        th.text(),
                    ),
                ]));
            }
        }
        let tele = if self.telemetry { "on" } else { "off" };
        lines.push(Line::from(vec![
            Span::styled("· ", th.muted()),
            Span::styled("Install count: ", th.strong()),
            Span::styled(tele, th.text()),
        ]));
        lines.push(Line::from(vec![
            Span::styled("· ", th.muted()),
            Span::styled("Signed in: ", th.strong()),
            Span::styled("no, and you don't need to be", th.text()),
        ]));
        lines.push(Line::default());
        lines.push(Line::styled(
            if in_repo {
                "Next, in this repo:"
            } else {
                "Next:"
            },
            th.strong(),
        ));
        // Commands on the left, what they do in the middle, "soon" in its own column on the right.
        let desc_w = (width as usize).saturating_sub(2 + 20 + 6).min(44);
        let cmd = |c: &'static str, what: &'static str, soon: bool| {
            Line::from(vec![
                Span::styled(format!("  {c:<20}"), th.accent()),
                Span::styled(format!("{:<desc_w$}", fit(what, desc_w)), th.text()),
                Span::styled(if soon { "  soon" } else { "" }, th.muted()),
            ])
        };
        if !in_repo {
            lines.push(cmd(
                "cd your-project",
                "go to a Git repo you work on",
                false,
            ));
        }
        lines.push(cmd("lane init", "set up this repo and your agents", true));
        lines.push(cmd("lane new fix-auth", "make your first lane", true));
        lines.push(cmd("lane", "open Lanes any time", false));
        lines.push(Line::default());
        lines.extend(
            wrap(
                "Commands marked soon are being built in the open: github.com/Autter-dev/lanes",
                (width as usize).min(78),
                0,
            )
            .into_iter()
            .map(|l| Line::styled(l, th.muted())),
        );
        lines
    }
}

/// `s`, cut to `n` columns with an ellipsis when it doesn't fit.
fn fit(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let cut: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{}…", cut.trim_end())
    }
}

fn label_for(id: &str) -> &'static str {
    match id {
        "os" => "System",
        "copies" => "Instant lanes",
        "git" => "Git",
        "repo" => "This folder",
        "stack" => "Your stack",
        "agents" => "Agents",
        "disk" => "Free disk",
        _ => "Check",
    }
}
