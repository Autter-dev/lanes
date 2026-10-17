//! What a lane's processes see: its ports, URL and name as environment variables, its `.env`,
//! and how a command is started inside it.
//!
//! | Variable | Value |
//! | --- | --- |
//! | `PORT` | the first port of the lane's block |
//! | `VITE_PORT` | the second (or the first, for a one-port block) |
//! | `LANE_PORTS` | the whole block, like `4100-4109` |
//! | `LANE_NAME`, `LANE_ID`, `LANE_PATH` | the lane |
//! | `LANE_URL` | `http://<name>.localhost:<PORT>` (browsers send `*.localhost` to this machine) |
//!
//! The lane's values win over anything already in the shell, so an exported `PORT` never
//! makes two lanes collide.

use crate::error::{Error, ErrorKind, Result};
use crate::state::Lane;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The variables every command in the lane gets, in a stable order.
pub fn vars(lane: &Lane) -> Vec<(&'static str, String)> {
    let p = lane.ports.start;
    let vite = if lane.ports.count > 1 { p + 1 } else { p };
    vec![
        ("PORT", p.to_string()),
        ("VITE_PORT", vite.to_string()),
        ("LANE_PORTS", format!("{p}-{}", lane.ports.end())),
        ("LANE_NAME", lane.name.clone()),
        ("LANE_ID", lane.id.clone()),
        ("LANE_PATH", lane.path.to_string_lossy().into_owned()),
        ("LANE_URL", url(lane)),
    ]
}

/// The lane's URL: `http://<name>.localhost:<PORT>`.
pub fn url(lane: &Lane) -> String {
    format!("http://{}.localhost:{}", lane.name, lane.ports.start)
}

/// The keys Lanes writes into a lane's `.env`.
const DOTENV_KEYS: [&str; 5] = ["PORT", "VITE_PORT", "LANE_NAME", "LANE_ID", "LANE_URL"];

/// Write the lane's `.env`: start from the lane's own `.env` if it has one, else from
/// `template` (relative to the lane), then set the lane's keys, replacing their lines or adding
/// them. Without either, nothing is written. Returns the file written. Other lines (secrets
/// included) are kept exactly as they are; nothing is copied from the user's own `.env`.
pub fn write_dotenv(lane: &Lane, template: Option<&Path>) -> Result<Option<PathBuf>> {
    let target = lane.path.join(".env");
    let start = if target.is_file() {
        fs::read_to_string(&target)
    } else if let Some(t) = template {
        let t = lane.path.join(t);
        if !t.is_file() {
            return Err(Error::new(
                ErrorKind::Config,
                format!(
                    "`env.template` in lane.toml names {}, which isn't there",
                    t.display()
                ),
            )
            .hint("Point it at a file the repo has, like `.env.example`, or remove it."));
        }
        fs::read_to_string(&t)
    } else {
        return Ok(None);
    }
    .map_err(|e| {
        Error::new(
            ErrorKind::Io,
            format!("couldn't read the template for {}: {e}", target.display()),
        )
        .hint("Check that the file is readable text.")
    })?;

    let values = vars(lane);
    let get = |k: &str| {
        values
            .iter()
            .find(|(key, _)| *key == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let mut seen = Vec::new();
    let mut lines: Vec<String> = start
        .lines()
        .map(|line| {
            let body = line
                .trim_start()
                .strip_prefix("export ")
                .unwrap_or(line.trim_start());
            match body.split_once('=') {
                Some((k, _)) if DOTENV_KEYS.contains(&k.trim()) => {
                    let k = k.trim().to_string();
                    let new = format!("{k}={}", get(&k));
                    seen.push(k);
                    new
                }
                _ => line.to_string(),
            }
        })
        .collect();
    let missing: Vec<_> = DOTENV_KEYS
        .iter()
        .filter(|k| !seen.iter().any(|s| s == *k))
        .collect();
    if !missing.is_empty() {
        if lines.last().is_some_and(|l| !l.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push("# Set by Lanes for this lane.".to_string());
        for k in missing {
            lines.push(format!("{k}={}", get(k)));
        }
    }
    let tmp = target.with_extension("lane-tmp");
    fs::write(&tmp, lines.join("\n") + "\n")
        .and_then(|()| fs::rename(&tmp, &target))
        .map_err(|e| {
            Error::new(
                ErrorKind::Io,
                format!("couldn't write {}: {e}", target.display()),
            )
            .hint("Check that you can write to the lane's folder.")
        })?;
    Ok(Some(target))
}

/// A command ready to run in the lane: its folder, its environment, and on Windows, npm-style
/// `.cmd` and `.ps1` launchers found the way a shell would.
pub fn command(lane: &Lane, argv: &[String]) -> Result<Command> {
    let (program, args) = argv.split_first().ok_or_else(|| {
        Error::usage("no command to run")
            .hint("Put the command after `--`, like `lane exec fix-auth -- npm run dev`.")
    })?;
    let values = vars(lane);
    let args: Vec<String> = args.iter().map(|a| expand(a, &values)).collect();
    let mut cmd = build(&expand(program, &values), &args);
    cmd.current_dir(&lane.path);
    for (k, v) in values {
        cmd.env(k, v);
    }
    Ok(cmd)
}

/// Fill in the lane's own variables in an argument, written `$PORT`, `${PORT}` or `%PORT%`, so
/// `--port $PORT` works the same everywhere without a shell. Other text is left alone.
pub fn expand(arg: &str, vars: &[(&'static str, String)]) -> String {
    let mut out = arg.to_string();
    // Longest names first, so `$LANE_PORTS` isn't read as `$LANE_PORT` + "S".
    let mut sorted: Vec<_> = vars.iter().collect();
    sorted.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
    for (k, v) in sorted {
        out = out
            .replace(&format!("${{{k}}}"), v)
            .replace(&format!("%{k}%"), v);
        // `$PORT` only when the name isn't followed by more name characters.
        let token = format!("${k}");
        let mut result = String::new();
        let mut rest = out.as_str();
        while let Some(i) = rest.find(&token) {
            let after = &rest[i + token.len()..];
            let boundary = !after.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_');
            result.push_str(&rest[..i]);
            result.push_str(if boundary { v } else { &token });
            rest = after;
        }
        result.push_str(rest);
        out = result;
    }
    out
}

#[cfg(not(windows))]
fn build(program: &str, args: &[String]) -> Command {
    let mut cmd = Command::new(program);
    cmd.args(args);
    cmd
}

#[cfg(windows)]
fn build(program: &str, args: &[String]) -> Command {
    use std::os::windows::process::CommandExt;
    let path = find_on_path(program).unwrap_or_else(|| PathBuf::from(program));
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        // cmd.exe runs batch files; the whole line is quoted once more so cmd keeps the inner quotes.
        "cmd" | "bat" => {
            let mut line = quote(&path.to_string_lossy());
            for a in args {
                line.push(' ');
                line.push_str(&quote(a));
            }
            let mut cmd = Command::new("cmd");
            cmd.arg("/D")
                .arg("/S")
                .arg("/C")
                .raw_arg(format!("\"{line}\""));
            cmd
        }
        "ps1" => {
            let mut cmd = Command::new("powershell");
            cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
                .arg(&path)
                .args(args);
            cmd
        }
        _ => {
            let mut cmd = Command::new(&path);
            cmd.args(args);
            cmd
        }
    }
}

/// `npm` → `C:\Program Files\nodejs\npm.cmd`, using `PATH` and `PATHEXT` (plus `.ps1`).
#[cfg(windows)]
fn find_on_path(program: &str) -> Option<PathBuf> {
    let p = Path::new(program);
    if p.extension().is_some() || p.components().count() > 1 {
        return None;
    }
    let exts: Vec<String> = std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
        .split(';')
        .filter(|e| !e.is_empty())
        .map(str::to_string)
        .chain([".PS1".to_string()])
        .collect();
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        exts.iter()
            .map(|ext| dir.join(format!("{program}{ext}")))
            .find(|c| c.is_file())
    })
}

#[cfg(windows)]
fn quote(s: &str) -> String {
    if !s.is_empty() && !s.contains([' ', '\t', '"', '&', '|', '<', '>', '^', '(', ')']) {
        s.to_string()
    } else {
        format!("\"{}\"", s.replace('"', "\"\""))
    }
}

/// The variables as lines for a shell: `sh` (`export K='v'`), `powershell` (`$env:K = 'v'`),
/// `cmd` (`set "K=v"`), or `dotenv` (`K=v`).
pub fn render(vars: &[(&'static str, String)], shell: &str) -> Result<String> {
    let line = |k: &str, v: &str| -> Result<String> {
        Ok(match shell {
            "sh" => format!("export {k}='{}'", v.replace('\'', "'\\''")),
            "powershell" | "pwsh" => format!("$env:{k} = '{}'", v.replace('\'', "''")),
            "cmd" => format!("set \"{k}={v}\""),
            "dotenv" => format!("{k}={v}"),
            other => {
                return Err(
                    Error::usage(format!("`{other}` isn't a shell Lanes can print for"))
                        .hint("Use --shell sh, powershell, cmd or dotenv."),
                );
            }
        })
    };
    let mut out = String::new();
    for (k, v) in vars {
        out.push_str(&line(k, v)?);
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{PortBlock, Status};

    fn lane(dir: &Path) -> Lane {
        Lane {
            id: "ln_abc".into(),
            repo: dir.join("acme"),
            name: "fix-auth".into(),
            path: dir.to_path_buf(),
            base_commit: None,
            ports: PortBlock {
                start: 4100,
                count: 10,
            },
            agent: None,
            outcome: None,
            status: Status::Ready,
            created_at: 0,
        }
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lane-env-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_lane_gets_its_ports_and_url() {
        let v = vars(&lane(Path::new("/l")));
        let get = |k| v.iter().find(|(key, _)| *key == k).unwrap().1.clone();
        assert_eq!(get("PORT"), "4100");
        assert_eq!(get("VITE_PORT"), "4101");
        assert_eq!(get("LANE_PORTS"), "4100-4109");
        assert_eq!(get("LANE_URL"), "http://fix-auth.localhost:4100");
        let mut one = lane(Path::new("/l"));
        one.ports.count = 1;
        assert_eq!(
            vars(&one)
                .iter()
                .find(|(k, _)| *k == "VITE_PORT")
                .unwrap()
                .1,
            "4100"
        );
    }

    #[test]
    fn the_dotenv_keeps_every_other_line_and_sets_the_lanes() {
        let dir = temp("dotenv");
        fs::write(
            dir.join(".env.example"),
            "# app\nexport PORT=3000\nAPI_KEY=\nDATABASE_URL=postgres://localhost/app\n",
        )
        .unwrap();
        let l = lane(&dir);
        let written = write_dotenv(&l, Some(Path::new(".env.example")))
            .unwrap()
            .unwrap();
        let text = fs::read_to_string(&written).unwrap();
        assert!(text.contains("PORT=4100\n"), "{text}");
        assert!(!text.contains("3000"), "{text}");
        assert!(
            text.contains("API_KEY=\n") && text.contains("DATABASE_URL=postgres://localhost/app"),
            "{text}"
        );
        assert!(
            text.contains("LANE_URL=http://fix-auth.localhost:4100"),
            "{text}"
        );
        assert_eq!(
            text.matches("PORT=").count(),
            2,
            "PORT and VITE_PORT, once each: {text}"
        );
        // Running it again changes nothing.
        write_dotenv(&l, Some(Path::new(".env.example"))).unwrap();
        assert_eq!(fs::read_to_string(&written).unwrap(), text);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_template_and_no_dotenv_writes_nothing() {
        let dir = temp("none");
        assert_eq!(write_dotenv(&lane(&dir), None).unwrap(), None);
        assert!(!dir.join(".env").exists());
        let e = write_dotenv(&lane(&dir), Some(Path::new(".env.missing"))).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Config);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lane_variables_are_filled_in_without_a_shell() {
        let v = vars(&lane(Path::new("/l")));
        assert_eq!(expand("--port", &v), "--port");
        assert_eq!(expand("$PORT", &v), "4100");
        assert_eq!(expand("--port=${PORT}", &v), "--port=4100");
        assert_eq!(expand("%VITE_PORT%", &v), "4101");
        assert_eq!(expand("$LANE_PORTS", &v), "4100-4109");
        assert_eq!(
            expand("$PORTAL $HOME", &v),
            "$PORTAL $HOME",
            "only the lane's own names, whole"
        );
        assert_eq!(
            expand("open $LANE_URL/login", &v),
            "open http://fix-auth.localhost:4100/login"
        );
    }

    #[test]
    fn shells_get_their_own_syntax() {
        let v = vec![
            ("PORT", "4100".to_string()),
            ("LANE_NAME", "it's".to_string()),
        ];
        assert_eq!(
            render(&v, "sh").unwrap(),
            "export PORT='4100'\nexport LANE_NAME='it'\\''s'\n"
        );
        assert_eq!(
            render(&v, "powershell").unwrap(),
            "$env:PORT = '4100'\n$env:LANE_NAME = 'it''s'\n"
        );
        assert_eq!(
            render(&v, "cmd").unwrap(),
            "set \"PORT=4100\"\nset \"LANE_NAME=it's\"\n"
        );
        assert_eq!(render(&v, "dotenv").unwrap(), "PORT=4100\nLANE_NAME=it's\n");
        assert!(render(&v, "fish").is_err());
    }

    #[test]
    fn a_command_runs_in_the_lane_with_its_variables() {
        let dir = temp("cmd");
        let l = lane(&dir);
        let argv: Vec<String> = if cfg!(windows) {
            ["cmd", "/C", "echo %PORT% %LANE_NAME%"]
                .map(String::from)
                .to_vec()
        } else {
            ["sh", "-c", "echo $PORT $LANE_NAME"]
                .map(String::from)
                .to_vec()
        };
        // Filled in from the arguments...
        let out = command(&l, &argv).unwrap().output().unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains("4100 fix-auth"));
        // ...and really in the environment, for servers that read it themselves.
        let list: Vec<String> = if cfg!(windows) {
            ["cmd", "/C", "set"].map(String::from).to_vec()
        } else {
            ["env"].map(String::from).to_vec()
        };
        let out = command(&l, &list)
            .unwrap()
            .env_remove("NOTHING")
            .output()
            .unwrap();
        let env = String::from_utf8_lossy(&out.stdout);
        assert!(env.lines().any(|l| l.trim() == "PORT=4100"), "{env}");
        assert!(
            env.lines()
                .any(|l| l.trim() == "LANE_URL=http://fix-auth.localhost:4100")
        );
        // It runs in the lane's folder.
        let pwd: Vec<String> = if cfg!(windows) {
            ["cmd", "/C", "cd"].map(String::from).to_vec()
        } else {
            ["pwd"].map(String::from).to_vec()
        };
        let out = command(&l, &pwd).unwrap().output().unwrap();
        let here = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert_eq!(
            fs::canonicalize(&here).unwrap(),
            fs::canonicalize(&dir).unwrap()
        );
        assert!(command(&l, &[]).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
