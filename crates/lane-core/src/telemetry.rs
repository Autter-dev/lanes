//! The anonymous install count: one small message when Lanes is set up, and one after each
//! upgrade. It says a random install id, the Lanes version, the OS and CPU type, and how Lanes was
//! installed, and nothing else: never code, file paths, repo names or what you do with Lanes.
//!
//! It goes out only once the setting has been chosen (the welcome, or `lane setup`), so nothing
//! is sent while someone looks around. Never in CI, never with an opt-out set (`lane telemetry
//! off`, `LANE_TELEMETRY=0`, `DO_NOT_TRACK=1`), never from unit tests. `curl` sends it detached,
//! with a 1-second limit, so no command waits for it; if it fails, it's dropped, not retried.
//!
//! The server is `services/install-count` in this repo: a Cloudflare Worker that keeps counts in
//! D1 and accepts nothing beyond these five fields.

use crate::VERSION;
use crate::config::{Config, config_dir};
use crate::env::Env;
use crate::error::{Error, ErrorKind, Result};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::RandomState;
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where release builds send the count: the Worker in `services/install-count`, which keeps it
/// in D1. `LANE_TELEMETRY_URL` points it elsewhere (tests, staging).
pub const ENDPOINT: Option<&str> = Some("https://lanes-count.autter.dev/v1/installs");

/// The whole message. Its fields are the list the welcome shows, and a test keeps it that way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ping {
    /// Random, made on this machine, not linked to anyone.
    pub id: String,
    pub version: &'static str,
    pub os: &'static str,
    pub arch: &'static str,
    /// How Lanes was installed: see [`channel`].
    pub channel: &'static str,
}

/// What's kept between runs: the install id, and the last version counted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Install {
    pub id: Option<String>,
    pub counted: Option<String>,
}

/// Why nothing was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// An environment opt-out, or CI.
    ForcedOff(&'static str),
    /// `telemetry = false` in the settings.
    SettingOff,
    /// The welcome or `lane setup` hasn't run, so the setting hasn't been chosen yet.
    NotChosen,
    /// A development build with no `LANE_TELEMETRY_URL`: it never sends.
    NoEndpoint,
    /// This version was already counted on this machine.
    Counted,
}

impl Skip {
    pub fn describe(self) -> String {
        match self {
            Skip::ForcedOff(why) => format!("off: {why}"),
            Skip::SettingOff => "off in your settings".into(),
            Skip::NotChosen => "not sent until you run `lane setup` or the welcome".into(),
            Skip::NoEndpoint => "this is a development build, which never sends it".into(),
            Skip::Counted => format!("already counted for {VERSION}"),
        }
    }
}

/// How Lanes was installed, from where the running binary lives.
pub fn channel(exe: &Path) -> &'static str {
    let p = exe
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    if p.contains("/node_modules/") {
        "npm"
    } else if p.contains("/cellar/") || p.contains("/homebrew/") || p.contains("linuxbrew/") {
        "homebrew"
    } else if p.contains("/scoop/") {
        "scoop"
    } else if p.contains("/winget/") {
        "winget"
    } else if p.contains("/.cargo/bin/") {
        "cargo"
    } else if p.contains("/target/debug/") || p.contains("/target/release/") {
        "source"
    } else if p.ends_with("/.local/bin/lane") || p.contains("/programs/lanes/bin/") {
        // The install scripts' default folders.
        "script"
    } else {
        "other"
    }
}

/// This machine's message, with `id`.
pub fn ping(id: &str) -> Ping {
    let exe = std::env::current_exe()
        .map(|p| fs::canonicalize(&p).unwrap_or(p))
        .unwrap_or_default();
    Ping {
        id: id.to_string(),
        version: VERSION,
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        channel: channel(&exe),
    }
}

/// Where the count would go, if anywhere.
/// Development builds (and so every test binary) never use [`ENDPOINT`]: someone's own builds
/// and test runs aren't installs. They send only where `LANE_TELEMETRY_URL` says.
pub fn endpoint(env: &Env) -> Option<String> {
    env.var("LANE_TELEMETRY_URL")
        .map(str::to_string)
        .or_else(|| {
            (!cfg!(debug_assertions))
                .then_some(ENDPOINT)
                .flatten()
                .map(str::to_string)
        })
}

/// Whether to send now, and where. Pure, so every rule is tested without a network.
pub fn decide(
    env: &Env,
    config: &Config,
    install: &Install,
    endpoint: Option<String>,
) -> std::result::Result<String, Skip> {
    if let Some(why) = env.telemetry_forced_off() {
        return Err(Skip::ForcedOff(why));
    }
    if !config.telemetry {
        return Err(Skip::SettingOff);
    }
    if !config.is_onboarded() {
        return Err(Skip::NotChosen);
    }
    let url = endpoint.ok_or(Skip::NoEndpoint)?;
    if install.counted.as_deref() == Some(VERSION) {
        return Err(Skip::Counted);
    }
    Ok(url)
}

/// The file holding [`Install`].
pub fn install_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("install.json"))
}

impl Install {
    /// What's saved, or nothing yet. A damaged file counts as nothing: it's rewritten next send.
    pub fn load() -> Install {
        install_path()
            .ok()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self) -> Result<()> {
        let path = install_path()?;
        let io = |e: std::io::Error| {
            Error::new(
                ErrorKind::Failure,
                format!("couldn't write {}: {e}", path.display()),
            )
            .hint("Check that you can write to Lanes' settings folder.")
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(io)?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(
            &tmp,
            serde_json::to_string_pretty(self).expect("serializes"),
        )
        .map_err(io)?;
        fs::rename(&tmp, &path).map_err(io)
    }
}

/// 32 hex characters from the OS's randomness (std seeds its hash keys from it).
fn new_id() -> String {
    let half = || {
        let mut h = RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default(),
        );
        h.write_u32(std::process::id());
        h.finish()
    };
    format!("{:016x}{:016x}", half(), half())
}

/// Count this install if the rules allow, in the background. Called once at the end of every
/// command; costs a file read when there's nothing to send.
pub fn maybe_send(env: &Env, config: &Config) -> std::result::Result<(), Skip> {
    if cfg!(test) {
        return Err(Skip::NoEndpoint);
    }
    let mut install = Install::load();
    let url = decide(env, config, &install, endpoint(env))?;
    let id = install.id.clone().unwrap_or_else(new_id);
    install.id = Some(id.clone());
    install.counted = Some(VERSION.to_string());
    // Mark it counted first: a send that fails is dropped, never retried on every command.
    if install.save().is_err() {
        return Err(Skip::Counted);
    }
    let body = serde_json::to_string(&ping(&id)).expect("serializes");
    if send(&url, &body) {
        crate::log::info("telemetry", format!("counted this install of {VERSION}")).write();
    }
    Ok(())
}

/// Start `curl` to post `body`, and don't wait for it. False if `curl` couldn't start.
fn send(url: &str, body: &str) -> bool {
    let mut cmd = Command::new("curl");
    cmd.args([
        "--silent",
        "--max-time",
        "1",
        "--connect-timeout",
        "1",
        "--proto",
        "=https,http",
        "--request",
        "POST",
        "--header",
        "Content-Type: application/json",
        "--data-binary",
        "@-",
        "--output",
        if cfg!(windows) { "NUL" } else { "/dev/null" },
        "--url",
        url,
    ])
    .stdin(Stdio::piped())
    .stdout(Stdio::null())
    .stderr(Stdio::null());
    #[cfg(windows)]
    {
        // No console window flashing up when an agent's hook runs `lane` from a GUI app.
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    // Otherwise, on Windows, curl would hold `lane`'s output open for up to a second.
    crate::process::keep_std_handles_to_ourselves();
    let Ok(mut child) = cmd.spawn() else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(body.as_bytes());
    }
    // Dropping `child` doesn't stop it: curl finishes (or gives up after 1 s) on its own.
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_up() -> Config {
        Config {
            onboarded: Some("0.0.1".into()),
            ..Config::default()
        }
    }

    const URL: Option<&str> = Some("https://example.test/count");

    fn decide_with(
        vars: &[(&str, &str)],
        config: &Config,
        install: &Install,
    ) -> std::result::Result<String, Skip> {
        decide(
            &Env::fake(vars, false),
            config,
            install,
            URL.map(str::to_string),
        )
    }

    #[test]
    fn it_sends_once_per_version_after_setup() {
        let none = Install::default();
        assert_eq!(
            decide_with(&[], &set_up(), &none).as_deref(),
            Ok("https://example.test/count")
        );
        let counted = Install {
            id: Some("abc".into()),
            counted: Some(VERSION.into()),
        };
        assert_eq!(decide_with(&[], &set_up(), &counted), Err(Skip::Counted));
        let older = Install {
            counted: Some("0.0.1".into()),
            ..counted
        };
        assert!(
            decide_with(&[], &set_up(), &older).is_ok(),
            "an upgrade counts again"
        );
    }

    #[test]
    fn every_opt_out_wins() {
        let none = Install::default();
        for vars in [
            &[("DO_NOT_TRACK", "1")][..],
            &[("LANE_TELEMETRY", "0")],
            &[("CI", "true")],
            &[("GITHUB_ACTIONS", "true")],
        ] {
            assert!(
                matches!(decide_with(vars, &set_up(), &none), Err(Skip::ForcedOff(_))),
                "{vars:?}"
            );
        }
        let off = Config {
            telemetry: false,
            ..set_up()
        };
        assert_eq!(decide_with(&[], &off, &none), Err(Skip::SettingOff));
    }

    #[test]
    fn nothing_goes_before_the_setting_is_chosen_or_without_an_address() {
        let none = Install::default();
        assert_eq!(
            decide_with(&[], &Config::default(), &none),
            Err(Skip::NotChosen)
        );
        assert_eq!(
            decide(&Env::fake(&[], false), &set_up(), &none, None),
            Err(Skip::NoEndpoint)
        );
    }

    #[test]
    fn only_release_builds_use_the_real_address() {
        let none = Env::fake(&[], false);
        let release = !cfg!(debug_assertions);
        assert_eq!(
            endpoint(&none).is_some(),
            release,
            "test runs never reach it"
        );
        let told = Env::fake(&[("LANE_TELEMETRY_URL", "http://127.0.0.1:1/")], false);
        assert_eq!(endpoint(&told).as_deref(), Some("http://127.0.0.1:1/"));
    }

    #[test]
    fn the_message_holds_exactly_what_the_welcome_lists() {
        let json = serde_json::to_value(ping("0123")).unwrap();
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort();
        assert_eq!(keys, ["arch", "channel", "id", "os", "version"]);
        assert_eq!(json["version"], VERSION);
    }

    #[test]
    fn ids_are_random_hex() {
        let (a, b) = (new_id(), new_id());
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn the_channel_comes_from_where_lane_lives() {
        for (path, want) in [
            ("/opt/homebrew/Cellar/lanes/0.0.7/bin/lane", "homebrew"),
            ("/usr/local/Cellar/lanes/0.0.7/bin/lane", "homebrew"),
            ("/home/linuxbrew/.linuxbrew/bin/lane", "homebrew"),
            (
                "/usr/lib/node_modules/@autter/lanes-linux-x64/bin/lane",
                "npm",
            ),
            (
                r"C:\Users\me\AppData\Roaming\npm\node_modules\@autter\lanes-win32-x64\lane.exe",
                "npm",
            ),
            (r"C:\Users\me\scoop\apps\lanes\current\lane.exe", "scoop"),
            (
                r"C:\Users\me\AppData\Local\Microsoft\WinGet\Packages\Autter.Lanes_x\lane.exe",
                "winget",
            ),
            ("/home/me/.cargo/bin/lane", "cargo"),
            ("/home/me/src/lanes/target/release/lane", "source"),
            ("/home/me/.local/bin/lane", "script"),
            (
                r"C:\Users\me\AppData\Local\Programs\Lanes\bin\lane.exe",
                "script",
            ),
            ("/usr/bin/lane", "other"),
        ] {
            assert_eq!(channel(Path::new(path)), want, "{path}");
        }
    }
}
