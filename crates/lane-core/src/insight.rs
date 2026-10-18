//! What the terminal app shows about lanes, measured rather than guessed. Per lane: its changed
//! files, its last test run, whether its app is running, and the extra disk it takes. Across
//! lanes: what they saved compared with the usual way, a worktree plus its own dependency
//! install for each agent.

use crate::log::Entry;
use crate::state::{Lane, LaneStats};
use serde::Serialize;
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::path::Path;
use std::time::Duration;

/// What a lane looks like right now.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Details {
    /// Files changed, added or deleted since the lane was made (`git status`, untracked included).
    pub changed: usize,
    /// Their size on disk: on a disk that clones, roughly the extra space the lane takes.
    pub changed_bytes: u64,
    /// Something answers on the lane's PORT or VITE_PORT: its app is up.
    pub running: bool,
    /// The last test command run in the lane with `lane exec`.
    pub tests: Option<TestRun>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TestRun {
    pub command: String,
    pub passed: bool,
    /// UTC, from the log.
    pub at: String,
}

/// Look at one lane. `log` is Lanes' recent log, for its test runs.
pub fn details(lane: &Lane, log: &[Entry]) -> Details {
    let (changed, changed_bytes) = changes(&lane.path);
    Details {
        changed,
        changed_bytes,
        running: running(lane),
        tests: last_tests(&lane.name, log),
    }
}

/// Files `git status` reports in the lane, and their size.
fn changes(path: &Path) -> (usize, u64) {
    let Some(out) = crate::git::try_run(
        path,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    ) else {
        return (0, 0);
    };
    let mut files = 0;
    let mut bytes = 0;
    let mut parts = out.split('\0').filter(|p| !p.is_empty());
    while let Some(entry) = parts.next() {
        if entry.len() < 4 {
            continue;
        }
        let (code, file) = entry.split_at(3);
        // A rename or copy is followed by its old name.
        if code.starts_with('R') || code.starts_with('C') {
            parts.next();
        }
        files += 1;
        bytes += std::fs::metadata(path.join(file))
            .map(|m| m.len())
            .unwrap_or(0);
    }
    (files, bytes)
}

/// Whether the lane's app answers on its PORT or VITE_PORT.
fn running(lane: &Lane) -> bool {
    let ports = [lane.ports.start, lane.ports.start.saturating_add(1)];
    ports.iter().any(|&p| {
        TcpStream::connect_timeout(
            &SocketAddrV4::new(Ipv4Addr::LOCALHOST, p).into(),
            Duration::from_millis(60),
        )
        .is_ok()
    })
}

/// The last `lane exec` of a test command in this lane, from the log.
pub fn last_tests(lane: &str, log: &[Entry]) -> Option<TestRun> {
    log.iter().rev().find_map(|e| {
        if e.event != "exec" || e.lane.as_deref() != Some(lane) {
            return None;
        }
        // "`npm test` exited with 0"
        let rest = e.message.strip_prefix('`')?;
        let (command, rest) = rest.split_once('`')?;
        let code: i64 = rest
            .trim()
            .strip_prefix("exited with")?
            .trim()
            .parse()
            .ok()?;
        let is_test = command
            .split_whitespace()
            .any(|w| w.contains("test") || w == "spec" || w.ends_with("jest"));
        is_test.then(|| TestRun {
            command: command.to_string(),
            passed: code == 0,
            at: e.at.clone(),
        })
    })
}

/// What lanes saved, compared with a worktree plus its own install for each.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Savings {
    pub lanes: usize,
    /// Lanes with a record of what they cost (made with 0.0.7 or later).
    pub measured: usize,
    /// Disk a worktree plus its own install would take for each measured lane.
    pub as_worktrees: u64,
    /// Disk the lanes actually add: a clone only its changed files, a copy all of it.
    pub as_lanes: u64,
    pub saved: u64,
    /// Lanes that skipped the dependency install, and the time those installs would have taken.
    pub installs_skipped: usize,
    pub install_ms_saved: u64,
    /// Lanes copied rather than cloned (no disk saved on this disk).
    pub copies: usize,
}

/// Add up what the lanes saved.
pub fn savings(rows: &[(&Lane, Option<&LaneStats>, Option<&Details>)]) -> Savings {
    let mut s = Savings {
        lanes: rows.len(),
        ..Savings::default()
    };
    for (_, stats, details) in rows {
        let Some(stats) = stats else { continue };
        s.measured += 1;
        s.as_worktrees += stats.bytes;
        let extra = match stats.method.as_str() {
            "clone" => details.map_or(0, |d| d.changed_bytes),
            _ => {
                s.copies += 1;
                stats.bytes
            }
        };
        s.as_lanes += extra;
        if let Some(ms) = stats.install_ms {
            s.installs_skipped += 1;
            s.install_ms_saved += ms;
        }
    }
    s.saved = s.as_worktrees.saturating_sub(s.as_lanes);
    s
}

/// `1.2 GB`, `340 MB`, `12 KB`, `0 B`.
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 1000.0 && unit < UNITS.len() - 1 {
        v /= 1000.0;
        unit += 1;
    }
    match unit {
        0 => format!("{n} B"),
        _ if v < 10.0 => format!("{v:.1} {}", UNITS[unit]),
        _ => format!("{v:.0} {}", UNITS[unit]),
    }
}

/// `48 s`, `4 min`, `1 h 5 min`.
pub fn duration(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min", (s + 30) / 60),
        _ => format!("{} h {} min", s / 3600, (s % 3600) / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::Level;
    use crate::state::{PortBlock, Status};
    use std::path::PathBuf;

    fn lane(name: &str) -> Lane {
        Lane {
            id: format!("l_{name}"),
            repo: PathBuf::from("/work/app"),
            name: name.into(),
            path: PathBuf::from(format!("/work/app.lanes/{name}")),
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

    fn exec(lane: &str, message: &str) -> Entry {
        let mut e = Entry::new(Level::Info, "exec", message);
        e.lane = Some(lane.into());
        e
    }

    #[test]
    fn the_last_test_run_comes_from_the_log() {
        let log = vec![
            exec("fix-auth", "`npm test` exited with 1"),
            exec("fix-auth", "`npm run dev` exited with 0"),
            exec("other", "`cargo test` exited with 0"),
            exec("fix-auth", "`npx vitest run` exited with 0"),
        ];
        let t = last_tests("fix-auth", &log).unwrap();
        assert_eq!(t.command, "npx vitest run");
        assert!(t.passed);
        assert!(!last_tests("fix-auth", &log[..2]).unwrap().passed);
        assert!(last_tests("nobody", &log).is_none());
    }

    #[test]
    fn savings_count_clones_by_what_changed_and_copies_in_full() {
        let (a, b, c) = (lane("a"), lane("b"), lane("c"));
        let clone = LaneStats {
            method: "clone".into(),
            files: 10,
            bytes: 500_000_000,
            made_ms: 900,
            install_ms: Some(48_000),
        };
        let copy = LaneStats {
            method: "copy".into(),
            install_ms: None,
            ..clone.clone()
        };
        let changed = Details {
            changed_bytes: 2_000_000,
            ..Details::default()
        };
        let s = savings(&[
            (&a, Some(&clone), Some(&changed)),
            (&b, Some(&copy), None),
            (&c, None, None),
        ]);
        assert_eq!(s.lanes, 3);
        assert_eq!(s.measured, 2);
        assert_eq!(s.as_worktrees, 1_000_000_000);
        assert_eq!(s.as_lanes, 502_000_000);
        assert_eq!(s.saved, 498_000_000);
        assert_eq!((s.installs_skipped, s.install_ms_saved), (1, 48_000));
        assert_eq!(s.copies, 1);
    }

    #[test]
    fn sizes_and_times_read_well() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(999), "999 B");
        assert_eq!(bytes(1_234_567), "1.2 MB");
        assert_eq!(bytes(5_800_000_000), "5.8 GB");
        assert_eq!(bytes(340_000_000), "340 MB");
        assert_eq!(duration(48_000), "48 s");
        assert_eq!(duration(240_000), "4 min");
        assert_eq!(duration(3_900_000), "1 h 5 min");
    }

    #[test]
    fn a_lane_with_nothing_listening_isnt_running() {
        let mut l = lane("quiet");
        // A port nothing should be on.
        l.ports.start = 1;
        assert!(!running(&l));
    }
}
