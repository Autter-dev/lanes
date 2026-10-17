//! `lane`: the Lanes command-line tool.

use lane_core::config::Config;
use lane_core::env::Env;
use lane_core::machine::{self, Check};
use lane_core::{PROGRESS_URL, VERSION, exit};
use lane_tui::theme::Theme;
use lane_tui::{home, onboarding};
use std::path::PathBuf;
use std::process::ExitCode;

const HELP: &str = "\
lane: run many coding agents on one repo without them getting in each other's way

Usage: lane [command] [args]

Run `lane` on its own in a terminal to open the Lanes app (a short welcome on the first run).

Commands:
  setup    Check this machine and choose your settings
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

const SETUP_HELP: &str = "\
lane setup: check this machine and choose your settings

Usage: lane setup [--json] [--yes] [--telemetry on|off]

In a terminal, `lane setup` opens the welcome: your machine, how a lane works, privacy and sign-in.

Options:
  --json               Print the machine checks and settings as JSON; changes nothing unless --yes is given
  --yes                Save the settings without the terminal app (for scripts, CI and agents)
  --telemetry on|off   With --yes: turn the anonymous install count on or off
  -h, --help           Show this help
";

const COMMANDS: [&str; 5] = ["init", "new", "list", "exec", "rm"];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let env = Env::current();

    match args.first().map(String::as_str) {
        None => open_app(&env),
        Some("-h" | "--help" | "help") => {
            print!("{HELP}");
            ExitCode::from(exit::OK)
        }
        Some("-V" | "--version") => {
            println!("lane {VERSION} (by Autter)");
            ExitCode::from(exit::OK)
        }
        Some("setup") => setup(&args[1..], &env),
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

fn fail(msg: impl std::fmt::Display) -> ExitCode {
    eprintln!("lane: {msg}");
    ExitCode::from(exit::FAILURE)
}

fn here() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn theme(env: &Env) -> Theme {
    Theme::new(env.no_color(), env.truecolor())
}

/// `lane` on its own: the onboarding on the first run, then the home screen.
fn open_app(env: &Env) -> ExitCode {
    if env.interactive().is_err() {
        print!("{HELP}");
        return ExitCode::from(exit::OK);
    }
    let mut config = match Config::load() {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    if !config.is_onboarded() {
        match run_onboarding(env, &mut config) {
            Ok(true) => {}
            Ok(false) => {
                println!(
                    "Setup skipped; nothing was saved. Run `lane` or `lane setup` when you're ready."
                );
                return ExitCode::from(exit::OK);
            }
            Err(code) => return code,
        }
    }
    run_home(env, config)
}

/// Run the onboarding and save on finish. `Ok(true)` when finished, `Ok(false)` when the user left.
fn run_onboarding(env: &Env, config: &mut Config) -> Result<bool, ExitCode> {
    let input = onboarding::Input {
        dir: here(),
        telemetry: config.telemetry,
        telemetry_forced_off: env.telemetry_forced_off(),
        animate: !env.no_animation(),
        theme: theme(env),
    };
    match lane_tui::run_onboarding(input) {
        Ok(onboarding::Outcome::Finished { telemetry }) => {
            config.onboarded = Some(VERSION.to_string());
            if env.telemetry_forced_off().is_none() {
                config.telemetry = telemetry;
            }
            config.save().map_err(fail)?;
            Ok(true)
        }
        Ok(onboarding::Outcome::Quit) => Ok(false),
        Err(e) => Err(fail(format!(
            "the terminal app couldn't start ({e}). Run `lane setup --yes` to set up without it."
        ))),
    }
}

fn run_home(env: &Env, mut config: Config) -> ExitCode {
    loop {
        let input = home::Input {
            version: VERSION,
            telemetry: config.telemetry,
            telemetry_forced_off: env.telemetry_forced_off(),
            theme: theme(env),
        };
        let (outcome, telemetry) = match lane_tui::run_home(input) {
            Ok(home::Outcome::Quit { telemetry }) => (None, telemetry),
            Ok(home::Outcome::Setup { telemetry }) => (Some(()), telemetry),
            Err(e) => {
                return fail(format!(
                    "the terminal app couldn't start ({e}). Try `lane --help`."
                ));
            }
        };
        if env.telemetry_forced_off().is_none() && telemetry != config.telemetry {
            config.telemetry = telemetry;
            if let Err(e) = config.save() {
                return fail(e);
            }
        }
        match outcome {
            None => return ExitCode::from(exit::OK),
            Some(()) => {
                if let Err(code) = run_onboarding(env, &mut config) {
                    return code;
                }
            }
        }
    }
}

struct SetupFlags {
    json: bool,
    yes: bool,
    telemetry: Option<bool>,
}

fn parse_setup(args: &[String]) -> Result<Option<SetupFlags>, String> {
    let mut flags = SetupFlags {
        json: false,
        yes: false,
        telemetry: None,
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--json" => flags.json = true,
            "--yes" | "-y" => flags.yes = true,
            "--telemetry" => {
                flags.telemetry = Some(match it.next().map(String::as_str) {
                    Some("on") => true,
                    Some("off") => false,
                    _ => return Err("--telemetry takes `on` or `off`".into()),
                })
            }
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    if flags.telemetry.is_some() && !flags.yes {
        return Err("--telemetry only works with --yes".into());
    }
    Ok(Some(flags))
}

fn setup(args: &[String], env: &Env) -> ExitCode {
    let flags = match parse_setup(args) {
        Ok(Some(f)) => f,
        Ok(None) => {
            print!("{SETUP_HELP}");
            return ExitCode::from(exit::OK);
        }
        Err(e) => {
            eprintln!("lane setup: {e}. Run `lane setup --help` to see the options.");
            return ExitCode::from(exit::USAGE);
        }
    };
    let mut config = match Config::load() {
        Ok(c) => c,
        Err(e) => return fail(e),
    };

    if flags.yes {
        config.onboarded = Some(VERSION.to_string());
        if let Some(t) = flags.telemetry {
            config.telemetry = t;
        }
        if let Err(e) = config.save() {
            return fail(e);
        }
    }

    if flags.json {
        let checks = machine::run_all(&here());
        let report = serde_json::json!({
            "version": VERSION,
            "dir": here(),
            "checks": checks,
            "settings": {
                "onboarded": config.onboarded,
                "telemetry": config.telemetry && env.telemetry_forced_off().is_none(),
                "telemetry_setting": config.telemetry,
                "telemetry_forced_off": env.telemetry_forced_off(),
                "saved": flags.yes,
            },
            "config_path": lane_core::config::config_path().ok(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("report serializes")
        );
        return ExitCode::from(exit::OK);
    }

    if !flags.yes && env.interactive().is_ok() {
        return match run_onboarding(env, &mut config) {
            Ok(true) => {
                println!("Saved. Run `lane` any time to open Lanes.");
                ExitCode::from(exit::OK)
            }
            Ok(false) => {
                println!("Setup closed; nothing was changed.");
                ExitCode::from(exit::OK)
            }
            Err(code) => code,
        };
    }

    // No terminal app: print the checks as plain text.
    print_checks(&machine::run_all(&here()));
    println!();
    let tele = if config.telemetry && env.telemetry_forced_off().is_none() {
        "on"
    } else {
        "off"
    };
    if flags.yes {
        let path = lane_core::config::config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        println!("Saved: install count {tele}. Settings are in {path}.");
    } else {
        println!("Install count: {tele}. Nothing was changed.");
        println!(
            "Run `lane setup --yes` to save these settings, or `lane setup` in a terminal for the full welcome."
        );
    }
    ExitCode::from(exit::OK)
}

fn print_checks(checks: &[Check]) {
    for c in checks {
        println!("{} {:<15}{}", c.status.symbol(), c.label, c.summary);
        if let Some(hint) = c
            .hint
            .as_deref()
            .filter(|_| c.status != machine::Status::Ok)
        {
            println!("  {:<15}{hint}", "");
        }
    }
}
