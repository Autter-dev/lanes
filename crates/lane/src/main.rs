//! `lane`: the Lanes command-line tool.
//!
//! The CLI contract, for scripts and agents:
//! - `--json` prints exactly one JSON object on stdout: the result, or `{"error": {...}}` with
//!   `code`, `message`, `hint` and `exit` (see `lane_core::error`). Nothing else goes to stdout.
//! - Exit codes come from `lane_core::exit` and never change meaning.
//! - Nothing prompts or opens a pager unless stdout is a terminal and `--json` isn't given.
//! - Writes take `--request-id <id>` (or `LANE_REQUEST_ID`): a retry with the same id returns
//!   the first result and changes nothing.

mod args;

use lane_core::config::Config;
use lane_core::engine::{self, Made, Repo};
use lane_core::env::Env;
use lane_core::error::{Error, Result};
use lane_core::machine::{self, Check};
use lane_core::state::{Lane, Status, Store};
use lane_core::{VERSION, exit};
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
  list     List lanes with their ports and folders
  exec     Run a command inside a lane
  rm       Remove a lane
  adopt    Turn a worktree you already have into a lane

Options:
  --json             Machine-readable output: one JSON object on stdout, errors included
  --request-id ID    Make a write safe to retry: the same id never runs twice (or LANE_REQUEST_ID)
  -h, --help         Show this help
  -V, --version      Show the version
";

const LIST_HELP: &str = "lane list: list lanes with their ports and folders

Usage: lane list [--all] [--json]

Lists the lanes of the repo you're in, oldest first.

Options:
  --all, -a    List lanes in every repo on this machine
  --json       Print {\"repo\": ..., \"lanes\": [...]} instead of a table
  -h, --help   Show this help
";

const NEW_HELP: &str = "\
lane new: make a lane, an instant, ready-to-run copy of this repo

Usage: lane new <name> [--agent NAME] [--print-path] [--json] [--request-id ID]

The lane starts from the repo's base branch (main) with dependencies already installed, gets
its own block of ports, and is a normal Git worktree: `git status` and `git diff` work there.
No branch is created. Names use lowercase letters, digits and hyphens, like `fix-auth`.

Options:
  --agent NAME     Record which agent works in it (claude, codex, ...)
  --print-path     Print only the lane's folder (for agent hooks); notes go to stderr
  --json           Print the lane, the base and how its files were copied
  --request-id ID  Make a retry safe: the same id never makes a second lane
  -h, --help       Show this help
";

const RM_HELP: &str = "\
lane rm: remove a lane

Usage: lane rm <name> [--keep-files] [--json] [--request-id ID]

Removes the lane's folder (changes and all) and its worktree record, and frees its ports.

Options:
  --keep-files     Forget the lane but leave its folder where it is
  --json           Print the lane that was removed
  --request-id ID  Make a retry safe
  -h, --help       Show this help
";

const ADOPT_HELP: &str = "\
lane adopt: turn a worktree you already have into a lane

Usage: lane adopt [<path>] [--name NAME] [--agent NAME] [--json] [--request-id ID]

For worktrees made by `git worktree add` or by an agent's own tool. The folder stays where it
is; Lanes gives it a block of ports and lists it. Adopting a folder twice is safe. <path>
defaults to the folder you're in.

Options:
  --name NAME      The lane's name (default: from the folder's name)
  --agent NAME     Record which agent works in it
  --json           Print the lane
  --request-id ID  Make a retry safe
  -h, --help       Show this help
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

/// Commands that are listed but not built yet.
const COMING: [&str; 2] = ["init", "exec"];

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
        Some("list") => run("list", &args[1..], list),
        Some("new") => run("new", &args[1..], new),
        Some("rm") => run("rm", &args[1..], rm),
        Some("adopt") => run("adopt", &args[1..], adopt),
        Some(cmd) if COMING.contains(&cmd) => report(
            Some(cmd),
            &Error::not_implemented(cmd),
            args::wants_json(&args),
        ),
        Some(other) => report(
            None,
            &Error::usage(format!("unknown command `{other}`"))
                .hint("Run `lane --help` to see the commands."),
            args::wants_json(&args),
        ),
    }
}

/// Run a command and turn its error, if any, into output and an exit code.
fn run(name: &str, args: &[String], command: fn(&[String]) -> Result<()>) -> ExitCode {
    match command(args) {
        Ok(()) => ExitCode::from(exit::OK),
        Err(e) => report(Some(name), &e, args::wants_json(args)),
    }
}

/// Print an error: one JSON object on stdout with `--json`, otherwise a line on stderr.
fn report(command: Option<&str>, e: &Error, json: bool) -> ExitCode {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&e.to_json()).expect("errors serialize")
        );
    } else {
        match command {
            Some(c) => eprintln!("lane {c}: {e}"),
            None => eprintln!("lane: {e}"),
        }
    }
    ExitCode::from(e.exit_code())
}

fn print_json(value: &serde_json::Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).expect("output serializes")
    );
}

/// `lane list`: the lanes of this repo (or every repo), from the state store. Writes nothing.
fn list(args: &[String]) -> Result<()> {
    let (common, rest) = args::split(args)?;
    let mut all = false;
    for arg in &rest {
        match arg.as_str() {
            "--all" | "-a" => all = true,
            "-h" | "--help" => {
                print!("{LIST_HELP}");
                return Ok(());
            }
            other => {
                return Err(Error::usage(format!("unknown option `{other}`"))
                    .hint("Run `lane list --help` to see the options."));
            }
        }
    }
    let repo = if all {
        None
    } else {
        Some(Repo::open(&here())?.root)
    };
    let lanes = match Store::open_existing()? {
        Some(store) => store.lanes(repo.as_deref())?,
        None => Vec::new(),
    };
    if common.json {
        print_json(&serde_json::json!({ "repo": repo, "lanes": lanes }));
        return Ok(());
    }
    if lanes.is_empty() {
        match &repo {
            Some(r) => println!("No lanes in {} yet.", r.display()),
            None => println!("No lanes yet."),
        }
        println!("Make one with `lane new <name>`, like `lane new fix-auth`.");
        return Ok(());
    }
    let name_w = lanes.iter().map(|l| l.name.len()).max().unwrap_or(4).max(4);
    println!(
        "{:<name_w$}  {:<11}  {:<10}  PATH",
        "NAME", "PORTS", "STATE"
    );
    for l in &lanes {
        let ports = format!("{}-{}", l.ports.start, l.ports.end());
        let state = match (l.status, l.outcome) {
            (Status::Creating, _) => "creating".to_string(),
            (Status::Ready, None) => "ready".to_string(),
            (Status::Ready, Some(o)) => serde_json::to_value(o)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default(),
        };
        println!(
            "{:<name_w$}  {ports:<11}  {state:<10}  {}",
            l.name,
            l.path.display()
        );
    }
    Ok(())
}

fn unknown_option(command: &str, option: &str) -> Error {
    Error::usage(format!("unknown option `{option}`"))
        .hint(format!("Run `lane {command} --help` to see the options."))
}

fn value(it: &mut std::slice::Iter<'_, String>, flag: &str, example: &str) -> Result<String> {
    it.next().cloned().ok_or_else(|| {
        Error::usage(format!("{flag} needs a value"))
            .hint(format!("Pass one, like `{flag} {example}`."))
    })
}

/// 13822 â†’ "13,822".
fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn ports(l: &Lane) -> String {
    format!("{}-{}", l.ports.start, l.ports.end())
}

/// A note on stderr when a lane was copied rather than cloned: it works, but slower.
fn copied_note(made: &Made) {
    if made.clone.copied > 0 {
        let fs = match made.clone.filesystem.as_deref() {
            Some("ntfs") => "NTFS",
            Some("refs") => "ReFS",
            Some("apfs") => "APFS",
            Some("btrfs") => "Btrfs",
            Some("xfs") => "XFS",
            Some("ext4") => "ext4",
            Some(other) => other,
            None => "This disk",
        };
        eprintln!(
            "lane new: {fs} can't clone files, so {} files were copied ({:.1} s, {} MB). On a disk that clones, lanes are instant and nearly free; `lane setup` shows how.",
            thousands(made.clone.copied),
            made.clone.millis as f64 / 1000.0,
            made.clone.bytes / 1_000_000,
        );
    }
}

/// `lane new <name>`: make a lane from `_base`.
fn new(args: &[String]) -> Result<()> {
    let (common, rest) = args::split(args)?;
    let (mut name, mut agent, mut print_path) = (None, None, false);
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{NEW_HELP}");
                return Ok(());
            }
            "--print-path" => print_path = true,
            "--agent" => agent = Some(value(&mut it, "--agent", "claude")?),
            s if s.starts_with('-') => return Err(unknown_option("new", s)),
            s if name.is_none() => name = Some(s.to_string()),
            s => {
                return Err(Error::usage(format!("unexpected `{s}`"))
                    .hint("`lane new` takes one name, like `lane new fix-auth`."));
            }
        }
    }
    let name = name.ok_or_else(|| {
        Error::usage("`lane new` needs a name").hint("Name the lane, like `lane new fix-auth`.")
    })?;
    let started = std::time::Instant::now();
    let repo = Repo::open(&here())?;
    let mut store = Store::open()?;
    let made = engine::new_lane(
        &mut store,
        &repo,
        &name,
        agent.as_deref(),
        common.request_id.as_deref(),
    )?;
    let total = started.elapsed().as_secs_f64();
    if common.json {
        print_json(&serde_json::to_value(&made).expect("lanes serialize"));
        return Ok(());
    }
    if print_path {
        println!("{}", made.lane.path.display());
        copied_note(&made);
        return Ok(());
    }
    let how = if made.clone.copied == 0 {
        "cloned"
    } else {
        "copied"
    };
    println!(
        "Made lane {} in {total:.1} s ({} files {how} in {:.1} s).",
        made.lane.name,
        thousands(made.clone.files),
        made.clone.millis as f64 / 1000.0,
    );
    println!("  folder  {}", made.lane.path.display());
    println!("  ports   {}", ports(&made.lane));
    let mut base = format!(
        "{} at {}",
        made.base.branch,
        &made.base.commit[..made.base.commit.len().min(7)]
    );
    if made.base.created {
        base.push_str(", set up just now");
    } else if made.base.moved {
        base.push_str(", moved to its latest commit");
    }
    if made.base.installed {
        base.push_str("; dependencies installed");
    }
    println!("  base    {base}");
    if made.fixed_paths > 0 {
        println!("  fixed   {} files that pointed at _base", made.fixed_paths);
    }
    println!("Start working: cd {}", made.lane.path.display());
    copied_note(&made);
    Ok(())
}

/// `lane rm <name>`: remove a lane's folder, worktree record and ports.
fn rm(args: &[String]) -> Result<()> {
    let (common, rest) = args::split(args)?;
    let (mut name, mut keep_files) = (None, false);
    for arg in &rest {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{RM_HELP}");
                return Ok(());
            }
            "--keep-files" => keep_files = true,
            s if s.starts_with('-') => return Err(unknown_option("rm", s)),
            s if name.is_none() => name = Some(s.to_string()),
            s => {
                return Err(Error::usage(format!("unexpected `{s}`"))
                    .hint("`lane rm` takes one name, like `lane rm fix-auth`."));
            }
        }
    }
    let name = name.ok_or_else(|| {
        Error::usage("`lane rm` needs a name")
            .hint("Run `lane list` to see the lanes you can remove.")
    })?;
    let repo = Repo::open(&here())?;
    let mut store = Store::open()?;
    let lane = engine::remove(
        &mut store,
        &repo,
        &name,
        keep_files,
        common.request_id.as_deref(),
    )?;
    if common.json {
        print_json(&serde_json::json!({ "removed": lane }));
    } else if keep_files {
        println!(
            "Forgot lane {}; its folder is still at {}. Ports {} are free.",
            lane.name,
            lane.path.display(),
            ports(&lane)
        );
    } else {
        println!(
            "Removed lane {} and its folder. Ports {} are free.",
            lane.name,
            ports(&lane)
        );
    }
    Ok(())
}

/// `lane adopt [<path>]`: make an existing worktree a lane.
fn adopt(args: &[String]) -> Result<()> {
    let (common, rest) = args::split(args)?;
    let (mut path, mut name, mut agent) = (None, None, None);
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{ADOPT_HELP}");
                return Ok(());
            }
            "--name" => name = Some(value(&mut it, "--name", "fix-auth")?),
            "--agent" => agent = Some(value(&mut it, "--agent", "claude")?),
            s if s.starts_with('-') => return Err(unknown_option("adopt", s)),
            s if path.is_none() => path = Some(PathBuf::from(s)),
            s => {
                return Err(Error::usage(format!("unexpected `{s}`"))
                    .hint("`lane adopt` takes one folder, like `lane adopt ../my-worktree`."));
            }
        }
    }
    let path = path.unwrap_or_else(here);
    let repo = Repo::open(&path)?;
    let mut store = Store::open()?;
    let lane = engine::adopt(
        &mut store,
        &repo,
        &path,
        name.as_deref(),
        agent.as_deref(),
        common.request_id.as_deref(),
    )?;
    if common.json {
        print_json(&serde_json::json!({ "lane": lane }));
    } else {
        println!(
            "{} is a lane: {}, ports {}.",
            lane.name,
            lane.path.display(),
            ports(&lane)
        );
    }
    Ok(())
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
        if let Some(fix) = c.fix.as_ref().filter(|_| c.status != machine::Status::Ok) {
            print_fix(fix);
        }
    }
}

/// A fix as numbered steps under its check, with the command where it belongs. Nothing is run.
fn print_fix(fix: &machine::Fix) {
    let pad = " ".repeat(17);
    println!("{pad}How to fix: {}", fix.title);
    let command = || {
        if let Some(cmd) = &fix.command {
            println!("{pad}     $ {cmd}");
        }
    };
    let at = fix.command_after();
    for (n, step) in fix.steps.iter().enumerate() {
        if n == at {
            command();
        }
        println!("{pad}  {}. {step}", n + 1);
    }
    if at >= fix.steps.len() {
        command();
    }
    if let Some(page) = fix.open {
        println!("{pad}  Settings: {} ({})", page.label, page.uri);
    }
    println!("{pad}  Then run `lane setup` to check again.");
}
