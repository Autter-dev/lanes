//! Every screen rendered with ratatui's test backend, plus the key flow end to end.

use crate::home::{self, Home};
use crate::onboarding::{Input, Onboarding, Outcome};
use crate::theme::Theme;
use lane_core::machine::{Check, Status};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::PathBuf;

fn input(forced_off: Option<&'static str>) -> Input {
    Input {
        dir: PathBuf::from("/work/acme"),
        telemetry: true,
        telemetry_forced_off: forced_off,
        animate: false,
        theme: Theme::new(true, false),
    }
}

fn check(
    id: &'static str,
    label: &'static str,
    status: Status,
    summary: &str,
    hint: Option<&str>,
) -> Check {
    Check {
        id,
        label,
        status,
        summary: summary.into(),
        hint: hint.map(Into::into),
        fix: None,
    }
}

fn sample_checks() -> Vec<Check> {
    vec![
        check("os", "System", Status::Ok, "Windows · x86_64", None),
        check(
            "copies",
            "Instant lanes",
            Status::Warn,
            "NTFS: NTFS can't clone files, so lanes are copied",
            Some("Move the repo to a Dev Drive for instant lanes."),
        ),
        check("git", "Git", Status::Ok, "git 2.51.0", None),
        check(
            "repo",
            "This folder",
            Status::Ok,
            "the Git repo acme on main",
            None,
        ),
        check(
            "stack",
            "Your stack",
            Status::Ok,
            "pnpm, Docker Compose",
            None,
        ),
        check(
            "agents",
            "Agents",
            Status::Ok,
            "Claude Code, Codex",
            Some("Lanes routes these into lanes."),
        ),
        check("disk", "Free disk", Status::Ok, "212 GB free", None),
    ]
}

fn screen_text(app: &Onboarding, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    buffer_text(&terminal)
}

fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
    let buf = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn at(step: usize, explainer: usize) -> Onboarding {
    let mut app = Onboarding::with_checks(input(None), sample_checks());
    app.go_to(step, explainer);
    app
}

#[test]
fn every_screen_renders_at_both_sizes() {
    let expect: [(usize, usize, &str); 9] = [
        (0, 0, "Every agent gets its own lane."),
        (1, 0, "Instant lanes"),
        (2, 0, "Your repo"),
        (2, 1, "A lane per agent"),
        (2, 2, "both editing validateSession()"),
        (2, 3, "● PR"),
        (3, 0, "The anonymous install count"),
        (4, 0, "Continue without signing in"),
        (5, 0, "You're set."),
    ];
    for (step, ex, needle) in expect {
        for (w, h) in [(100, 30), (80, 24), (60, 18)] {
            let text = screen_text(&at(step, ex), w, h);
            assert!(
                text.contains(needle),
                "step {step}.{ex} at {w}x{h} is missing {needle:?}:\n{text}"
            );
            assert!(text.contains("Lanes."), "header at {w}x{h}");
        }
    }
}

#[test]
fn wide_explainer_shows_urls_and_the_pr_label() {
    assert!(screen_text(&at(2, 1), 100, 30).contains("fix-auth.localhost :3001"));
    assert!(screen_text(&at(2, 3), 100, 30).contains("one clean pull request"));
}

#[test]
fn machine_screen_shows_symbols_and_hints() {
    let text = screen_text(&at(1, 0), 100, 30);
    assert!(text.contains("✓ System"));
    assert!(text.contains("! Instant lanes"));
    assert!(text.contains("Dev Drive"), "warnings carry what to do next");
    assert!(text.contains("1 thing worth knowing"));
}

#[test]
fn small_terminals_get_a_message_not_a_broken_layout() {
    let text = screen_text(&at(0, 0), 50, 15);
    assert!(text.contains("Make the window a little bigger"));
}

#[test]
fn sign_in_is_shown_but_disabled() {
    let mut app = at(4, 0);
    let text = screen_text(&app, 80, 24);
    assert!(text.contains("Sign in to Autter"));
    assert!(text.contains("available when the gateway opens"));
    assert_eq!(app.on_key(key(KeyCode::Down)), None);
    assert!(screen_text(&app, 80, 24).contains("Sign-in isn't available yet"));
}

#[test]
fn enter_walks_through_to_finished() {
    let mut app = Onboarding::with_checks(input(None), sample_checks());
    let mut outcome = None;
    for _ in 0..20 {
        outcome = app.on_key(key(KeyCode::Enter));
        if outcome.is_some() {
            break;
        }
    }
    assert_eq!(outcome, Some(Outcome::Finished { telemetry: true }));
}

#[test]
fn back_returns_to_the_last_explainer_step() {
    let mut app = at(3, 0);
    app.on_key(key(KeyCode::Left));
    assert!(screen_text(&app, 80, 24).contains("One clean PR"));
}

#[test]
fn privacy_toggles_and_respects_opt_outs() {
    let mut app = at(3, 0);
    app.on_key(key(KeyCode::Char(' ')));
    assert!(!app.telemetry());
    assert!(screen_text(&app, 80, 24).contains("install count: Off"));

    let mut forced = Onboarding::with_checks(input(Some("DO_NOT_TRACK is set")), sample_checks());
    forced.go_to(3, 0);
    assert!(
        !forced.telemetry(),
        "an environment opt-out wins over the default"
    );
    forced.on_key(key(KeyCode::Char(' ')));
    assert!(!forced.telemetry());
    assert!(screen_text(&forced, 100, 30).contains("DO_NOT_TRACK is set"));
}

#[test]
fn quit_saves_nothing() {
    let mut app = at(2, 1);
    assert_eq!(app.on_key(key(KeyCode::Char('q'))), Some(Outcome::Quit));
    let mut app = at(0, 0);
    assert_eq!(
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Some(Outcome::Quit)
    );
}

#[test]
fn help_opens_and_closes() {
    let mut app = at(1, 0);
    app.on_key(key(KeyCode::Char('?')));
    assert!(screen_text(&app, 80, 24).contains("Keys"));
    assert_eq!(
        app.on_key(key(KeyCode::Esc)),
        None,
        "Esc closes help instead of quitting"
    );
    assert!(!screen_text(&app, 80, 24).contains("show or hide this help"));
}

#[test]
fn ready_screen_tailors_next_steps() {
    let text = screen_text(&at(5, 0), 100, 30);
    assert!(text.contains("Next, in this repo:"));
    assert!(text.contains("  soon"));

    let mut checks = sample_checks();
    checks[3] = check(
        "repo",
        "This folder",
        Status::Info,
        "not inside a Git repo",
        None,
    );
    let mut app = Onboarding::with_checks(input(None), checks);
    app.go_to(5, 0);
    assert!(screen_text(&app, 100, 30).contains("cd your-project"));
}

#[test]
fn home_renders_and_switches_the_install_count() {
    let mut app = Home::new(home::Input {
        version: "0.0.1",
        telemetry: true,
        telemetry_forced_off: None,
        theme: Theme::new(true, false),
    });
    let mut terminal = Terminal::new(TestBackend::new(90, 26)).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("No lanes yet."));
    assert!(text.contains("lane setup"));
    assert_eq!(app.on_key(key(KeyCode::Char('t'))), None);
    assert_eq!(
        app.on_key(key(KeyCode::Char('q'))),
        Some(home::Outcome::Quit { telemetry: false })
    );
    assert_eq!(
        Home::new(home::Input {
            version: "0.0.1",
            telemetry: true,
            telemetry_forced_off: None,
            theme: Theme::new(true, false)
        })
        .on_key(key(KeyCode::Char('s'))),
        Some(home::Outcome::Setup { telemetry: true })
    );
}

#[test]
fn the_otter_shows_up_where_autter_does() {
    assert!(screen_text(&at(0, 0), 100, 30).contains("by Autter"));
    let wide = screen_text(&at(4, 0), 110, 30);
    assert!(
        wide.contains("_[___]_"),
        "captain's hat in wide terminals:\n{wide}"
    );
    assert!(
        screen_text(&at(4, 0), 80, 24).contains("=(o.o)="),
        "inline otter when narrow"
    );
    let ready = screen_text(&at(5, 0), 100, 30);
    assert!(
        ready.contains("=(o.o)=") && ready.contains("✓"),
        "the otter reaches the finish:\n{ready}"
    );
}

#[test]
fn privacy_lane_is_cut_when_the_count_is_off() {
    let mut app = at(3, 0);
    assert!(screen_text(&app, 100, 30).contains("▸  install count"));
    app.on_key(key(KeyCode::Char(' ')));
    assert!(screen_text(&app, 100, 30).contains("✕"));
}

#[test]
fn step_marker_sits_on_the_current_stop_when_still() {
    let theme = Theme::new(true, false);
    let lane: String = crate::widgets::step_lane(&theme, 2, 6, 3, None)
        .spans
        .iter()
        .map(|s| s.content.to_string())
        .collect();
    assert_eq!(lane, "●───●───◉───○───○───○");
    let mid: String = crate::widgets::step_lane(&theme, 2, 6, 3, Some((1, 0.1)))
        .spans
        .iter()
        .map(|s| s.content.to_string())
        .collect();
    assert_ne!(mid, lane, "mid-glide the marker is between stops");
}

#[test]
fn welcome_shows_the_big_wordmark_when_there_is_room() {
    let tall = screen_text(&at(0, 0), 80, 24);
    assert!(tall.contains("██▄▄▄"), "big letters at 80x24:\n{tall}");
    let short = screen_text(&at(0, 0), 80, 19);
    assert!(
        !short.contains("██▄▄▄"),
        "falls back to one line when short"
    );
    assert!(short.contains("Every agent gets its own lane."));
}

fn with_fixes() -> Onboarding {
    use lane_core::machine::{Fix, Settings};
    let mut checks = sample_checks();
    checks[1].fix = Some(Fix {
        title: "Make lanes instant with a Dev Drive".into(),
        steps: vec![
            "Open Settings › System › Storage › Disks & volumes.".into(),
            "Choose Create Dev Drive.".into(),
        ],
        command: None,
        open: Some(Settings {
            label: "Disks & volumes",
            uri: "ms-settings:disksandvolumes",
        }),
    });
    checks[2] = check(
        "git",
        "Git",
        Status::Fail,
        "Git isn't installed or isn't on PATH",
        Some("Install Git, then check again."),
    );
    checks[2].fix = Some(Fix {
        title: "Install Git".into(),
        steps: vec![
            "Run this in another terminal window:".into(),
            "Then open a new terminal.".into(),
        ],
        command: Some("winget install --id Git.Git -e --source winget".into()),
        open: None,
    });
    let mut app = Onboarding::with_checks(input(None), checks);
    app.go_to(1, 0);
    app
}

#[test]
fn problems_offer_a_fix_and_arrows_choose_one() {
    let mut app = with_fixes();
    let text = screen_text(&app, 100, 30);
    assert!(text.contains("› Make lanes instant with a Dev Drive   f to fix"));
    assert!(
        text.contains("  Install Git"),
        "the other fix is listed, not chosen"
    );
    assert!(text.contains("f fix it"));
    app.on_key(key(KeyCode::Down));
    let text = screen_text(&app, 100, 30);
    assert!(text.contains("› Install Git"));
    app.on_key(key(KeyCode::Down));
    assert!(
        screen_text(&app, 100, 30).contains("› Make lanes instant"),
        "wraps around"
    );
}

#[test]
fn the_fix_card_shows_steps_and_the_command_in_place() {
    let mut app = with_fixes();
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.on_key(key(KeyCode::Char('f'))), None);
    let text = screen_text(&app, 100, 30);
    let step1 = text
        .find(" 1  Run this in another terminal window:")
        .expect(&text);
    let cmd = text.find("$ winget install --id Git.Git").expect(&text);
    let step2 = text.find(" 2  Then open a new terminal.").expect(&text);
    assert!(
        step1 < cmd && cmd < step2,
        "the command sits under the step that asks for it"
    );
    assert!(text.contains("Lanes never runs this for you"));
    assert!(text.contains("c copy command"));
    assert!(text.contains("r check again"));
    // Esc closes the card instead of quitting.
    assert_eq!(app.on_key(key(KeyCode::Esc)), None);
    assert!(screen_text(&app, 100, 30).contains("Checking this machine"));
}

#[test]
fn a_settings_fix_offers_to_open_the_page() {
    let mut app = with_fixes();
    app.on_key(key(KeyCode::Char('f')));
    let text = screen_text(&app, 100, 30);
    assert!(text.contains("o open Disks & volumes"));
    assert!(!text.contains("c copy command"), "no command, no copy key");
    for (w, h) in [(80, 24), (60, 18)] {
        let text = screen_text(&app, w, h);
        assert!(
            text.contains("Make lanes instant with a Dev Drive"),
            "{w}x{h}:\n{text}"
        );
    }
}

#[test]
fn checking_again_runs_live_and_reports() {
    let mut app = with_fixes();
    app.on_key(key(KeyCode::Char('f')));
    app.on_key(key(KeyCode::Char('r')));
    assert!(app.animating(), "keeps drawing while the check runs");
    assert!(screen_text(&app, 100, 30).contains("checking again"));
    let start = std::time::Instant::now();
    while app.animating() && start.elapsed().as_secs() < 30 {
        app.tick();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(!app.animating(), "the re-check came back");
    let text = screen_text(&app, 100, 30);
    // The real check replaced the fake one, so something changed either way.
    assert!(text.contains('✓') || text.contains('!'), "{text}");
}

#[test]
fn enter_still_moves_on_from_the_machine_step() {
    let mut app = with_fixes();
    app.on_key(key(KeyCode::Enter));
    assert!(screen_text(&app, 100, 30).contains("How a lane works"));
}
