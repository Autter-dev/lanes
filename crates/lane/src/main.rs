//! `lane`: the Lanes command-line tool.

use lane_core::{PROGRESS_URL, VERSION, exit};
use std::process::ExitCode;

const HELP: &str = "\
lane: run many coding agents on one repo without them getting in each other's way

Usage: lane <command> [args]

Commands:
  init     Set up Lanes for this repo and the agents you use
  new      Create a lane
  list     List lanes with their ports and URLs
  exec     Run a command inside a lane
  rm       Remove a lane

Options:
  --json         Machine-readable output
  -h, --help     Show this help
  -V, --version  Show the version
";

const COMMANDS: [&str; 5] = ["init", "new", "list", "exec", "rm"];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.first().map(String::as_str) {
        None | Some("-h" | "--help" | "help") => {
            print!("{HELP}");
            ExitCode::from(exit::OK)
        }
        Some("-V" | "--version") => {
            println!("lane {VERSION} (by Autter)");
            ExitCode::from(exit::OK)
        }
        Some(cmd) if COMMANDS.contains(&cmd) => {
            eprintln!("lane {cmd}: not built yet. Follow progress at {PROGRESS_URL}");
            ExitCode::from(exit::NOT_IMPLEMENTED)
        }
        Some(other) => {
            eprintln!("lane: unknown command '{other}'. Run `lane --help` to see the commands.");
            ExitCode::from(exit::USAGE)
        }
    }
}
