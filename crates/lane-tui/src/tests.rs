//! Every screen rendered with ratatui's test backend, plus the key flow end to end.

use crate::feed::{Row, Snapshot, Source};
use crate::home::{self, Home};
use crate::onboarding::{Input, Onboarding, Outcome};
use crate::theme::Theme;
use lane_core::insight::{Details, TestRun};
use lane_core::machine::{Check, Status};
use lane_core::state::{LaneRecord, LaneStats};
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
        agents: true,
        agents_found: vec!["Claude Code", "Codex"],
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
    assert_eq!(
        outcome,
        Some(Outcome::Finished {
            telemetry: true,
            agents: true
        })
    );
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
    assert!(text.contains("lane new fix-auth"));
    assert!(
        !text.contains("soon"),
        "every command on the ready screen is built"
    );

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
    let mut input = home_input(None, Vec::new());
    input.telemetry = true;
    let mut app = Home::new(input);
    let text = home_text(&app, 90, 26);
    assert!(text.contains("No lanes yet."));
    assert!(text.contains("lane doctor"));
    assert_eq!(app.on_key(key(KeyCode::Char('t'))), None);
    assert!(home_text(&app, 90, 26).contains("Install count off"));
    assert_eq!(
        app.on_key(key(KeyCode::Char('q'))),
        Some(home::Action::Exit(home::Outcome::Quit { telemetry: false }))
    );
    let mut input = home_input(None, Vec::new());
    input.telemetry = true;
    assert_eq!(
        Home::new(input).on_key(key(KeyCode::Char('s'))),
        Some(home::Action::Exit(home::Outcome::Setup { telemetry: true }))
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

fn home_input(repo: Option<home::HomeRepo>, rows: Vec<Row>) -> home::Input {
    home_input_with(repo, rows, &[])
}

/// The home screen's input, with the records of lanes made before (removed ones included).
fn home_input_with(
    repo: Option<home::HomeRepo>,
    rows: Vec<Row>,
    records: &[LaneRecord],
) -> home::Input {
    // No per-file cost here, so the numbers read the same on every system.
    let (savings, totals) = lane_core::snapshot::add_up(&rows, records, 0);
    home::Input {
        version: "0.0.7",
        telemetry: false,
        telemetry_forced_off: None,
        theme: Theme::new(true, false),
        repo,
        snapshot: Snapshot {
            rows,
            savings,
            totals,
            cores: 8,
            source: Source::StateFile,
        },
        agents_unset: Vec::new(),
    }
}

fn home_text(app: &Home, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    buffer_text(&terminal)
}

fn app_repo(set_up: bool) -> Option<home::HomeRepo> {
    Some(home::HomeRepo {
        name: "app".into(),
        set_up,
        root: PathBuf::from("/work/app"),
    })
}

fn a_lane(name: &str, port: u16) -> lane_core::state::Lane {
    lane_core::state::Lane {
        id: format!("l_{name}"),
        repo: PathBuf::from("/work/app"),
        name: name.into(),
        path: PathBuf::from(format!("/work/app.lanes/{name}")),
        base_commit: None,
        ports: lane_core::state::PortBlock {
            start: port,
            count: 10,
        },
        agent: None,
        outcome: None,
        status: lane_core::state::Status::Ready,
        created_at: 0,
    }
}

fn a_row(name: &str, port: u16) -> Row {
    Row {
        lane: a_lane(name, port),
        details: Details::default(),
        stats: None,
        gone: false,
    }
}

fn stats(method: &str, bytes: u64, install_ms: Option<u64>) -> LaneStats {
    LaneStats {
        method: method.into(),
        files: 1200,
        bytes,
        made_ms: 900,
        install_ms,
    }
}

/// Three lanes: Claude Code's, running with passing tests on a disk that clones; Codex's, with
/// failing tests; and one copied on a disk that can't clone.
fn busy_rows() -> Vec<Row> {
    let mut claude = a_row("fix-auth", 4100);
    claude.lane.agent = Some("claude".into());
    claude.details = Details {
        changed: 3,
        changed_bytes: 2_000_000,
        running: true,
        tests: Some(TestRun {
            command: "npm test".into(),
            passed: true,
            at: "2026-10-18T09:00:00Z".into(),
        }),
    };
    claude.stats = Some(stats("clone", 900_000_000, Some(48_000)));
    let mut codex = a_row("dark-mode", 4110);
    codex.lane.agent = Some("codex".into());
    codex.details = Details {
        changed: 1,
        changed_bytes: 10_000,
        running: false,
        tests: Some(TestRun {
            command: "cargo test".into(),
            passed: false,
            at: "2026-10-18T09:05:00Z".into(),
        }),
    };
    codex.stats = Some(stats("clone", 900_000_000, Some(48_000)));
    let mut copied = a_row("old-disk", 4120);
    copied.stats = Some(stats("copy", 300_000_000, None));
    vec![claude, codex, copied]
}

#[test]
fn home_lists_every_lane_with_its_agent_status_url_changes_tests_and_disk() {
    let app = Home::new(home_input(app_repo(true), busy_rows()));
    let text = home_text(&app, 120, 32);
    assert!(text.contains("Lanes in app"), "{text}");
    for header in [
        "LANE",
        "AGENT",
        "STATUS",
        "URL",
        "CHANGED",
        "TESTS",
        "EXTRA DISK",
    ] {
        assert!(text.contains(header), "{header} missing:\n{text}");
    }
    // A clone's extra disk: what it changed, plus each of its files' own record on the disk.
    let disk = |changed: u64| {
        lane_core::insight::bytes(lane_core::insight::lane_disk(
            &stats("clone", 900_000_000, Some(48_000)),
            changed,
            lane_core::insight::PER_FILE_BYTES,
        ))
    };
    let fix = text.lines().find(|l| l.contains("fix-auth")).unwrap();
    for cell in [
        "Claude Code",
        "running",
        "fix-auth.localhost:4100",
        "3 files",
        "✓ passed",
        &disk(2_000_000),
    ] {
        assert!(fix.contains(cell), "{cell} missing: {fix}");
    }
    let dark = text.lines().find(|l| l.contains("dark-mode")).unwrap();
    for cell in ["Codex", "ready", "1 file", "✗ failed", &disk(10_000)] {
        assert!(dark.contains(cell), "{cell} missing: {dark}");
    }
    let old = text.lines().find(|l| l.contains("old-disk")).unwrap();
    assert!(old.contains("300 MB (copy)"), "{old}");
    assert!(!text.contains("No lanes yet."));

    // A narrow terminal drops the agent and tests columns, and keeps the rest.
    let narrow = home_text(&app, 90, 32);
    assert!(
        !narrow.contains("AGENT") && !narrow.contains("TESTS"),
        "{narrow}"
    );
    assert!(narrow.contains("fix-auth.localhost:4100"), "{narrow}");
}

#[test]
fn home_shows_what_lanes_saved() {
    let app = Home::new(home_input(app_repo(true), busy_rows()));
    let text = home_text(&app, 120, 32);
    assert!(
        text.contains("Saved by lanes   1.8 GB of disk  ·  2 min of waiting"),
        "{text}"
    );
    // As worktrees: 900 MB + 900 MB + 300 MB. As lanes: 2 MB + 10 KB changed, and the copy.
    // Both bars are on one scale, so the lanes' bar is about a seventh of the worktrees'.
    let bar = |label: &str| {
        let line = text.lines().find(|l| l.contains(label)).unwrap();
        (line.matches('█').count(), line.to_string())
    };
    let (worktrees, line) = bar("As worktrees");
    assert!(line.contains("2.1 GB"), "{line}");
    let (lanes, line) = bar("As lanes");
    assert!(line.contains("302 MB"), "{line}");
    assert!(worktrees > 5 * lanes && lanes > 0, "{worktrees} vs {lanes}");
    assert!(
        text.contains("2 of 3 lanes didn't install again: at least 2 min saved"),
        "{text}"
    );
    assert!(
        text.contains("3 lanes with 30 ports of their own"),
        "{text}"
    );
    assert!(text.contains("1 lane is copied in full"), "{text}");
    // With no lanes removed yet, the totals would say the same again: not shown.
    assert!(!text.contains("In total"), "{text}");

    // Lanes made before Lanes measured them: nothing made up.
    let app = Home::new(home_input(app_repo(true), vec![a_row("fix-auth", 4100)]));
    let text = home_text(&app, 120, 32);
    assert!(text.contains("Make a lane with this version"), "{text}");
    assert!(!text.contains("As worktrees"), "{text}");
}

#[test]
fn home_keeps_what_removed_lanes_saved() {
    let rows = busy_rows();
    let mut records: Vec<LaneRecord> = rows
        .iter()
        .map(|r| LaneRecord {
            lane_id: r.lane.id.clone(),
            stats: r.stats.clone().unwrap(),
            at: 1_792_257_600,
            removed_at: None,
            extra_bytes: None,
        })
        .collect();
    // Two lanes done and removed: one measured as it went, one whose folder had already gone.
    for (id, extra) in [("ln_done1", Some(1_000_000)), ("ln_done2", None)] {
        records.push(LaneRecord {
            lane_id: id.into(),
            stats: stats("clone", 900_000_000, Some(48_000)),
            at: 1_792_300_000,
            removed_at: Some(1_792_400_000),
            extra_bytes: extra,
        });
    }
    let app = Home::new(home_input_with(app_repo(true), rows, &records));
    let text = home_text(&app, 120, 34);
    // Disk: the three lanes here and the one measured as it went. Time: four clones skipped a
    // 48 s install and took 0.9 s each to make.
    assert!(
        text.contains("In total      5 lanes since 2026-10-17, 2.7 GB not written, at least 3 min of waiting saved"),
        "{text}"
    );
}

#[test]
fn the_status_bar_says_what_is_true() {
    let app = Home::new(home_input(app_repo(true), busy_rows()));
    let text = home_text(&app, 140, 32);
    let bar = text
        .lines()
        .find(|l| l.contains("not signed in"))
        .unwrap_or_else(|| panic!("{text}"));
    for part in [
        "daemon not running yet: live from Lanes' state",
        "8 cores",
        "1 app running",
        "1.8 GB saved",
    ] {
        assert!(bar.contains(part), "{part} missing: {bar}");
    }
    // Narrower, it says the same in fewer words.
    let text = home_text(&app, 90, 32);
    let bar = text.lines().find(|l| l.contains("not signed in")).unwrap();
    assert!(bar.contains("1.8 GB saved"), "{bar}");
}

#[test]
fn question_mark_shows_the_keys_and_any_key_hides_them() {
    let mut app = Home::new(home_input(app_repo(true), busy_rows()));
    assert_eq!(app.on_key(key(KeyCode::Char('?'))), None);
    let text = home_text(&app, 120, 32);
    assert!(text.contains("Keys"), "{text}");
    assert!(text.contains("select a lane"), "{text}");
    assert!(
        text.contains("remove the selected lane (asks first)"),
        "{text}"
    );
    assert_eq!(app.on_key(key(KeyCode::Char('x'))), None);
    assert!(!home_text(&app, 120, 32).contains("select a lane"));
}

#[test]
fn the_palette_filters_commands_and_runs_one() {
    let mut app = Home::new(home_input(app_repo(true), busy_rows()));
    // Ctrl-K opens it, and doesn't move the selection the way k does.
    app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
    let text = home_text(&app, 120, 32);
    assert!(text.contains("Commands"), "{text}");
    assert!(text.contains("Quit"), "{text}");
    for c in "remove".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    let text = home_text(&app, 120, 32);
    assert!(text.contains("Remove the selected lane"), "{text}");
    assert!(!text.contains("Quit"), "{text}");
    // Enter runs it: removing asks first, and only y removes.
    assert_eq!(app.on_key(key(KeyCode::Enter)), None);
    assert!(home_text(&app, 120, 32).contains("Remove fix-auth and its folder"));
    assert_eq!(app.on_key(key(KeyCode::Char('n'))), None);
    assert!(!home_text(&app, 120, 32).contains("Remove fix-auth and its folder"));

    // `:` opens it too; Esc closes it.
    app.on_key(key(KeyCode::Char(':')));
    for c in "zzz".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    assert!(home_text(&app, 120, 32).contains("Nothing matches."));
    app.on_key(key(KeyCode::Esc));
    assert!(!home_text(&app, 120, 32).contains("Nothing matches."));
}

#[test]
fn removing_a_lane_asks_then_waits_for_it() {
    let mut app = Home::new(home_input(app_repo(true), busy_rows()));
    app.on_key(key(KeyCode::Char('j')));
    assert_eq!(app.on_key(key(KeyCode::Char('d'))), None);
    assert!(home_text(&app, 120, 32).contains("Remove dark-mode and its folder"));
    assert_eq!(
        app.on_key(key(KeyCode::Char('y'))),
        Some(home::Action::Remove("dark-mode".into()))
    );
    assert!(app.busy());
    assert!(home_text(&app, 120, 32).contains("Removing dark-mode…"));
    // q waits while it's at it, and so does making another; Ctrl-C still leaves.
    assert_eq!(app.on_key(key(KeyCode::Char('q'))), None);
    assert!(home_text(&app, 120, 32).contains("Lanes is still at it"));
    assert_eq!(app.on_key(key(KeyCode::Char('n'))), None);
    assert!(home_text(&app, 120, 32).contains("Wait for the lane being made or removed"));
    assert_eq!(
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Some(home::Action::Exit(home::Outcome::Quit { telemetry: false }))
    );
    app.done("Removed dark-mode and its folder.");
    assert!(!app.busy());
    assert!(home_text(&app, 120, 32).contains("Removed dark-mode and its folder."));
}

#[test]
fn n_names_a_new_lane() {
    let mut app = Home::new(home_input(app_repo(true), busy_rows()));
    app.on_key(key(KeyCode::Char('n')));
    assert!(home_text(&app, 120, 32).contains("New lane"));
    for c in "Fix Login!".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    assert!(home_text(&app, 120, 32).contains("lane new fix-login"));
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Some(home::Action::New("fix-login".into()))
    );
    assert!(home_text(&app, 120, 32).contains("Making fix-login…"));

    // An empty name isn't sent; outside a repo there's nowhere to make one.
    let mut app = Home::new(home_input(app_repo(true), Vec::new()));
    app.on_key(key(KeyCode::Char('n')));
    assert_eq!(app.on_key(key(KeyCode::Enter)), None);
    assert!(home_text(&app, 120, 32).contains("Use lowercase letters"));
    let mut app = Home::new(home_input(None, Vec::new()));
    app.on_key(key(KeyCode::Char('n')));
    assert!(home_text(&app, 120, 32).contains("Open Lanes in a repo"));
}

#[test]
fn the_selection_moves_and_follows_new_snapshots() {
    let many: Vec<Row> = (0..12)
        .map(|i| a_row(&format!("lane-{i}"), 4100 + i * 10))
        .collect();
    let mut app = Home::new(home_input(app_repo(true), many.clone()));
    for _ in 0..11 {
        app.on_key(key(KeyCode::Down));
    }
    // The table scrolls to keep the selected lane on screen.
    let text = home_text(&app, 100, 24);
    assert!(text.contains("▸ lane-11"), "{text}");
    assert!(!text.contains("lane-0 "), "{text}");

    // The same snapshot again changes nothing; a new one redraws, and the selection stays on a
    // lane that exists.
    let same = home_input(app_repo(true), many.clone()).snapshot;
    assert!(!app.update(same));
    let fewer = home_input(app_repo(true), many[..3].to_vec()).snapshot;
    assert!(app.update(fewer));
    assert!(home_text(&app, 100, 24).contains("▸ lane-2"));
}

#[test]
fn home_says_what_to_do_next() {
    let empty = |repo| home_text(&Home::new(home_input(repo, Vec::new())), 90, 26);
    assert!(empty(app_repo(false)).contains("This repo isn't set up yet: lane init"));
    assert!(empty(app_repo(true)).contains("Make your first lane: lane new fix-auth"));
    assert!(empty(None).contains("cd your-project"));
}

#[test]
fn the_last_screen_puts_agents_in_lanes_and_a_switches_it() {
    let mut app = at(5, 0);
    let text = screen_text(&app, 100, 30);
    assert!(
        text.contains("Agents in lanes: on, for Claude Code and Codex"),
        "{text}"
    );
    assert_eq!(app.on_key(key(KeyCode::Char('a'))), None);
    assert!(screen_text(&app, 100, 30).contains("Agents in lanes: off"));
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Some(Outcome::Finished {
            telemetry: true,
            agents: false
        })
    );
}

#[test]
fn home_says_when_an_agent_isnt_in_lanes_yet() {
    let mut input = home_input(None, Vec::new());
    input.agents_unset = vec!["Codex"];
    assert!(
        home_text(&Home::new(input), 100, 30).contains("Codex: not in lanes yet. Run lane setup")
    );
    let mut input = home_input(app_repo(true), busy_rows());
    input.agents_unset = vec!["Codex"];
    assert!(
        home_text(&Home::new(input), 120, 32).contains("Codex: not in lanes yet. Run lane setup")
    );
}
