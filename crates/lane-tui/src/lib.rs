//! The Lanes terminal app: the first-run onboarding and the home screen.
//!
//! Only call these when [`lane_core::env::Env::interactive`] says a full-screen app may open.
//! The terminal is always restored on quit, on Ctrl-C and on a panic (`ratatui::init` installs a
//! panic hook that restores it first).

mod actions;
pub mod feed;
pub mod home;
pub mod onboarding;
pub mod theme;
pub mod widgets;
mod wrap;

use lane_core::engine::{self, Repo};
use lane_core::state::Store;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::{DefaultTerminal, Frame};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
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

/// Run the home screen until the user quits or asks for setup again. The lanes stay live: a
/// snapshot arrives whenever something changes (from `laned` when `daemon` allows starting it,
/// otherwise from a watcher here), and the screen redraws only then (or on a key or a resize).
/// Making and removing a lane run in the background, with their output in Lanes' log rather
/// than on the screen.
pub fn run_home(input: home::Input, daemon: bool) -> io::Result<home::Outcome> {
    let root = input.repo.as_ref().map(|r| r.root.clone());
    let mut app = home::Home::new(input);
    let watch = feed::watch(root.clone(), Duration::from_secs(2), daemon);
    let (done_tx, done_rx) = mpsc::channel::<String>();
    let was_quiet = lane_core::log::quiet();
    lane_core::log::set_quiet(true);
    let result = with_terminal(|terminal| {
        let mut dirty = true;
        loop {
            while let Ok(snapshot) = watch.rx.try_recv() {
                dirty |= app.update(snapshot);
            }
            if let Ok(message) = done_rx.try_recv() {
                app.done(message);
                app.update(feed::read(root.as_deref()));
                dirty = true;
            }
            if dirty {
                terminal.draw(|f| app.render(f))?;
                dirty = false;
            }
            if !event::poll(Duration::from_millis(250))? {
                continue;
            }
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    match app.on_key(k) {
                        Some(home::Action::Exit(outcome)) => return Ok(outcome),
                        Some(home::Action::New(name)) => {
                            background(&done_tx, root.clone(), move |root| make(&root, &name))
                        }
                        Some(home::Action::Remove(name)) => {
                            background(&done_tx, root.clone(), move |root| drop_lane(&root, &name))
                        }
                        None => {}
                    }
                    dirty = true;
                }
                Event::Resize(..) => dirty = true,
                _ => {}
            }
        }
    });
    lane_core::log::set_quiet(was_quiet);
    result
}

/// Run a lane change on its own thread, and send back what to tell the user.
fn background(
    done: &mpsc::Sender<String>,
    root: Option<PathBuf>,
    job: impl FnOnce(PathBuf) -> lane_core::error::Result<String> + Send + 'static,
) {
    let done = done.clone();
    std::thread::spawn(move || {
        let message = match root {
            Some(root) => job(root).unwrap_or_else(|e| e.to_string()),
            None => "Open Lanes in a repo to change its lanes.".into(),
        };
        let _ = done.send(message);
    });
}

fn make(root: &Path, name: &str) -> lane_core::error::Result<String> {
    let started = std::time::Instant::now();
    let repo = Repo::open(root)?;
    let mut store = Store::open()?;
    let made = engine::new_lane(&mut store, &repo, name, None, None)?;
    lane_core::log::info(
        "new",
        format!(
            "made the lane from the terminal app: {} files {} at {}",
            made.clone.files,
            if made.clone.copied == 0 {
                "cloned"
            } else {
                "copied"
            },
            made.lane.path.display()
        ),
    )
    .lane(&made.lane.name)
    .repo(&repo.root)
    .ms(started.elapsed().as_millis() as u64)
    .write();
    Ok(format!(
        "Made {name} at {}. Its app runs at {}",
        made.lane.path.display(),
        lane_core::lane_env::url(&made.lane)
    ))
}

fn drop_lane(root: &Path, name: &str) -> lane_core::error::Result<String> {
    let repo = Repo::open(root)?;
    let mut store = Store::open()?;
    let lane = engine::remove(&mut store, &repo, name, false, None)?;
    lane_core::log::info(
        "rm",
        "removed the lane and its folder from the terminal app",
    )
    .lane(&lane.name)
    .repo(&repo.root)
    .write();
    Ok(format!("Removed {name} and its folder."))
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
