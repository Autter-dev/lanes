//! Print the home screen as text, with sample lanes: the table, what lanes saved, the status bar,
//! then the help overlay and the command palette over it.
//!   cargo run -p lane-tui --example home [-- WIDTH HEIGHT]

use lane_core::insight::{self, Details, TestRun};
use lane_core::state::{Lane, LaneStats, PortBlock, Status};
use lane_tui::feed::{Row, Snapshot, Source};
use lane_tui::home::{HomeRepo, Input};
use lane_tui::theme::Theme;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::PathBuf;

fn row(name: &str, port: u16, agent: Option<&str>, details: Details, stats: LaneStats) -> Row {
    Row {
        lane: Lane {
            id: format!("l_{name}"),
            repo: PathBuf::from("/work/shop"),
            name: name.into(),
            path: PathBuf::from(format!("/work/shop.lanes/{name}")),
            base_commit: None,
            ports: PortBlock {
                start: port,
                count: 10,
            },
            agent: agent.map(Into::into),
            outcome: None,
            status: Status::Ready,
            created_at: 0,
        },
        details,
        stats: Some(stats),
        gone: false,
    }
}

fn tests(command: &str, passed: bool) -> Option<TestRun> {
    Some(TestRun {
        command: command.into(),
        passed,
        at: "2026-10-18T09:00:00Z".into(),
    })
}

fn main() {
    let mut args = std::env::args().skip(1).map(|a| a.parse::<u16>().ok());
    let w = args.next().flatten().unwrap_or(120);
    let h = args.next().flatten().unwrap_or(32);
    let clone = |install_ms| LaneStats {
        method: "clone".into(),
        files: 41_200,
        bytes: 1_400_000_000,
        made_ms: 620,
        install_ms,
    };
    let rows = vec![
        row(
            "fix-auth",
            4100,
            Some("claude"),
            Details {
                changed: 7,
                changed_bytes: 184_000,
                running: true,
                tests: tests("npm test", true),
            },
            clone(Some(52_000)),
        ),
        row(
            "codex-3f2a",
            4110,
            Some("codex"),
            Details {
                changed: 2,
                changed_bytes: 9_000,
                running: false,
                tests: tests("npm test", false),
            },
            clone(Some(52_000)),
        ),
        row(
            "cursor-checkout",
            4120,
            Some("cursor"),
            Details::default(),
            clone(Some(52_000)),
        ),
    ];
    let savings = insight::savings(
        &rows
            .iter()
            .map(|r| (&r.lane, r.stats.as_ref(), Some(&r.details)))
            .collect::<Vec<_>>(),
    );
    let keys = [None, Some(KeyCode::Char('?')), Some(KeyCode::Char(':'))];
    for key in keys {
        let mut app = lane_tui::home::Home::new(Input {
            version: "0.0.7",
            telemetry: true,
            telemetry_forced_off: None,
            theme: Theme::new(true, false),
            repo: Some(HomeRepo {
                name: "shop".into(),
                set_up: true,
                root: PathBuf::from("/work/shop"),
            }),
            snapshot: Snapshot {
                rows: rows.clone(),
                savings: savings.clone(),
                cores: 10,
                source: Source::StateFile,
            },
            agents_unset: Vec::new(),
        });
        if let Some(k) = key {
            app.on_key(KeyEvent::new(k, KeyModifiers::NONE));
        }
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        let buf = terminal.backend().buffer();
        println!("┌{}┐", "─".repeat(w as usize));
        for y in 0..h {
            let line: String = (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect();
            println!("│{line}│");
        }
        println!("└{}┘", "─".repeat(w as usize));
    }
}
