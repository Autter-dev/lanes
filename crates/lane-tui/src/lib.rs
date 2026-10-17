//! The Lanes terminal app: the first-run onboarding and the home screen.
//!
//! Only call these when [`lane_core::env::Env::interactive`] says a full-screen app may open.
//! The terminal is always restored on quit, on Ctrl-C and on a panic (`ratatui::init` installs a
//! panic hook that restores it first).

mod actions;
pub mod home;
pub mod onboarding;
pub mod theme;
pub mod widgets;
mod wrap;

use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::{DefaultTerminal, Frame};
use std::io;
use std::time::Duration;

/// Run the onboarding until it finishes or the user quits.
pub fn run_onboarding(input: onboarding::Input) -> io::Result<onboarding::Outcome> {
    let mut app = onboarding::Onboarding::new(input);
    with_terminal(|terminal| {
        let mut dirty = true;
        loop {
            dirty |= app.tick();
            if dirty || app.animating() {
                terminal.draw(|f| app.render(f))?;
                dirty = false;
            }
            // Fast ticks only while something moves; otherwise wait quietly for a key.
            let wait = if app.animating() {
                Duration::from_millis(33)
            } else {
                Duration::from_millis(250)
            };
            if event::poll(wait)? {
                match event::read()? {
                    Event::Key(k) if k.kind == KeyEventKind::Press => {
                        if let Some(outcome) = app.on_key(k) {
                            return Ok(outcome);
                        }
                        dirty = true;
                    }
                    Event::Resize(..) => dirty = true,
                    _ => {}
                }
            }
        }
    })
}

/// Run the home screen until the user quits or asks for setup again.
pub fn run_home(input: home::Input) -> io::Result<home::Outcome> {
    let mut app = home::Home::new(input);
    with_terminal(|terminal| {
        terminal.draw(|f| app.render(f))?;
        loop {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    if let Some(outcome) = app.on_key(k) {
                        return Ok(outcome);
                    }
                    terminal.draw(|f| app.render(f))?;
                }
                Event::Resize(..) => {
                    terminal.draw(|f| app.render(f))?;
                }
                _ => {}
            }
        }
    })
}

fn with_terminal<T>(run: impl FnOnce(&mut DefaultTerminal) -> io::Result<T>) -> io::Result<T> {
    let mut terminal = ratatui::try_init()?;
    let result = run(&mut terminal);
    ratatui::restore();
    result
}

/// What every screen shows when the terminal is smaller than it can lay out in.
pub(crate) fn too_small(frame: &mut Frame, theme: &theme::Theme, min: (u16, u16)) {
    let area = frame.area();
    let [_, mid, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(3),
        Constraint::Fill(1),
    ])
    .areas(area);
    let lines = vec![
        Line::styled("Make the window a little bigger", theme.strong()),
        Line::styled(
            format!(
                "Lanes needs {}×{}; this is {}×{}.",
                min.0, min.1, area.width, area.height
            ),
            theme.muted(),
        ),
        Line::styled("q to quit", theme.muted()),
    ];
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), mid);
}

#[cfg(test)]
mod tests;
