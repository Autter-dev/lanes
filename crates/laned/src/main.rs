//! `laned`: the Lanes background daemon. One per user; `lane` starts it when needed.

use lane_core::{PROGRESS_URL, VERSION, exit};
use std::process::ExitCode;

fn main() -> ExitCode {
    let arg = std::env::args().nth(1);

    match arg.as_deref() {
        Some("-V" | "--version") => {
            println!("laned {VERSION} (by Autter)");
            ExitCode::from(exit::OK)
        }
        _ => {
            eprintln!("laned: the daemon isn't built yet. Follow progress at {PROGRESS_URL}");
            ExitCode::from(exit::NOT_IMPLEMENTED)
        }
    }
}
