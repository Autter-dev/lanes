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
mod guided;
mod hook;
mod mcp;

use lane_core::agents;
use lane_core::config::Config;
use lane_core::daemon;
use lane_core::engine::{self, Made, Repo};
use lane_core::env::Env;
use lane_core::error::ErrorKind;
use lane_core::error::{Error, Result};
use lane_core::machine::{self, Check};
use lane_core::project::Project;
use lane_core::state::{Lane, Status, Store};
use lane_core::telemetry;
use lane_core::writes::{self, Via};
use lane_core::{VERSION, exit};
use lane_core::{detect, lane_env, next};
use lane_tui::theme::Theme;
use lane_tui::{home, onboarding};
use std::path::PathBuf;
use std::process::ExitCode;

const HELP: &str = "\
lane: run many coding agents on one repo without them getting in each other's way

Usage: lane [command] [args]

Run `lane` on its own in a terminal to open the Lanes app (a short welcome on the first run).

Commands:
  setup      Check this machine and choose your settings
  init       Find this repo's stack and write lane.toml
  new        Create a lane
  list       List lanes with their ports and folders
  exec       Run a command inside a lane, with its ports
  env        Print a lane's variables (PORT, LANE_URL, ...)
  rm         Remove a lane
  adopt      Turn a worktree you already have into a lane
  doctor     Check what could stop lanes working, and say how to fix it
  logs       Show what Lanes did: lanes made and removed, agents' hooks, installs
  telemetry  Show or switch the anonymous install count (on, off, status)
  daemon     Show, start or stop laned, which keeps the terminal app live
  mcp        Let agents read this repo and make lanes, through MCP tools

Start in a repo:
  lane init                            find your stack and write lane.toml
  lane new fix-auth                    make a lane
  lane exec fix-auth -- npm run dev    run the app in it, on the lane's own port
Each command ends by saying what to run next.

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

Usage: lane adopt [<path>] [--ready] [--name NAME] [--agent NAME] [--json] [--request-id ID]

For worktrees made by `git worktree add` or by an agent's own tool. The folder stays where it
is; Lanes gives it a block of ports and lists it. Adopting a folder twice is safe. <path>
defaults to the folder you're in.

Options:
  --ready          Also make it ready to run, like a lane from `lane new`: its .env, and its
                   dependencies, cloned from the clean _base copy when the lockfiles match and
                   installed otherwise. The Git hook `lane init` adds uses this.
  --name NAME      The lane's name (default: from the folder's name)
  --agent NAME     Record which agent works in it
  --json           Print the lane
  --request-id ID  Make a retry safe
  -h, --help       Show this help
";

const EXEC_HELP: &str = "\
lane exec: run a command inside a lane, with its own ports

Usage: lane exec [<name>] [--json] -- <command> [args...]

Runs the command in the lane's folder with PORT, VITE_PORT, LANE_URL, LANE_NAME, LANE_ID,
LANE_PORTS and LANE_PATH set, so two lanes can run the same dev server at once. $PORT, ${PORT}
and %PORT% in the arguments are filled in too (likewise the other names), with no shell needed.
Quote them, so your own shell leaves them for Lanes. Without <name>, the lane you're in. Exits
with the command's own exit code.

Examples:
  lane exec fix-auth -- npm run dev
  lane exec fix-auth -- pnpm dev --port '$PORT'

Options:
  --json       The command's output goes to stderr; stdout gets {\"lane\", \"exit\", \"millis\"}
  -h, --help   Show this help
";

const ENV_HELP: &str = "\
lane env: print a lane's variables

Usage: lane env [<name>] [--shell sh|powershell|cmd|dotenv] [--json]

The variables `lane exec` sets: PORT, VITE_PORT, LANE_PORTS, LANE_NAME, LANE_ID, LANE_PATH and
LANE_URL. Without <name>, the lane you're in. Load them in a shell:
  eval \"$(lane env fix-auth --shell sh)\"
  lane env fix-auth --shell powershell | Invoke-Expression

Options:
  --shell NAME   Print for sh, powershell, cmd or dotenv (default: dotenv, KEY=value)
  --json         Print {\"lane\": ..., \"vars\": {...}}
  -h, --help     Show this help
";

const INIT_HELP: &str = "\
lane init: set up Lanes for this repo and the agents you use

Usage: lane init [--yes] [--force] [--print] [--no-agents] [--json]

Looks at the repo (package manager, dev server and its port, .env template, Compose file,
migration tool) and writes lane.toml with what it found: how to install dependencies in the
clean _base lane, how to start the dev server, and where each lane's .env starts from. Check
lane.toml in, so teammates get the same lanes.

In a terminal, it sets everything up and asks Y/n before each step that's yours to choose:
lane.toml, putting this machine's agents in lanes (Claude Code, Codex, Cursor, Gemini CLI,
GitHub Copilot), the Git hook, the AGENTS.md note, and installing dependencies now so the first
lane is ready in seconds. Then it says what's left to do. `--yes` takes every default without
asking. Agents, scripts and CI are never asked: for them it writes lane.toml and the repo's
hooks, and installs nothing.
(Set LANE_INTERACTIVE=1 if your terminal isn't recognised as one, and you'll be asked.)

Then it puts agents in lanes:
  Claude Code   hooks in .claude/settings.local.json (kept out of Git): `claude -w <name>`
                makes a lane, a session in a lane gets its ports, and `git worktree add`, a
                clone of the repo or a copy of its folder is turned down with a pointer to
                `lane new`. Only when Claude Code is in use here.
  Git hook      post-checkout: a worktree any other tool makes becomes a lane. With Husky, a
                line in .husky/post-checkout (checked in, so it's in the `git add` step). With
                lefthook or another core.hooksPath, nothing is written: init prints what to add.
  AGENTS.md     a short note on working in lanes (and `@AGENTS.md` in CLAUDE.md)
Running it again changes nothing that's already set up.

Options:
  --yes         Set everything up without asking, taking every default
  --force       Rewrite lane.toml even if it's already there
  --print       Show the lane.toml it would write, and write nothing
  --no-agents   Write lane.toml only: no hooks, no AGENTS.md note
  --json        Print what was found and what was written
  -h, --help    Show this help
";

const DOCTOR_HELP: &str = "\
lane doctor: check what could stop lanes working, and say how to fix it

Usage: lane doctor [--fix] [--yes] [--json]

Everything `lane setup` checks about this machine (Git, whether this disk clones files, disk
space), and then: that `lane` is on PATH so agents' hooks can run it, which agents put their work
in lanes, this repo's Git hook, lanes whose folders have gone, the daemon, and errors in the log.
Each problem says what to do next, and nothing is changed. Exits 1 when something must be fixed.

With --fix, Lanes fixes what it can by itself, asking Y/n before each: agents not in lanes yet,
this repo's Git hook, lanes whose folders have gone. What only you can do (install Git, move a
repo to a Dev Drive, change PATH) keeps its steps. In a script, add --yes to make every fix.

Options:
  --fix        Fix what Lanes can fix by itself, asking first
  --yes        With --fix: make every fix without asking
  --json       Print {\"healthy\": ..., \"checks\": [...]} (and \"fixed\" with --fix)
  -h, --help   Show this help
";

const LOGS_HELP: &str = "\
lane logs: what Lanes did, newest last

Usage: lane logs [<lane>] [-n N] [--path] [--json]

Lanes keeps a log of what changed lanes, what agents' hooks did and how dependency installs
went. It's never written into an agent's output, so look here when a hook or an install went
wrong. Times are UTC.

Options:
  <lane>          Only entries about this lane
  -n, --lines N   The last N entries (default 30)
  --path          Print the log file's path and nothing else
  --json          Print {\"path\": ..., \"entries\": [...]}
  -h, --help      Show this help
";

const DAEMON_HELP: &str = "\
lane daemon: laned, the Lanes daemon

Usage: lane daemon [status|start|stop] [--json]

laned runs in the background, one per user. It's the one writer: while it runs, `lane new`,
`rm` and `adopt`, agents' hooks and the terminal app send it their changes, and it makes them one
at a time per repo, with your environment, showing install output as it goes. It also keeps the
terminal app live: it reads each repo's lanes once and sends every open app what changed.

The terminal app and your own `lane new`, `rm` and `adopt` start it when it isn't running. It
stops by itself after half an hour with nobody connected. Agents' sessions, hooks and CI never
start it (they use it if it's running), and LANE_DAEMON=0 keeps anything from starting it.
Nothing needs it: every command works without it, and stopping it loses nothing.

  status   whether it's running, since when, and who's connected (the default)
  start    start it, unless it's running; a laned from another Lanes version is replaced
  stop     stop it, if it's running

Options:
  --json       Print {\"running\": ..., \"version\": ..., \"pid\": ..., \"uptime_secs\": ...}
  -h, --help   Show this help
";

const TELEMETRY_HELP: &str = "\
lane telemetry: the anonymous install count

Usage: lane telemetry [status|on|off] [--json]

Once Lanes is set up, and once after each upgrade, it sends a random install id, the Lanes
version, your OS and CPU type, and how you installed it. Nothing else: never code, file paths,
repo names or your IP address. `status` (the default) shows exactly what this machine would send.
DO_NOT_TRACK=1 or LANE_TELEMETRY=0 turn it off whatever the setting says, and CI never sends it.

Options:
  --json       Print {\"on\": ..., \"setting\": ..., \"forced_off\": ..., \"sends\": {...}}
  -h, --help   Show this help
";

const SETUP_HELP: &str = "\
lane setup: check this machine and choose your settings

Usage: lane setup [--json] [--yes] [--telemetry on|off] [--agents on|off]

In a terminal, `lane setup` opens the welcome: your machine, how a lane works, privacy and sign-in.

It also puts your agents in lanes, once for this machine: it adds Lanes' hooks and a `lanes`
skill to each agent's own user settings (Claude Code's in ~/.claude, Codex's in ~/.codex,
Cursor's in ~/.cursor, Gemini CLI's in ~/.gemini, GitHub Copilot's in ~/.copilot for the Copilot
CLI and VS Code), so every repo gets lanes without `lane init`. Gemini CLI also reads AGENTS.md
from then on, and VS Code gets `chat.useAgentsMdFile` unless you've set it. `--agents off` takes
exactly that back out, and leaves each file as it was.

Options:
  --json               Print the machine checks and settings as JSON; changes nothing unless --yes is given
  --yes                Save the settings without the terminal app (for scripts, CI and agents)
  --telemetry on|off   With --yes: turn the anonymous install count on or off
  --agents on|off      With --yes: put agents in lanes on this machine, or take it back out
  -h, --help           Show this help
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let env = Env::current();

    let code = match args.first().map(String::as_str) {
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
        Some("exec") => exec(&args[1..]),
        Some("env") => run("env", &args[1..], env_vars),
        Some("init") => run("init", &args[1..], init),
        Some("hook") => hook::hook(&args[1..]),
        Some("doctor") => doctor(&args[1..]),
        Some("logs") => run("logs", &args[1..], logs),
        Some("telemetry") => run("telemetry", &args[1..], telemetry),
        Some("daemon") => run("daemon", &args[1..], daemon_command),
        Some("mcp") => run("mcp", &args[1..], mcp::mcp),
        Some(other) => report(
            None,
            &Error::usage(format!("unknown command `{other}`"))
                .hint("Run `lane --help` to see the commands."),
            args::wants_json(&args),
        ),
    };
    // The install count, when the rules allow: after the command, in the background, so the
    // welcome and `lane setup` have saved the choice first and nothing waits for it.
    if let Ok(config) = Config::load() {
        let _ = telemetry::maybe_send(&env, &config);
    }
    code
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
    // Usage mistakes aren't worth keeping; everything else is.
    if e.kind != ErrorKind::Usage {
        lane_core::log::error(command.unwrap_or("lane"), e.to_string()).write();
    }
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
        Some(Repo::open(&here())?)
    };
    let root = repo.as_ref().map(|r| r.root.as_path());
    let lanes = match Store::open_existing()? {
        Some(store) => store.lanes(root)?,
        None => Vec::new(),
    };
    let steps = match &repo {
        Some(r) if lanes.is_empty() => next::first_lane(r),
        _ => Vec::new(),
    };
    if common.json {
        print_json(&serde_json::json!({ "repo": root, "lanes": lanes, "next": steps }));
        return Ok(());
    }
    if lanes.is_empty() {
        match root {
            Some(r) => println!("No lanes in {} yet.", r.display()),
            None => println!("No lanes yet. Run `lane new <name>` in a repo to make one."),
        }
        print!("{}", next::render(&steps));
        return Ok(());
    }
    let name_w = lanes.iter().map(|l| l.name.len()).max().unwrap_or(4).max(4);
    println!(
        "{:<name_w$}  {:<11}  {:<10}  PATH",
        "NAME", "PORTS", "STATE"
    );
    for l in &lanes {
        let ports = format!("{}-{}", l.ports.start, l.ports.end());
        println!(
            "{:<name_w$}  {ports:<11}  {:<10}  {}",
            l.name,
            l.state(),
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
    if made.clone.copied > 0 && lane_core::log::quiet() {
        lane_core::log::warn(
            "new",
            format!(
                "{} files were copied, not cloned: this disk can't clone files",
                made.clone.copied
            ),
        )
        .lane(&made.lane.name)
        .write();
        return;
    }
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
    let made = writes::make(
        &writes::Write::New {
            repo: repo.root.clone(),
            name,
            agent,
            request_id: common.request_id.clone(),
        },
        Via::for_env(&Env::current()),
        &mut writes::to_stderr_or_log,
    )?
    .made()?;
    let total = started.elapsed().as_secs_f64();
    lane_core::log::info(
        "new",
        format!(
            "made the lane: {} files {} at {}",
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
    .ms((total * 1000.0) as u64)
    .write();
    let steps = next::in_lane(&repo, &made.lane);
    if common.json {
        let mut value = serde_json::to_value(&made).expect("lanes serialize");
        value["next"] = serde_json::json!(steps);
        print_json(&value);
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
    print!("{}", next::render(&steps));
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
    let lane = writes::make(
        &writes::Write::Remove {
            repo: repo.root.clone(),
            name,
            keep_files,
            request_id: common.request_id.clone(),
        },
        Via::for_env(&Env::current()),
        &mut writes::to_stderr_or_log,
    )?
    .lane()?;
    lane_core::log::info(
        "rm",
        match keep_files {
            true => format!(
                "forgot the lane; its folder stays at {}",
                lane.path.display()
            ),
            false => "removed the lane and its folder".to_string(),
        },
    )
    .lane(&lane.name)
    .repo(&repo.root)
    .write();
    let steps = next::after_rm(&repo, Store::open()?.lanes(Some(&repo.root))?.len());
    if common.json {
        print_json(&serde_json::json!({ "removed": lane, "next": steps }));
        return Ok(());
    }
    if keep_files {
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
    print!("{}", next::render(&steps));
    Ok(())
}

/// `lane adopt [<path>]`: make an existing worktree a lane.
fn adopt(args: &[String]) -> Result<()> {
    let (common, rest) = args::split(args)?;
    let (mut path, mut name, mut agent, mut ready) = (None, None, None, false);
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{ADOPT_HELP}");
                return Ok(());
            }
            "--name" => name = Some(value(&mut it, "--name", "fix-auth")?),
            "--agent" => agent = Some(value(&mut it, "--agent", "claude")?),
            "--ready" => ready = true,
            s if s.starts_with('-') => return Err(unknown_option("adopt", s)),
            s if path.is_none() => path = Some(PathBuf::from(s)),
            s => {
                return Err(Error::usage(format!("unexpected `{s}`"))
                    .hint("`lane adopt` takes one folder, like `lane adopt ../my-worktree`."));
            }
        }
    }
    // Absolute, so it means the same to laned, which runs elsewhere.
    let path = std::path::absolute(path.unwrap_or_else(here)).map_err(|e| {
        Error::new(ErrorKind::Io, format!("couldn't resolve that folder: {e}"))
            .hint("Pass the worktree's folder, like `lane adopt ../my-worktree`.")
    })?;
    let repo = Repo::open(&path)?;
    let via = Via::for_env(&Env::current());
    let lane = writes::make(
        &writes::Write::Adopt {
            repo: repo.root.clone(),
            path,
            name,
            agent,
            request_id: common.request_id.clone(),
        },
        via,
        &mut writes::to_stderr_or_log,
    )?
    .lane()?;
    let made_ready = if ready {
        let ready = writes::Write::Ready {
            repo: repo.root.clone(),
            path: lane.path.clone(),
        };
        Some(writes::make(&ready, via, &mut writes::to_stderr_or_log)?.ready()?)
    } else {
        None
    };
    lane_core::log::info("adopt", format!("{} is a lane", lane.path.display()))
        .lane(&lane.name)
        .repo(&repo.root)
        .write();
    let steps = next::in_lane(&repo, &lane);
    if common.json {
        print_json(&serde_json::json!({ "lane": lane, "ready": made_ready, "next": steps }));
    } else {
        println!(
            "{} is a lane: {}, ports {}.",
            lane.name,
            lane.path.display(),
            ports(&lane)
        );
        if let Some(r) = &made_ready {
            match r.deps {
                engine::Deps::Cloned => println!(
                    "  deps    {} cloned from _base",
                    r.folders
                        .iter()
                        .map(|f| f.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                engine::Deps::Installed => {
                    println!("  deps    installed here: its lockfiles differ from _base's")
                }
                engine::Deps::None => {}
            }
            if let Some(env) = &r.env_file {
                println!("  env     {}", env.display());
            }
        }
        print!("{}", next::render(&steps));
    }
    Ok(())
}

/// The lane named `name` in this repo, or the lane the current folder is in.
fn lane_here(name: Option<&str>, command: &str) -> Result<Lane> {
    let repo = Repo::open(&here())?;
    let store = Store::open_existing()?;
    let lane = match (name, &store) {
        (Some(n), Some(s)) => s.lane(&repo.root, n)?,
        (None, Some(s)) => s.lane_at(&engine::norm(&lane_core::repo::top_level(&here())?))?,
        (_, None) => None,
    };
    let lane = lane.ok_or_else(|| match name {
        Some(n) => Error::new(
            ErrorKind::NotFound,
            format!("no lane called `{n}` in this repo"),
        )
        .hint("Run `lane list` to see this repo's lanes."),
        None => Error::usage("you're not inside a lane").hint(format!(
            "Name one, like `lane {command} fix-auth`, or run it from inside a lane's folder."
        )),
    })?;
    if lane.status == Status::Creating {
        return Err(Error::new(
            ErrorKind::Conflict,
            format!("the lane `{}` is still being created", lane.name),
        )
        .hint("Wait for `lane new` to finish, then try again."));
    }
    Ok(lane)
}

/// `lane exec [<name>] -- <command>`: run a command in a lane. Exits with the command's code.
fn exec(args: &[String]) -> ExitCode {
    // Everything after `--` (or after the lane's name) belongs to the command, not to Lanes.
    let (ours, command): (Vec<String>, Vec<String>) = match args.iter().position(|a| a == "--") {
        Some(i) => (args[..i].to_vec(), args[i + 1..].to_vec()),
        None => match args.iter().position(|a| !a.starts_with('-')) {
            Some(i) => (args[..=i].to_vec(), args[i + 1..].to_vec()),
            None => (args.to_vec(), Vec::new()),
        },
    };
    let json = args::wants_json(&ours);
    match exec_in(&ours, &command) {
        Ok(code) => ExitCode::from(code),
        Err(e) => report(Some("exec"), &e, json),
    }
}

fn exec_in(ours: &[String], command: &[String]) -> Result<u8> {
    let (common, rest) = args::split(ours)?;
    let mut name = None;
    for arg in &rest {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{EXEC_HELP}");
                return Ok(exit::OK);
            }
            s if s.starts_with('-') => return Err(unknown_option("exec", s)),
            s if name.is_none() => name = Some(s.to_string()),
            s => {
                return Err(Error::usage(format!("unexpected `{s}` before `--`")).hint(
                    "Put the command after `--`, like `lane exec fix-auth -- npm run dev`.",
                ));
            }
        }
    }
    let lane = lane_here(name.as_deref(), "exec")?;
    let mut cmd = lane_env::command(&lane, command)?;
    if common.json {
        cmd.stdout(std::process::Stdio::from(std::io::stderr()));
    }
    let started = std::time::Instant::now();
    let status = cmd.status().map_err(|e| {
        Error::new(
            ErrorKind::Failure,
            format!("couldn't start `{}`: {e}", command[0]),
        )
        .hint("Check that it's installed and on PATH.")
    })?;
    let code = exit_code(status);
    lane_core::log::info(
        "exec",
        format!("`{}` exited with {code}", command.join(" ")),
    )
    .lane(&lane.name)
    .ms(started.elapsed().as_millis() as u64)
    .write();
    if common.json {
        print_json(&serde_json::json!({
            "lane": lane.name,
            "command": command,
            "exit": code,
            "millis": started.elapsed().as_millis() as u64,
        }));
    }
    Ok(code)
}

/// A child's exit status as one byte: its own code, or 128 plus the signal that ended it.
fn exit_code(status: std::process::ExitStatus) -> u8 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return (128 + sig).min(255) as u8;
        }
    }
    match status.code() {
        Some(c) if (0..=255).contains(&c) => c as u8,
        Some(_) | None => exit::FAILURE,
    }
}

/// `lane env [<name>]`: print the lane's variables.
fn env_vars(args: &[String]) -> Result<()> {
    let (common, rest) = args::split(args)?;
    let (mut name, mut shell) = (None, "dotenv".to_string());
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{ENV_HELP}");
                return Ok(());
            }
            "--shell" => shell = value(&mut it, "--shell", "sh")?,
            s if s.starts_with('-') => return Err(unknown_option("env", s)),
            s if name.is_none() => name = Some(s.to_string()),
            s => {
                return Err(Error::usage(format!("unexpected `{s}`"))
                    .hint("`lane env` takes one lane name."));
            }
        }
    }
    let lane = lane_here(name.as_deref(), "env")?;
    let vars = lane_env::vars(&lane);
    if common.json {
        let map: serde_json::Map<String, serde_json::Value> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), serde_json::Value::String(v.clone())))
            .collect();
        print_json(&serde_json::json!({ "lane": lane.name, "vars": map }));
    } else {
        print!("{}", lane_env::render(&vars, &shell)?);
    }
    Ok(())
}

/// `lane init`: find what the repo needs and write lane.toml.
fn init(args: &[String]) -> Result<()> {
    let (common, rest) = args::split(args)?;
    let (mut force, mut print_only, mut no_agents, mut yes) = (false, false, false, false);
    for arg in &rest {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{INIT_HELP}");
                return Ok(());
            }
            "--force" => force = true,
            "--print" => print_only = true,
            "--no-agents" => no_agents = true,
            "--yes" | "-y" => yes = true,
            s => return Err(unknown_option("init", s)),
        }
    }
    let repo = Repo::open(&here())?;
    let found = detect::detect(&repo.root);
    let text = Project::init_text(
        &found.install,
        found.dev.as_ref().map(|d| d.command.as_str()),
        found.env_template.as_deref(),
    );
    let path = repo.root.join(lane_core::project::FILE_NAME);
    // A person at a terminal is guided, with a question before each step; --yes takes every
    // default. Agents, scripts and CI get the plain init below, which asks nothing.
    let person = !common.json && Env::current().can_ask();
    if !print_only && !no_agents && (yes || person) {
        return init_guided(&repo, &found, &text, &path, force, yes, common.json);
    }
    let existed = path.exists();
    let write = !print_only && (!existed || force);
    if write {
        // Same text, same file: running it again is safe.
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, &text)
            .and_then(|()| std::fs::rename(&tmp, &path))
            .map_err(|e| {
                Error::new(
                    ErrorKind::Io,
                    format!("couldn't write {}: {e}", path.display()),
                )
                .hint("Check that you can write to the repo's folder.")
            })?;
    }
    let setup = if print_only || no_agents {
        Vec::new()
    } else {
        setup_agents(&repo)?
    };
    if !print_only {
        let changed: Vec<String> = setup
            .iter()
            .filter(|(_, c)| c.changed)
            .map(|(w, _)| w.to_string())
            .collect();
        lane_core::log::info(
            "init",
            format!(
                "{} lane.toml{}",
                if write { "wrote" } else { "kept" },
                match changed.is_empty() {
                    true => String::new(),
                    false => format!("; set up {}", changed.join(", ")),
                }
            ),
        )
        .repo(&repo.root)
        .write();
    }
    let claude = setup.iter().any(|(what, _)| *what == "claude");
    let mut to_add: Vec<String> = Vec::new();
    if write {
        to_add.push(lane_core::project::FILE_NAME.to_string());
    }
    for (_, change) in &setup {
        if change.changed && change.check_in {
            let rel = change.path.strip_prefix(&repo.root).unwrap_or(&change.path);
            to_add.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    let steps = if print_only {
        Vec::new()
    } else {
        next::after_init(&to_add, claude)
    };
    if common.json {
        let agents: Vec<serde_json::Value> = setup
            .iter()
            .map(|(what, change)| {
                let mut v = serde_json::to_value(change).expect("changes serialize");
                v["what"] = serde_json::json!(what);
                v
            })
            .collect();
        print_json(&serde_json::json!({
            "repo": repo.root,
            "found": found,
            "lane_toml": path,
            "written": write,
            "kept_existing": existed && !write && !print_only,
            "text": text,
            "agents": agents,
            "next": steps,
        }));
        return Ok(());
    }
    if print_only {
        print!("{text}");
        return Ok(());
    }
    print_found(&found, &repo.root);
    if write {
        println!("Wrote {}.", path.display());
    } else {
        println!(
            "lane.toml is already there, so it was left as is. `lane init --print` shows what it would write; `--force` rewrites it."
        );
    }
    if !setup.is_empty() {
        println!("Agents:");
        for (what, change) in &setup {
            let (label, done) = match *what {
                "claude" => (
                    "Claude Code",
                    "hooks in .claude/settings.local.json, kept out of Git",
                ),
                "git_hook" => (
                    "Git hook",
                    "post-checkout: worktrees other tools make become lanes",
                ),
                "agents_md" => ("AGENTS.md", "a note on working in lanes"),
                _ => ("CLAUDE.md", "@AGENTS.md, so Claude Code reads the note"),
            };
            let state = match (&change.note, change.changed) {
                (Some(note), _) => note.clone(),
                (None, true) => done.to_string(),
                (None, false) => format!("{done} (already set up)"),
            };
            println!("  {label:<12} {state}");
            for line in change.snippet.iter().flat_map(|s| s.lines()) {
                println!("      {line}");
            }
        }
    }
    print!("{}", next::render(&steps));
    Ok(())
}

/// The guided init: what was found, each step done or asked about, and what's left to do.
fn init_guided(
    repo: &Repo,
    found: &detect::Detected,
    text: &str,
    path: &std::path::Path,
    force: bool,
    yes: bool,
    json: bool,
) -> Result<()> {
    if !json {
        println!("Setting up Lanes for {}", repo.root.display());
        print_found(found, &repo.root);
        println!();
    }
    let (done, steps) = match yes {
        true => guided::init(repo, text, path, force, &mut guided::Defaults, !json)?,
        false => guided::init(repo, text, path, force, &mut guided::Terminal, true)?,
    };
    let did: Vec<&str> = done.iter().filter(|d| d.done).map(|d| d.what).collect();
    lane_core::log::info(
        "init",
        match did.is_empty() {
            true => "nothing to set up".to_string(),
            false => format!("set up {}", did.join(", ")),
        },
    )
    .repo(&repo.root)
    .write();
    if json {
        print_json(&serde_json::json!({
            "repo": repo.root,
            "found": found,
            "lane_toml": path,
            "text": text,
            "done": done,
            "next": steps,
        }));
        return Ok(());
    }
    println!();
    print!("{}", next::render(&steps));
    Ok(())
}

/// What `lane init` found in the repo.
fn print_found(found: &detect::Detected, root: &std::path::Path) {
    if found.stack.is_empty() {
        println!("Nothing recognised in {}.", root.display());
    } else {
        println!("Found: {}", found.stack.join(", "));
    }
    if !found.install.is_empty() {
        println!("  install  {}", found.install.join(" && "));
    }
    if let Some(dev) = &found.dev {
        let port = dev
            .default_port
            .map(|p| format!(" (usually port {p})"))
            .unwrap_or_default();
        println!("  dev      {}{port}", dev.command);
    }
    if let Some(t) = &found.env_template {
        println!("  env      each lane's .env starts from {}", t.display());
    }
    for note in &found.notes {
        println!("  note     {note}");
    }
}

/// Put agents in lanes: the Git hook and the AGENTS.md note always, Claude Code's hooks when
/// Claude Code is in use here. Each step is safe to run again.
fn setup_agents(repo: &Repo) -> Result<Vec<(&'static str, agents::Change)>> {
    let mut done = vec![("git_hook", agents::setup_git_hook(repo)?)];
    let mut notes = agents::setup_agents_md(repo)?.into_iter();
    if let Some(c) = notes.next() {
        done.push(("agents_md", c));
    }
    if let Some(c) = notes.next() {
        done.push(("claude_md", c));
    }
    if agents::uses_claude(repo) {
        done.push(("claude", agents::setup_claude(repo)?));
    }
    Ok(done)
}

/// `lane doctor`: every check, and exit 1 when something must be fixed.
fn doctor(args: &[String]) -> ExitCode {
    let json = args::wants_json(args);
    let (mut fix, mut yes) = (false, false);
    for arg in args {
        match arg.as_str() {
            "--json" => {}
            "--fix" => fix = true,
            "--yes" | "-y" => yes = true,
            "-h" | "--help" => {
                print!("{DOCTOR_HELP}");
                return ExitCode::from(exit::OK);
            }
            s => return report(Some("doctor"), &unknown_option("doctor", s), json),
        }
    }
    let person = !json && Env::current().can_ask();
    if fix && !yes && !person {
        let e = Error::usage("`lane doctor --fix` asks before each fix")
            .hint("In a script, add --yes to make every fix: `lane doctor --fix --yes`.");
        return report(Some("doctor"), &e, json);
    }
    if yes && !fix {
        let e = Error::usage("--yes only works with --fix")
            .hint("Run `lane doctor --fix --yes` to make every fix without asking.");
        return report(Some("doctor"), &e, json);
    }
    let mut fixed = Vec::new();
    if fix {
        if person && !yes {
            print_checks(&lane_core::doctor::run(&here()));
            println!();
            println!("Lanes can fix some of this itself:");
        }
        let result = match yes {
            true => guided::fix(&here(), &mut guided::Defaults),
            false => guided::fix(&here(), &mut guided::Terminal),
        };
        fixed = match result {
            Ok(f) => f,
            Err(e) => return report(Some("doctor"), &e, json),
        };
        if !json {
            if fixed.is_empty() {
                println!("Nothing for Lanes to fix by itself.");
            }
            for f in &fixed {
                println!("  ✓ {}", f.summary);
            }
            println!();
        }
    }
    let checks = lane_core::doctor::run(&here());
    let healthy = lane_core::doctor::healthy(&checks);
    if json {
        let mut v = serde_json::json!({ "healthy": healthy, "checks": checks });
        if fix {
            v["fixed"] = serde_json::json!(fixed);
        }
        print_json(&v);
    } else {
        print_checks(&checks);
        println!();
        let fix = checks
            .iter()
            .filter(|c| c.status == machine::Status::Fail)
            .count();
        let warn = checks
            .iter()
            .filter(|c| c.status == machine::Status::Warn)
            .count();
        match (fix, warn) {
            (0, 0) => println!("All good."),
            (0, n) => println!(
                "Nothing must be fixed. {n} thing{} worth a look (!).",
                if n == 1 { "" } else { "s" }
            ),
            (n, _) => println!(
                "{n} thing{} to fix (✗) before lanes work. Each says how.",
                if n == 1 { "" } else { "s" }
            ),
        }
    }
    ExitCode::from(if healthy { exit::OK } else { exit::FAILURE })
}

/// `lane logs`: the log's last entries, oldest first.
fn logs(args: &[String]) -> Result<()> {
    let (common, rest) = args::split(args)?;
    let (mut lane, mut lines, mut path_only) = (None, 30usize, false);
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{LOGS_HELP}");
                return Ok(());
            }
            "-n" | "--lines" => {
                let v = value(&mut it, "--lines", "50")?;
                lines = v.parse().map_err(|_| {
                    Error::usage(format!("`{v}` isn't a number of lines"))
                        .hint("Pass a whole number, like `--lines 50`.")
                })?;
            }
            "--path" => path_only = true,
            s if s.starts_with('-') => return Err(unknown_option("logs", s)),
            s if lane.is_none() => lane = Some(s.to_string()),
            s => {
                return Err(Error::usage(format!("unexpected `{s}`"))
                    .hint("`lane logs` takes one lane name, like `lane logs fix-auth`."));
            }
        }
    }
    let path = lane_core::log::path()?;
    if path_only {
        if common.json {
            print_json(&serde_json::json!({ "path": path }));
        } else {
            println!("{}", path.display());
        }
        return Ok(());
    }
    let entries = lane_core::log::read(lines, lane.as_deref())?;
    if common.json {
        print_json(&serde_json::json!({ "path": path, "entries": entries }));
        return Ok(());
    }
    if entries.is_empty() {
        match &lane {
            Some(l) => println!("Nothing logged about {l} yet."),
            None => println!(
                "Nothing logged yet. Lanes writes here when lanes change, agents' hooks run, or an install goes wrong."
            ),
        }
        return Ok(());
    }
    let ev_w = entries
        .iter()
        .map(|e| e.event.len())
        .max()
        .unwrap_or(0)
        .min(28);
    for e in &entries {
        let mark = match e.level {
            lane_core::log::Level::Info => " ",
            lane_core::log::Level::Warn => "!",
            lane_core::log::Level::Error => "✗",
        };
        let at = e.at.replace('T', " ").trim_end_matches('Z').to_string();
        let lane = e
            .lane
            .as_deref()
            .map(|l| format!("{l}: "))
            .unwrap_or_default();
        let ms =
            e.ms.map(|m| format!(" ({:.1} s)", m as f64 / 1000.0))
                .unwrap_or_default();
        println!("{mark} {at}  {:<ev_w$}  {lane}{}{ms}", e.event, e.message);
    }
    println!("\nFrom {} (UTC).", path.display());
    Ok(())
}

/// `lane daemon [status|start|stop]`. Starting and stopping are safe to retry: each says what's
/// true afterwards, whatever it was before.
fn daemon_command(args: &[String]) -> Result<()> {
    let (common, rest) = args::split(args)?;
    let mut action: Option<&str> = None;
    for arg in &rest {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{DAEMON_HELP}");
                return Ok(());
            }
            a @ ("status" | "start" | "stop") if action.is_none() => action = Some(a),
            s if s.starts_with('-') => return Err(unknown_option("daemon", s)),
            s => {
                return Err(Error::usage(format!("unexpected `{s}`"))
                    .hint("Use `lane daemon status`, `lane daemon start` or `lane daemon stop`."));
            }
        }
    }
    let at = daemon::Endpoint::current()?;
    let failed = |what: &str, e: std::io::Error| {
        Error::new(ErrorKind::Failure, format!("couldn't {what} laned ({e})"))
            .hint("Run `lane logs` to see what laned said, or run `laned` in a terminal to watch it start.")
    };
    let mut stopped = None;
    let client = match action.unwrap_or("status") {
        "start" => {
            let laned = daemon::laned_path().ok_or_else(|| {
                Error::new(ErrorKind::Failure, "laned isn't installed next to lane").hint(
                    "Install Lanes again (lanes.autter.dev/download); every way installs both programs.",
                )
            })?;
            let client = daemon::connect_or_start(&at, &laned).map_err(|e| failed("start", e))?;
            if client.started {
                lane_core::log::info("daemon", format!("started laned (pid {})", client.pid))
                    .write();
            }
            Some(client)
        }
        "stop" => {
            let was = daemon::stop(&at).map_err(|e| failed("stop", e))?;
            if was {
                lane_core::log::info("daemon", "stopped laned").write();
            }
            stopped = Some(was);
            None
        }
        _ => daemon::Client::connect(&at).ok(),
    };
    let status = match client {
        Some(mut c) => Some(c.status().map_err(|e| failed("ask", e))?),
        None => None,
    };
    let uptime = status.as_ref().map(|s| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64 - s.started)
            .unwrap_or_default()
            .max(0)
    });

    if common.json {
        let mut out = serde_json::json!({
            "running": status.is_some(),
            "endpoint": at.describe(),
        });
        if let (Some(s), Some(up)) = (&status, uptime) {
            out["version"] = s.version.clone().into();
            out["pid"] = s.pid.into();
            out["uptime_secs"] = up.into();
            // Not counting this command's own connection.
            out["clients"] = s.clients.saturating_sub(1).into();
            out["watching"] = s.watching.into();
            out["writes"] = s.writes.into();
        }
        if let Some(was) = stopped {
            out["stopped"] = was.into();
        }
        print_json(&out);
        return Ok(());
    }

    match (&status, stopped) {
        (Some(s), _) => {
            let up = uptime.unwrap_or_default();
            let others = s.clients.saturating_sub(1);
            println!(
                "laned {} is running (pid {}), up {}, with {} other client{}.",
                s.version,
                s.pid,
                lane_core::insight::duration(up as u64 * 1000),
                others,
                if others == 1 { "" } else { "s" }
            );
            if s.version != VERSION {
                println!(
                    "It's from another Lanes version ({VERSION} here): `lane daemon start` replaces it."
                );
            }
            println!("It listens at {}.", at.describe());
        }
        (None, Some(true)) => {
            println!("Stopped laned. Nothing was lost: everything it knew is in Lanes' state.")
        }
        (None, Some(false)) => println!("laned wasn't running."),
        (None, None) => {
            println!("laned isn't running. Nothing needs it: every command works without it.");
            println!();
            print!(
                "{}",
                next::render(&[next::Step {
                    run: "lane daemon start".into(),
                    why: "start it now; the terminal app starts it too".into(),
                }])
            );
        }
    }
    Ok(())
}

/// `lane telemetry [status|on|off]`: what the install count sends from this machine, and the
/// switch. Switching is safe to retry: it sets the value, it doesn't toggle.
fn telemetry(args: &[String]) -> Result<()> {
    let (common, rest) = args::split(args)?;
    let mut action: Option<&str> = None;
    for arg in &rest {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{TELEMETRY_HELP}");
                return Ok(());
            }
            a @ ("status" | "on" | "off") if action.is_none() => action = Some(a),
            s if s.starts_with('-') => return Err(unknown_option("telemetry", s)),
            s => {
                return Err(Error::usage(format!("unexpected `{s}`")).hint(
                    "Use `lane telemetry on`, `lane telemetry off` or `lane telemetry status`.",
                ));
            }
        }
    }
    let env = Env::current();
    let mut config = Config::load()?;
    let switch = match action {
        Some("on") => Some(true),
        Some("off") => Some(false),
        _ => None,
    };
    if let Some(on) = switch {
        if config.telemetry != on {
            config.telemetry = on;
            config.save()?;
        }
        lane_core::log::info(
            "telemetry",
            format!("install count {}", if on { "on" } else { "off" }),
        )
        .write();
    }
    let forced = env.telemetry_forced_off();
    let on = config.telemetry && forced.is_none();
    let install = telemetry::Install::load();
    let ping = telemetry::ping(install.id.as_deref().unwrap_or(""));
    let endpoint = telemetry::endpoint(&env);
    let next = telemetry::decide(&env, &config, &install, endpoint.clone());

    if common.json {
        let mut sends = serde_json::to_value(&ping).expect("serializes");
        if install.id.is_none() {
            sends["id"] = serde_json::Value::Null;
        }
        print_json(&serde_json::json!({
            "on": on,
            "setting": config.telemetry,
            "forced_off": forced,
            "sends": sends,
            "counted": install.counted,
            "endpoint": endpoint,
            "saved": switch.is_some(),
        }));
        return Ok(());
    }

    let state = match (on, forced) {
        (true, _) => "on".to_string(),
        (false, Some(why)) => format!("off ({why})"),
        (false, None) => "off".to_string(),
    };
    if switch.is_some() {
        println!("Saved. Install count: {state}.");
        if switch == Some(true)
            && let Some(why) = forced
        {
            println!("It stays off while {why}.");
        }
    } else {
        println!("Install count: {state}.");
    }
    println!();
    println!(
        "When it's on, Lanes sends this once it's set up and once after each upgrade, and nothing else:"
    );
    let id = install
        .id
        .clone()
        .unwrap_or_else(|| "(made the first time it's sent)".into());
    for (name, value, why) in [
        ("id", id.as_str(), "random, not linked to you"),
        ("version", ping.version, ""),
        ("os", ping.os, ""),
        ("arch", ping.arch, "your CPU type"),
        ("channel", ping.channel, "how you installed Lanes"),
    ] {
        println!("  {name:<9} {value:<34} {why}");
    }
    println!("It never sends code, file paths, repo names or your IP address.");
    if on {
        match next {
            Ok(_) => println!("This install is counted when this command finishes."),
            Err(skip) => println!("Right now: {}.", skip.describe()),
        }
    }
    println!();
    let step = if on {
        next::Step {
            run: "lane telemetry off".into(),
            why: "stop sending it".into(),
        }
    } else {
        next::Step {
            run: "lane telemetry on".into(),
            why: "send it again (environment opt-outs still win)".into(),
        }
    };
    print!("{}", next::render(&[step]));
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
        agents: config.agents,
        agents_found: agents::user::found().iter().map(|a| a.name).collect(),
    };
    match lane_tui::run_onboarding(input) {
        Ok(onboarding::Outcome::Finished { telemetry, agents }) => {
            config.onboarded = Some(VERSION.to_string());
            if env.telemetry_forced_off().is_none() {
                config.telemetry = telemetry;
            }
            config.agents = agents;
            config.save().map_err(fail)?;
            match agents::user::setup(agents) {
                Ok(done) => {
                    log_agent_setup(&done, agents);
                    print_agent_setup(&done, agents);
                    print!("{}", next::render(&after_welcome(&done, agents)));
                }
                Err(e) => eprintln!("lane: {e}"),
            }
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
        let repo = home_repo();
        let snapshot = lane_tui::feed::read(repo.as_ref().map(|r| r.root.as_path()));
        let input = home::Input {
            version: VERSION,
            telemetry: config.telemetry,
            telemetry_forced_off: env.telemetry_forced_off(),
            theme: theme(env),
            repo,
            snapshot,
            agents_unset: if config.agents {
                agents::user::found()
                    .into_iter()
                    .filter(|a| !agents::user::is_set_up(*a))
                    .map(|a| a.name)
                    .collect()
            } else {
                Vec::new()
            },
        };
        let (outcome, telemetry) = match lane_tui::run_home(input, daemon::may_start(env)) {
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

/// The repo `lane` was opened in, if any: the home screen lists its lanes, or every lane on this
/// machine outside a repo.
fn home_repo() -> Option<home::HomeRepo> {
    Repo::open(&here()).ok().map(|r| home::HomeRepo {
        name: r
            .root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        set_up: r.has_lane_toml(),
        root: r.root,
    })
}

struct SetupFlags {
    json: bool,
    yes: bool,
    telemetry: Option<bool>,
    agents: Option<bool>,
}

fn parse_setup(args: &[String]) -> Result<Option<SetupFlags>, String> {
    let mut flags = SetupFlags {
        json: false,
        yes: false,
        telemetry: None,
        agents: None,
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
            "--agents" => {
                flags.agents = Some(match it.next().map(String::as_str) {
                    Some("on") => true,
                    Some("off") => false,
                    _ => return Err("--agents takes `on` or `off`".into()),
                })
            }
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    if flags.telemetry.is_some() && !flags.yes {
        return Err("--telemetry only works with --yes".into());
    }
    if flags.agents.is_some() && !flags.yes {
        return Err("--agents only works with --yes".into());
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

    let mut agent_setup = None;
    if flags.yes {
        config.onboarded = Some(VERSION.to_string());
        if let Some(t) = flags.telemetry {
            config.telemetry = t;
        }
        if let Some(a) = flags.agents {
            config.agents = a;
        }
        if let Err(e) = config.save() {
            return fail(e);
        }
        agent_setup = Some(match agents::user::setup(config.agents) {
            Ok(done) => done,
            Err(e) => return report(Some("setup"), &e, flags.json),
        });
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
                "agents": config.agents,
                "saved": flags.yes,
            },
            "agents": agents_json(agent_setup.as_deref()),
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
        if let Some(done) = &agent_setup {
            log_agent_setup(done, config.agents);
            print_agent_setup(done, config.agents);
        }
    } else {
        println!("Install count: {tele}. Nothing was changed.");
        println!(
            "Run `lane setup --yes` to save these settings, or `lane setup` in a terminal for the full welcome."
        );
    }
    ExitCode::from(exit::OK)
}

/// The agents found here, whether each is set up, and what `--yes` just changed.
fn agents_json(done: Option<&[(agents::user::Agent, Vec<agents::Change>)]>) -> serde_json::Value {
    let found = agents::user::found();
    serde_json::json!(
        found
            .iter()
            .map(|a| {
                let changes = done
                    .and_then(|d| d.iter().find(|(x, _)| x == a))
                    .map(|(_, c)| c.clone())
                    .unwrap_or_default();
                serde_json::json!({
                    "id": a.id,
                    "name": a.name,
                    "set_up": agents::user::is_set_up(*a),
                    "changes": changes,
                })
            })
            .collect::<Vec<_>>()
    )
}

/// What's left after the welcome: set up the repo you're in (or go to one), trust Codex's hooks
/// once, and get instant lanes where this disk copies them.
fn after_welcome(done: &[(agents::user::Agent, Vec<agents::Change>)], on: bool) -> Vec<next::Step> {
    let mut steps = Vec::new();
    let codex_now = done
        .iter()
        .any(|(a, c)| a.id == "codex" && c.iter().any(|c| c.changed));
    if on && codex_now {
        steps.push(next::Step {
            run: "/hooks".into(),
            why: "in Codex, once: trust Lanes' two hooks, so Codex sessions get lanes".into(),
        });
    }
    match Repo::open(&here()) {
        Ok(_) => steps.push(next::Step {
            run: "lane init".into(),
            why: "set this repo up, with a Y/n before each step".into(),
        }),
        Err(_) => steps.push(next::Step {
            run: "cd your-project && lane init".into(),
            why: "set up a repo you work on, with a Y/n before each step".into(),
        }),
    }
    if machine::run_check("copies", &here()).status == machine::Status::Warn {
        steps.push(next::Step {
            run: "lane doctor".into(),
            why:
                "this disk copies lanes instead of cloning them; it shows how to get instant lanes"
                    .into(),
        });
    }
    steps
}

/// A log line per agent that putting agents in lanes changed.
fn log_agent_setup(done: &[(agents::user::Agent, Vec<agents::Change>)], on: bool) {
    for (agent, changes) in done.iter().filter(|(_, c)| c.iter().any(|c| c.changed)) {
        let files: Vec<String> = changes
            .iter()
            .filter(|c| c.changed)
            .map(|c| c.path.display().to_string())
            .collect();
        lane_core::log::info(
            "setup",
            format!(
                "{} {}: {}",
                agent.name,
                if on { "set up" } else { "taken out" },
                files.join(", ")
            ),
        )
        .write();
    }
}

/// What putting agents in lanes did, one line per agent.
fn print_agent_setup(done: &[(agents::user::Agent, Vec<agents::Change>)], on: bool) {
    if done.is_empty() {
        if on {
            println!(
                "Agents in lanes: none found yet. Run `lane setup --yes` after you install one."
            );
        }
        return;
    }
    println!("Agents in lanes:");
    for (agent, changes) in done {
        let changed = changes.iter().any(|c| c.changed);
        let state = match (on, changed) {
            (true, true) => "set up: its worktrees become lanes, in every repo",
            (true, false) => "already set up",
            (false, true) => "taken back out of its settings",
            (false, false) => "not set up",
        };
        println!("  {:<14} {state}", agent.name);
        for note in changes.iter().filter_map(|c| c.note.as_deref()) {
            println!("  {:<14} {note}", "");
        }
    }
}

fn print_checks(checks: &[Check]) {
    for c in checks {
        println!("{} {:<16}{}", c.status.symbol(), c.label, c.summary);
        if let Some(hint) = c
            .hint
            .as_deref()
            .filter(|_| c.status != machine::Status::Ok)
        {
            println!("  {:<16}{hint}", "");
        }
        if let Some(fix) = c.fix.as_ref().filter(|_| c.status != machine::Status::Ok) {
            print_fix(fix);
        }
    }
}

/// A fix as numbered steps under its check, with the command where it belongs. Nothing is run.
fn print_fix(fix: &machine::Fix) {
    let pad = " ".repeat(18);
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
