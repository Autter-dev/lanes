//! Print every onboarding screen as text, using this machine's real checks.
//!   cargo run -p lane-tui --example preview [-- WIDTH HEIGHT]
//! Handy for reviewing layout without opening the full-screen app.

use lane_core::machine;
use lane_tui::onboarding::{Input, Onboarding};
use lane_tui::theme::Theme;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn main() {
    let mut args = std::env::args().skip(1).map(|a| a.parse::<u16>().ok());
    let w = args.next().flatten().unwrap_or(80);
    let h = args.next().flatten().unwrap_or(24);
    let dir = std::env::current_dir().unwrap();
    let checks = machine::run_all(&dir);
    let steps = [
        (0, 0),
        (1, 0),
        (2, 0),
        (2, 1),
        (2, 2),
        (2, 3),
        (3, 0),
        (4, 0),
        (5, 0),
    ];
    for (step, ex) in steps {
        let mut app = Onboarding::with_checks(
            Input {
                dir: dir.clone(),
                telemetry: true,
                telemetry_forced_off: None,
                animate: false,
                theme: Theme::new(true, false),
            },
            checks.clone(),
        );
        app.go_to(step, ex);
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        let buf = terminal.backend().buffer();
        println!("┌{}┐", "─".repeat(w as usize));
        for y in 0..h {
            let row: String = (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect();
            println!("│{row}│");
        }
        println!("└{}┘", "─".repeat(w as usize));
    }
}
