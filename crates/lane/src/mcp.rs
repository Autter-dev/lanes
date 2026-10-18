//! `lane mcp`: Lanes' MCP server over stdin and stdout, so a coding agent can read the repo and
//! make lanes through tools.
//!
//! - `repo.read`, `repo.outline` and `repo.grep` read the worktree the server was started in,
//!   and nothing outside it. They never write anything, not even to Lanes' log.
//! - `lane.new` and `lane.list` are `lane new` and `lane list --json`, through the same engine.
//! - A tool that fails answers with an MCP tool error (`isError`) whose text is the CLI's: what
//!   went wrong, then what to do next. Its `structuredContent` is the `--json` error object.
//!
//! stdout carries only the protocol. Installs that `lane.new` runs write to Lanes' log, as they
//! do inside agents' hooks.

mod outline;

use lane_core::engine::{self, Repo};
use lane_core::error::{Error, ErrorKind, Result};
use lane_core::state::Store;
use lane_core::{VERSION, lane_env, next};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    JsonObject, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde::Serialize;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

pub const HELP: &str = "\
lane mcp: let coding agents read this repo and make lanes, through MCP tools

Usage: lane mcp [--json]

Speaks MCP (the Model Context Protocol) over stdin and stdout, for an agent to start. It serves
the repo it's started in, and nothing outside it. Its tools:
  repo.read      a file, or some of its lines
  repo.outline   a file's top-level definitions and their line numbers (best effort)
  repo.grep      a regex search with `git grep`, so .gitignore is respected
  lane.new       make a lane, instead of `git worktree add` or copying the repo
  lane.list      this repo's lanes, as `lane list --json` prints them
The repo tools only read. A tool's errors say what to do next, as the commands do.

Add it to Claude Code, from the repo's folder:
  claude mcp add lanes -- lane mcp
Lanes doesn't change any agent's settings for this.

Options:
  --json       Print an error that stops the server from starting as JSON
  -h, --help   Show this help
";

/// What every client is told when it connects.
const INSTRUCTIONS: &str = "Lanes gives each coding agent its own lane: an instant, ready-to-run \
copy of this repo with its own ports and URL. Before you change code, make a lane for your task \
with lane.new and work in its folder, instead of `git worktree add` or copying the repo. \
repo.read, repo.outline and repo.grep read the repo without changing anything.";

const READ: &str = "Read a file in this repo, or some of its lines. `path` is relative to the \
repo's root, like `src/main.rs`. Nothing outside the repo can be read: `..` past the root, \
absolute paths elsewhere and links that point out of the repo are refused. Returns the text as \
it is in the file. A long file comes back up to 2,000 lines at a time, with a note saying where \
to carry on; repo.outline and repo.grep find the lines you need first.";

const OUTLINE: &str = "List a file's top-level definitions with their line numbers (functions, \
types, classes, traits, impls, constants), so you can read just the part you need with \
repo.read. Best effort: it reads Rust, TypeScript, JavaScript, Python and Go line by line \
rather than parsing them, so it lists only what starts at the left margin (not methods inside a \
class or impl) and can miss a definition written in an unusual way.";

const GREP: &str = "Search this repo's files for a regular expression with `git grep`, so \
files .gitignore lists are skipped (new files not committed yet are searched). Returns one \
`path:line:text` line per match. The pattern is a POSIX extended regex, as `git grep -E` takes \
it; pass `literal: true` to search for the text exactly. Up to `max_results` matches (200 unless \
you say), with a note when there are more.";

const NEW: &str = "Make a lane for this task: your own instant, ready-to-run copy of this \
repo, with its dependencies installed, its own block of ports, its own URL and its own .env. Use \
it instead of `git worktree add` or copying the repo. Then do all your work in the lane's folder \
(`path`): edit, run and test there, and other agents working on this repo never get in your way. \
Returns the lane's name, folder, ports and URL. A lane is a Git worktree at the base branch's \
latest commit; no branch is made.";

const LIST: &str = "List this repo's lanes with their folders, ports and states, the same as \
`lane list --json`. Use it to find a lane made earlier, or to check that a name is free before \
lane.new.";

/// The most `repo.read` returns at once.
const READ_LINES: usize = 2000;
const READ_BYTES: usize = 256 * 1024;

/// How many matches `repo.grep` returns unless asked, and at most.
const GREP_DEFAULT: usize = 200;
const GREP_MAX: usize = 2000;
/// A matching line longer than this is cut short (minified code, say).
const GREP_LINE_CHARS: usize = 500;

/// `lane mcp`: serve the tools until the client closes stdin.
pub fn mcp(args: &[String]) -> Result<()> {
    for arg in args {
        match arg.as_str() {
            "--json" => {}
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(());
            }
            s => return Err(crate::unknown_option("mcp", s)),
        }
    }
    // Installs and notes go to the log: stdout is the protocol's, and clients show stderr.
    lane_core::log::set_quiet(true);
    if std::io::stdin().is_terminal() {
        eprintln!(
            "lane mcp: waiting for an MCP client on stdin. Agents start it themselves; for Claude Code, run `claude mcp add lanes -- lane mcp` in the repo. Ctrl+C stops it."
        );
    }
    let failed = |e: &dyn std::fmt::Display| {
        Error::new(
            ErrorKind::Failure,
            format!("the MCP server stopped: {e}"),
        )
        .hint("lane mcp is for MCP clients to start. Add it to your agent, like `claude mcp add lanes -- lane mcp`.")
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| failed(&e))?;
    let server = Lanes { dir: crate::here() };
    runtime.block_on(async {
        let running = server
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|e| failed(&e))?;
        running.waiting().await.map_err(|e| failed(&e))?;
        Ok(())
    })
}

/// The server: the folder it was started in, which the tools work on.
#[derive(Clone)]
struct Lanes {
    dir: PathBuf,
}

impl ServerHandler for Lanes {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("lanes", VERSION).with_title("Lanes"))
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(tools()))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        tools().into_iter().find(|t| t.name == name)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, ErrorData> {
        let tool = request.name.to_string();
        if self.get_tool(&tool).is_none() {
            return Err(ErrorData::invalid_params(
                format!(
                    "no tool called `{tool}`: lane mcp has repo.read, repo.outline, repo.grep, lane.new and lane.list"
                ),
                None,
            ));
        }
        let dir = self.dir.clone();
        let args = request.arguments.unwrap_or_default();
        // The engine runs Git and installs, which block: off the thread that reads requests.
        let result = tokio::task::spawn_blocking(move || call(&tool, &dir, &args))
            .await
            .map_err(|e| ErrorData::internal_error(format!("the tool stopped: {e}"), None))?;
        Ok(match result {
            Ok(out) => out.into_result(),
            Err(e) => error_result(&e),
        }
        .into())
    }
}

fn schema(value: Value) -> Arc<JsonObject> {
    match value {
        Value::Object(map) => Arc::new(map),
        _ => unreachable!("tool schemas are objects"),
    }
}

/// The tools, as `tools/list` shows them.
fn tools() -> Vec<Tool> {
    let reads = ToolAnnotations::new()
        .read_only(true)
        .destructive(false)
        .idempotent(true)
        .open_world(false);
    let path = json!({
        "type": "string",
        "description": "The file, relative to the repo's root, like `src/main.rs`."
    });
    vec![
        Tool::new(
            "repo.read",
            READ,
            schema(json!({
                "type": "object",
                "properties": {
                    "path": path,
                    "start_line": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "The first line to read, counting from 1. Default: 1."
                    },
                    "end_line": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "The last line to read, included. Default: the end of the file."
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            })),
        )
        .with_title("Read a file")
        .with_annotations(reads.clone()),
        Tool::new(
            "repo.outline",
            OUTLINE,
            schema(json!({
                "type": "object",
                "properties": { "path": path },
                "required": ["path"],
                "additionalProperties": false
            })),
        )
        .with_title("Outline a file")
        .with_annotations(reads.clone()),
        Tool::new(
            "repo.grep",
            GREP,
            schema(json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "What to look for: a POSIX extended regex, like `fn (new|open)_lane`."
                    },
                    "path": {
                        "type": "string",
                        "description": "Only search here: a folder or file relative to the repo's root, or a glob like `*.rs`."
                    },
                    "literal": {
                        "type": "boolean",
                        "description": "Match `pattern` as plain text, not as a regex. Default: false."
                    },
                    "ignore_case": {
                        "type": "boolean",
                        "description": "Match upper and lower case alike. Default: false."
                    },
                    "max_results": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": GREP_MAX,
                        "description": "The most matches to return. Default: 200."
                    }
                },
                "required": ["pattern"],
                "additionalProperties": false
            })),
        )
        .with_title("Search the repo")
        .with_annotations(reads.clone()),
        Tool::new(
            "lane.new",
            NEW,
            schema(json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "pattern": "^[a-z0-9]([a-z0-9-]{0,38}[a-z0-9])?$",
                        "description": "The lane's name, after the task: lowercase letters, digits and hyphens, like `fix-auth`."
                    },
                    "agent": {
                        "type": "string",
                        "description": "Which agent works in it, like `claude` or `codex`."
                    },
                    "request_id": {
                        "type": "string",
                        "description": "Makes a retry safe: a second call with the same id returns the first lane instead of making another."
                    }
                },
                "required": ["name"],
                "additionalProperties": false
            })),
        )
        .with_title("Make a lane")
        .with_annotations(
            ToolAnnotations::new()
                .read_only(false)
                .destructive(false)
                .idempotent(false)
                .open_world(false),
        ),
        Tool::new(
            "lane.list",
            LIST,
            schema(json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            })),
        )
        .with_title("List lanes")
        .with_annotations(reads),
    ]
}

/// What a tool returns: text for the agent to read, a note when the answer was cut short, and
/// the same answer as JSON (`structuredContent`).
struct Output {
    text: String,
    note: Option<String>,
    data: Value,
}

impl Output {
    /// An answer whose text is its JSON, as the CLI prints it with `--json`.
    fn json(data: Value) -> Output {
        Output {
            text: serde_json::to_string_pretty(&data).expect("output serializes"),
            note: None,
            data,
        }
    }

    fn into_result(self) -> CallToolResult {
        let mut content = vec![ContentBlock::text(self.text)];
        content.extend(self.note.map(ContentBlock::text));
        let mut result = CallToolResult::success(content);
        result.structured_content = Some(self.data);
        result
    }
}

/// A failed tool: the CLI's text (what went wrong, then what to do next) and its `--json` error.
fn error_result(e: &Error) -> CallToolResult {
    let mut result = CallToolResult::error(vec![ContentBlock::text(e.to_string())]);
    result.structured_content = Some(e.to_json());
    result
}

fn call(tool: &str, dir: &Path, args: &JsonObject) -> Result<Output> {
    match tool {
        "repo.read" => read(dir, args),
        "repo.outline" => outline(dir, args),
        "repo.grep" => grep(dir, args),
        "lane.new" => new_lane(dir, args).inspect_err(|e| {
            // As the CLI does: usage mistakes aren't worth keeping, everything else is.
            if e.kind != ErrorKind::Usage {
                lane_core::log::error("mcp lane.new", e.to_string()).write();
            }
        }),
        "lane.list" => list(dir, args),
        other => Err(Error::usage(format!("no tool called `{other}`"))),
    }
}

/// A tool's arguments, checked against the names it takes.
struct Args<'a> {
    tool: &'static str,
    map: &'a JsonObject,
}

impl<'a> Args<'a> {
    fn new(tool: &'static str, map: &'a JsonObject, takes: &[&str]) -> Result<Args<'a>> {
        if let Some(key) = map.keys().find(|k| !takes.contains(&k.as_str())) {
            let hint = match takes {
                [] => format!("{tool} takes no arguments."),
                _ => format!(
                    "{tool} takes {}.",
                    takes
                        .iter()
                        .map(|t| format!("`{t}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            return Err(Error::usage(format!("{tool} doesn't take `{key}`")).hint(hint));
        }
        Ok(Args { tool, map })
    }

    fn string(&self, key: &str) -> Result<Option<String>> {
        match self.map.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(Error::usage(format!("`{key}` must be a string"))
                .hint(format!("Pass it in quotes, like {{\"{key}\": \"...\"}}."))),
        }
    }

    fn required(&self, key: &str, example: &str) -> Result<String> {
        self.string(key)?.filter(|s| !s.is_empty()).ok_or_else(|| {
            Error::usage(format!("{} needs `{key}`", self.tool))
                .hint(format!("Pass one, like {{\"{key}\": \"{example}\"}}."))
        })
    }

    /// A whole number from 1 up. Numbers sent as strings are taken too.
    fn number(&self, key: &str) -> Result<Option<usize>> {
        let n = match self.map.get(key) {
            None | Some(Value::Null) => return Ok(None),
            Some(Value::Number(n)) => n.as_u64(),
            Some(Value::String(s)) => s.trim().parse().ok(),
            Some(_) => None,
        };
        match n {
            Some(n) if n >= 1 => Ok(Some(n as usize)),
            _ => Err(
                Error::usage(format!("`{key}` must be a whole number from 1 up"))
                    .hint(format!("Pass one, like {{\"{key}\": 10}}.")),
            ),
        }
    }

    fn flag(&self, key: &str) -> Result<bool> {
        match self.map.get(key) {
            None | Some(Value::Null) => Ok(false),
            Some(Value::Bool(b)) => Ok(*b),
            Some(_) => Err(Error::usage(format!("`{key}` must be true or false"))
                .hint(format!("Pass one, like {{\"{key}\": true}}."))),
        }
    }
}

/// Said instead of the CLI's not-a-repo hint, which suggests `--all`.
fn not_a_repo(dir: &Path) -> Error {
    Error::new(
        ErrorKind::NotARepo,
        format!(
            "lane mcp was started in {}, which isn't inside a Git repo",
            dir.display()
        ),
    )
    .hint("Start `lane mcp` in a repo's folder: run `claude mcp add lanes -- lane mcp` there, or set the folder your agent starts MCP servers in.")
}

/// The top folder of the worktree the server runs in: the repo tools read here.
fn worktree(dir: &Path) -> Result<PathBuf> {
    match lane_core::repo::top_level(dir) {
        Ok(top) => Ok(engine::norm(&top)),
        Err(e) if e.kind == ErrorKind::NotARepo => Err(not_a_repo(dir)),
        Err(e) => Err(e),
    }
}

fn open_repo(dir: &Path) -> Result<Repo> {
    match Repo::open(dir) {
        Err(e) if e.kind == ErrorKind::NotARepo => Err(not_a_repo(dir)),
        other => other,
    }
}

/// Collapse `.` and `..` without touching the disk.
fn clean(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The file `path` names in the worktree at `root`, and its path from the root with `/`s.
/// Refuses what leaves the repo: `..` past its root, absolute paths elsewhere, and links that
/// point out of it.
fn resolve(tool: &str, root: &Path, path: &str) -> Result<(PathBuf, String)> {
    let real_root = std::fs::canonicalize(root).map_err(|e| {
        Error::new(
            ErrorKind::Io,
            format!("couldn't open the repo at {}: {e}", root.display()),
        )
        .hint("Check that the repo's folder is still there, then try again.")
    })?;
    // An absolute path replaces the root here.
    let given = clean(&root.join(path));
    let written_inside = given.starts_with(clean(root));
    let outside = || {
        Error::usage(format!("`{path}` is outside the repo")).hint(format!(
            "{tool} reads files in {} only. Give a path from there, like `src/main.rs`.",
            root.display()
        ))
    };
    let real = match std::fs::canonicalize(&given) {
        Ok(real) => real,
        Err(_) if !written_inside => return Err(outside()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::new(
                ErrorKind::NotFound,
                format!("there's no file `{path}` in this repo"),
            )
            .hint("Paths start at the repo's root, like `src/main.rs`. repo.grep finds a file by what's in it."));
        }
        Err(e) => {
            return Err(
                Error::new(ErrorKind::Io, format!("couldn't open `{path}`: {e}"))
                    .hint("Check that the file is there and readable, then try again."),
            );
        }
    };
    if !real.starts_with(&real_root) {
        return Err(match written_inside {
            // The path is in the repo, but a link on the way leads out of it.
            true => Error::usage(format!(
                "`{path}` is a link to {}, outside the repo",
                real.display()
            ))
            .hint(format!(
                "{tool} only follows links that stay inside the repo."
            )),
            false => outside(),
        });
    }
    if real.is_dir() {
        return Err(Error::usage(format!("`{path}` is a folder")).hint(format!(
            "{tool} takes a file. repo.grep with `path` searches a whole folder."
        )));
    }
    let rel = real
        .strip_prefix(&real_root)
        .unwrap_or(&real)
        .to_string_lossy()
        .replace('\\', "/");
    Ok((real, rel))
}

/// A text file's contents. Binary files are refused.
fn read_text(tool: &str, file: &Path, rel: &str) -> Result<String> {
    let bytes = std::fs::read(file).map_err(|e| {
        Error::new(ErrorKind::Io, format!("couldn't read `{rel}`: {e}"))
            .hint("Check that the file is there and readable, then try again.")
    })?;
    if bytes[..bytes.len().min(8000)].contains(&0) {
        return Err(Error::usage(format!("`{rel}` is a binary file"))
            .hint(format!("{tool} reads text files only.")));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The longest start of `s` that fits in `max` bytes, cut at a character.
fn cut(s: &str, max: usize) -> &str {
    let mut end = max.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// `repo.read`: a file, or the lines asked for, at most [`READ_LINES`] lines at once.
fn read(dir: &Path, args: &JsonObject) -> Result<Output> {
    let a = Args::new("repo.read", args, &["path", "start_line", "end_line"])?;
    let path = a.required("path", "src/main.rs")?;
    let (start, end) = (a.number("start_line")?, a.number("end_line")?);
    if let (Some(s), Some(e)) = (start, end)
        && e < s
    {
        return Err(
            Error::usage(format!("end_line {e} comes before start_line {s}"))
                .hint("Pass an end_line from start_line on, or leave it out to read to the end."),
        );
    }
    let root = worktree(dir)?;
    let (file, rel) = resolve("repo.read", &root, &path)?;
    let text = read_text("repo.read", &file, &rel)?;
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let total = lines.len();
    let first = start.unwrap_or(1);
    if first > total.max(1) {
        let has = match total {
            0 => "is empty".to_string(),
            1 => "has 1 line".to_string(),
            n => format!("has {n} lines"),
        };
        return Err(
            Error::usage(format!("`{rel}` {has}, so it has no line {first}"))
                .hint(format!("Pass a start_line from 1 to {}.", total.max(1))),
        );
    }
    let last = end.unwrap_or(total).min(total);
    let (mut out, mut shown, mut long_line) = (String::new(), first - 1, false);
    for line in lines.iter().take(last).skip(first - 1) {
        if shown - (first - 1) == READ_LINES || out.len() + line.len() > READ_BYTES {
            if out.is_empty() {
                // One line longer than the whole budget: its start is better than nothing.
                out.push_str(cut(line, READ_BYTES));
                shown += 1;
                long_line = true;
            }
            break;
        }
        out.push_str(line);
        shown += 1;
    }
    let truncated = shown < last || long_line;
    let note = match (long_line, shown < last) {
        (true, _) => Some(format!(
            "Line {shown} is longer than {} KB, so only its start is shown.",
            READ_BYTES / 1024
        )),
        (false, true) => Some(format!(
            "Showing lines {first}-{shown} of {total}. Read on with start_line {}.",
            shown + 1
        )),
        (false, false) => None,
    };
    Ok(Output {
        text: out,
        note,
        data: json!({
            "path": rel,
            "start_line": first,
            "end_line": shown,
            "total_lines": total,
            "truncated": truncated,
        }),
    })
}

/// `repo.outline`: a file's top-level definitions, best effort.
fn outline(dir: &Path, args: &JsonObject) -> Result<Output> {
    let a = Args::new("repo.outline", args, &["path"])?;
    let path = a.required("path", "src/main.rs")?;
    let root = worktree(dir)?;
    let (file, rel) = resolve("repo.outline", &root, &path)?;
    let Some(language) = outline::Language::of(&file) else {
        return Err(Error::usage(format!("repo.outline can't outline `{rel}`"))
            .hint("It reads Rust, TypeScript, JavaScript, Python and Go. Read the file with repo.read, or find a definition with repo.grep."));
    };
    let text = read_text("repo.outline", &file, &rel)?;
    let items = outline::outline(language, &text);
    let listed = match items.is_empty() {
        true => format!("No top-level definitions found in {rel}."),
        false => items
            .iter()
            .map(|i| format!("{}: {} {}", i.line, i.kind, i.name))
            .collect::<Vec<_>>()
            .join("\n"),
    };
    Ok(Output {
        text: listed,
        note: None,
        data: json!({ "path": rel, "language": language.name(), "items": items }),
    })
}

/// One `repo.grep` match.
#[derive(Debug, Serialize)]
struct Match {
    path: String,
    line: u64,
    text: String,
}

/// A line of `git grep -n -z`: `path\0line\0text\n`.
fn parse_match(record: &[u8]) -> Option<Match> {
    let record = record.strip_suffix(b"\n").unwrap_or(record);
    let mut parts = record.splitn(3, |b| *b == 0);
    let path = String::from_utf8_lossy(parts.next()?).into_owned();
    let line = std::str::from_utf8(parts.next()?).ok()?.parse().ok()?;
    let text = String::from_utf8_lossy(parts.next()?);
    let text = text.trim_end_matches('\r');
    let text = match text.char_indices().nth(GREP_LINE_CHARS) {
        Some((i, _)) => format!("{}…", &text[..i]),
        None => text.to_string(),
    };
    Some(Match { path, line, text })
}

/// `repo.grep`: `git grep` over the worktree, tracked files and new ones .gitignore doesn't
/// list. Stops reading once it has enough matches.
fn grep(dir: &Path, args: &JsonObject) -> Result<Output> {
    let a = Args::new(
        "repo.grep",
        args,
        &["pattern", "path", "literal", "ignore_case", "max_results"],
    )?;
    let pattern = a.required("pattern", "fn main")?;
    let path = a.string("path")?.filter(|p| !p.is_empty());
    let (literal, ignore_case) = (a.flag("literal")?, a.flag("ignore_case")?);
    let max = a.number("max_results")?.unwrap_or(GREP_DEFAULT);
    if max > GREP_MAX {
        return Err(Error::usage(format!(
            "max_results can be {} at most",
            crate::thousands(GREP_MAX)
        ))
        .hint("Narrow the search with `path` or a closer pattern instead."));
    }
    let root = worktree(dir)?;
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(&root)
        .args(["-c", "core.quotePath=false", "grep"])
        .args(["-n", "-z", "-I", "--untracked", "--no-color"])
        .arg(if literal { "-F" } else { "-E" });
    if ignore_case {
        cmd.arg("-i");
    }
    cmd.arg("-e").arg(&pattern).arg("--");
    if let Some(p) = &path {
        cmd.arg(p);
    }
    let mut child = cmd
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| {
            Error::new(ErrorKind::Failure, "Git isn't installed or isn't on PATH")
                .hint("Install Git, then try again. `lane setup` shows how.")
        })?;
    let mut matches = Vec::new();
    let mut more = false;
    let mut reader = BufReader::new(child.stdout.take().expect("stdout is piped"));
    let mut record = Vec::new();
    loop {
        record.clear();
        match reader.read_until(b'\n', &mut record) {
            Ok(0) | Err(_) => break,
            Ok(_) if matches.len() == max => {
                more = true;
                break;
            }
            Ok(_) => matches.extend(parse_match(&record)),
        }
    }
    drop(reader);
    if more {
        let _ = child.kill();
    }
    let mut stderr = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut stderr);
    }
    let status = child.wait().map_err(|e| {
        Error::new(ErrorKind::Failure, format!("`git grep` didn't finish: {e}")).hint("Try again.")
    })?;
    // 1 means no matches; anything else that isn't success is Git saying no.
    if !more && !status.success() && status.code() != Some(1) {
        let why = stderr
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("no message")
            .trim()
            .trim_start_matches("fatal: ")
            .to_string();
        let e = Error::usage(format!("`git grep` refused it: {why}"));
        return Err(if why.contains("outside repository") {
            e.hint("Give `path` from the repo's root, like `src`, or leave it out.")
        } else if path.is_some() && why.contains(path.as_deref().unwrap_or_default()) {
            e.hint("Check `path`: a folder or file from the repo's root, or a glob like `*.rs`.")
        } else {
            e.hint("Check the pattern: it's a POSIX extended regex, as `git grep -E` takes it. Put a backslash before ( ) [ ] { } . * + ? | to match them as they are, or pass literal: true.")
        });
    }
    let listed = match matches.is_empty() {
        true => format!("No matches for `{pattern}`."),
        false => matches
            .iter()
            .map(|m| format!("{}:{}:{}", m.path, m.line, m.text))
            .collect::<Vec<_>>()
            .join("\n"),
    };
    let note = more.then(|| {
        format!(
            "Stopped at {}; there are more. Narrow the pattern, search one folder with `path`, or raise max_results (up to {}).",
            match max {
                1 => "1 match".to_string(),
                n => format!("{} matches", crate::thousands(n)),
            },
            crate::thousands(GREP_MAX)
        )
    });
    Ok(Output {
        text: listed,
        note,
        data: json!({
            "pattern": pattern,
            "path": path,
            "matches": matches,
            "truncated": more,
        }),
    })
}

/// `lane.new`: `lane new <name>`, through the same engine.
fn new_lane(dir: &Path, args: &JsonObject) -> Result<Output> {
    let a = Args::new("lane.new", args, &["name", "agent", "request_id"])?;
    let name = a.required("name", "fix-auth")?;
    let agent = a.string("agent")?.filter(|s| !s.is_empty());
    let request_id = a.string("request_id")?.filter(|s| !s.is_empty());
    if let Some(id) = &request_id {
        crate::args::check_request_id(id)?;
    }
    let started = std::time::Instant::now();
    let repo = open_repo(dir)?;
    // Through laned when it's running, like every other write; an agent's tools never start it.
    // Install output goes to the log, so only the protocol reaches stdout.
    let new = lane_core::writes::Write::New {
        repo: repo.root.clone(),
        name,
        agent,
        request_id,
    };
    let made = lane_core::writes::make(
        &new,
        lane_core::writes::Via::DaemonIfRunning,
        &mut lane_core::writes::to_stderr_or_log,
    )?
    .made()?;
    lane_core::log::info(
        "mcp lane.new",
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
    .ms(started.elapsed().as_millis() as u64)
    .write();
    let lane = &made.lane;
    Ok(Output::json(json!({
        "name": lane.name,
        "path": lane.path,
        "ports": { "start": lane.ports.start, "end": lane.ports.end(), "count": lane.ports.count },
        "url": lane_env::url(lane),
        "agent": lane.agent,
        "base": { "branch": made.base.branch, "commit": made.base.commit },
        "env_file": made.env_file,
        "next": next::in_lane(&repo, lane),
    })))
}

/// `lane.list`: what `lane list --json` prints for this repo.
fn list(dir: &Path, args: &JsonObject) -> Result<Output> {
    Args::new("lane.list", args, &[])?;
    let repo = open_repo(dir)?;
    let lanes = match Store::open_existing()? {
        Some(store) => store.lanes(Some(&repo.root))?,
        None => Vec::new(),
    };
    let steps = match lanes.is_empty() {
        true => next::first_lane(&repo),
        false => Vec::new(),
    };
    Ok(Output::json(
        json!({ "repo": repo.root, "lanes": lanes, "next": steps }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(v: Value) -> JsonObject {
        match v {
            Value::Object(m) => m,
            _ => panic!("not an object"),
        }
    }

    #[test]
    fn tools_are_named_exactly_and_described_for_agents() {
        let tools = tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        assert_eq!(
            names,
            [
                "repo.read",
                "repo.outline",
                "repo.grep",
                "lane.new",
                "lane.list"
            ]
        );
        for t in &tools {
            let d = t.description.as_deref().unwrap_or_default();
            assert!(d.len() > 80, "{}: {d}", t.name);
            assert_eq!(t.input_schema["type"], "object", "{}", t.name);
        }
        let reads: Vec<&str> = tools
            .iter()
            .filter(|t| t.annotations.as_ref().and_then(|a| a.read_only_hint) == Some(true))
            .map(|t| t.name.as_ref())
            .collect();
        assert_eq!(
            reads,
            ["repo.read", "repo.outline", "repo.grep", "lane.list"]
        );
    }

    #[test]
    fn arguments_are_checked_by_name_and_type() {
        let map = object(json!({ "file": "x" }));
        let e = Args::new("repo.read", &map, &["path"]).err().unwrap();
        assert_eq!(e.kind, ErrorKind::Usage);
        assert_eq!(
            e.to_string(),
            "repo.read doesn't take `file`. repo.read takes `path`."
        );

        let map = object(json!({ "path": 3, "n": "12", "bad": -1, "flag": "yes" }));
        let a = Args::new("t", &map, &["path", "n", "bad", "flag"]).unwrap();
        assert!(a.string("path").is_err());
        assert_eq!(a.number("n").unwrap(), Some(12));
        assert!(a.number("bad").is_err());
        assert!(a.flag("flag").is_err());
        assert!(!a.flag("missing").unwrap());
        assert!(
            a.required("missing", "x")
                .unwrap_err()
                .to_string()
                .contains("Pass one")
        );
    }

    #[test]
    fn grep_records_split_on_nul() {
        let m = parse_match(b"src/a b.rs\x0012\x00  let x = 1;\r\n").unwrap();
        assert_eq!((m.path.as_str(), m.line), ("src/a b.rs", 12));
        assert_eq!(m.text, "  let x = 1;");
        let long = format!("p\x001\x00{}\n", "é".repeat(GREP_LINE_CHARS + 5));
        let m = parse_match(long.as_bytes()).unwrap();
        assert_eq!(m.text.chars().count(), GREP_LINE_CHARS + 1);
        assert!(m.text.ends_with('…'));
        assert!(parse_match(b"no separators\n").is_none());
    }

    #[test]
    fn cut_never_splits_a_character() {
        assert_eq!(cut("héllo", 2), "h");
        assert_eq!(cut("héllo", 3), "hé");
        assert_eq!(cut("hi", 10), "hi");
    }
}
